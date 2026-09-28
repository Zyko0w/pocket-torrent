//! GoFile (https://gofile.io/api).
//!
//! Chaque fichier part en un seul envoi (formulaire multipart, lu au fil de l'eau). Le premier
//! crée un dossier public, les suivants y sont rangés : un seul lien pour le tout, même pour un
//! seul fichier (GoFile ne donne pas de lien par fichier). Sans jeton de compte, GoFile crée un
//! compte invité et renvoie son jeton : gardé avec le lien, c'est lui qui permet de supprimer
//! le dossier. GoFile efface les fichiers après une période sans téléchargement (ni durée de vie
//! ni limite de téléchargements réglables sans abonnement).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::{Context, bail};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::json;

use super::http::{Multipart, check_status, file_size, send, share_url, valid_id, with_progress};
use super::{FileHost, Hosted, Uploaded};
use crate::secrets::{self, Secret};

/// Adresses de l'API : seules destinations des requêtes.
pub const API: &str = "https://api.gofile.io";
/// Envoi des fichiers (GoFile choisit le serveur le plus proche).
pub const UPLOAD: &str = "https://upload.gofile.io";

pub struct Gofile {
    api: String,
    upload: String,
    account_token: Option<String>,
}

#[derive(Deserialize)]
struct Reply<T> {
    status: String,
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileUploaded {
    download_page: String,
    parent_folder: String,
    guest_token: Option<String>,
}

impl Gofile {
    pub fn from_secrets() -> Self {
        Self { api: API.to_owned(), upload: UPLOAD.to_owned(), account_token: secrets::get(Secret::GofileToken) }
    }

    #[cfg(test)]
    fn for_tests(base: &str, account_token: Option<&str>) -> Self {
        Self { api: base.to_owned(), upload: base.to_owned(), account_token: account_token.map(str::to_owned) }
    }

    /// Jeton pour agir sur un dossier : celui du compte invité gardé avec le lien, sinon le compte.
    fn token_for(&self, owner_token: &str) -> anyhow::Result<String> {
        match owner_token {
            "" => self.account_token.clone().context("jeton de compte GoFile manquant (Réglages › Envoi en ligne)"),
            guest => Ok(guest.to_owned()),
        }
    }
}

impl FileHost for Gofile {
    async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        match &self.account_token {
            Some(token) => {
                let request = client.get(format!("{}/accounts/getid", self.api)).bearer_auth(token);
                data::<serde_json::Value>(request, "vérification du compte").await?;
                Ok("Compte GoFile relié.".into())
            }
            None => {
                data::<serde_json::Value>(client.get(format!("{}/servers", self.api)), "état du service").await?;
                Ok("GoFile joignable (sans compte : un compte invité est créé à chaque envoi).".into())
            }
        }
    }

    async fn upload(
        &self,
        client: &reqwest::Client,
        path: &Path,
        file_name: &str,
        _file_count: usize,
        group: Option<&Hosted>,
        on_progress: &mut (dyn FnMut(u64) + Send),
    ) -> anyhow::Result<Uploaded> {
        let size = file_size(path).await?;
        let fields: Vec<(&str, &str)> = group.iter().map(|folder| ("folderId", folder.id.as_str())).collect();
        let sent = Arc::new(AtomicU64::new(0));
        let form = Multipart::new(&fields, "file", file_name, size);
        let (content_type, length) = (form.content_type(), form.length());
        let body = form.body(tokio::fs::File::open(path).await?, sent.clone());
        let token = match group {
            Some(folder) => Some(self.token_for(&folder.owner_token)?),
            None => self.account_token.clone(),
        };
        let mut request = client
            .post(format!("{}/uploadfile", self.upload))
            .header(CONTENT_TYPE, content_type)
            .header(CONTENT_LENGTH, length)
            .body(body);
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }
        let uploaded: FileUploaded = with_progress(data(request, "envoi du fichier"), &sent, on_progress).await?;
        on_progress(size);

        if group.is_some() {
            return Ok(Uploaded::default());
        }
        // compte des réglages : son jeton reste dans Windows, rien à garder avec le lien
        let owner_token = match (&self.account_token, uploaded.guest_token) {
            (Some(_), _) => String::new(),
            (None, Some(guest)) if !guest.is_empty() => guest,
            (None, _) => bail!("réponse de GoFile sans jeton de compte invité"),
        };
        // identifiant renvoyé par le serveur, mis ensuite dans les envois suivants
        if !valid_id(&uploaded.parent_folder) {
            bail!("identifiant de dossier GoFile invalide dans la réponse");
        }
        let folder = Hosted { id: uploaded.parent_folder, url: share_url(&uploaded.download_page)?, owner_token };
        Ok(Uploaded { group: Some(folder), file: None })
    }

    async fn delete(&self, client: &reqwest::Client, _kind: &str, id: &str, owner_token: &str) -> anyhow::Result<()> {
        if !valid_id(id) {
            bail!("identifiant GoFile invalide");
        }
        let request = client
            .delete(format!("{}/contents", self.api))
            .bearer_auth(self.token_for(owner_token)?)
            .json(&json!({ "contentsId": id }));
        let response = send(request, "GoFile").await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let reply: Reply<serde_json::Value> = check_status(response, "GoFile", "suppression")
            .await?
            .json()
            .await
            .context("réponse de GoFile illisible (suppression)")?;
        if reply.status == "error-notFound" {
            return Ok(()); // effacé après inactivité, ou déjà supprimé
        }
        ok(reply, "suppression").map(|_| ())
    }
}

/// GoFile répond `{ "status": "ok", "data": … }`, ou un statut « error-… ».
fn ok<T>(reply: Reply<T>, step: &str) -> anyhow::Result<T> {
    if reply.status != "ok" {
        let status: String = reply.status.chars().take(80).collect();
        bail!("GoFile a refusé l'étape « {step} » ({status})");
    }
    reply.data.with_context(|| format!("réponse de GoFile sans données ({step})"))
}

async fn data<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder, step: &str) -> anyhow::Result<T> {
    let response = check_status(send(request, "GoFile").await?, "GoFile", step).await?;
    let reply: Reply<T> = response.json().await.with_context(|| format!("réponse de GoFile illisible ({step})"))?;
    ok(reply, step)
}

/// Tests dans un bac à sable : un faux serveur GoFile local (aucune requête vers le vrai).
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::{delete, post};
    use axum::{Json, Router};

    use super::super::http::sandbox_file;
    use super::*;

    #[derive(Default)]
    struct Fake {
        /// Formulaire multipart brut de chaque envoi, et jeton présenté.
        bodies: Vec<(Vec<u8>, Option<String>)>,
        deleted: Vec<(String, String)>,
    }

    type FakeState = Arc<Mutex<Fake>>;

    const FOLDER: &str = "4f1c2d3e-aaaa-bbbb-cccc-0123456789ab";

    fn bearer(headers: &HeaderMap) -> Option<String> {
        headers.get("authorization").map(|h| h.to_str().unwrap().trim_start_matches("Bearer ").to_owned())
    }

    async fn fake_server() -> (String, FakeState) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state: FakeState = Arc::default();
        let app = Router::new()
            .route(
                "/uploadfile",
                post(|State(s): State<FakeState>, headers: HeaderMap, body: axum::body::Bytes| async move {
                    let token = bearer(&headers);
                    s.lock().unwrap().bodies.push((body.to_vec(), token.clone()));
                    let guest = if token.is_none() { json!("guest_tok") } else { json!(null) };
                    Json(json!({ "status": "ok", "data": { "downloadPage": "https://gofile.io/d/AbC12", "id": "f-1",
                        "parentFolder": FOLDER, "guestToken": guest } }))
                }),
            )
            .route(
                "/contents",
                delete(
                    |State(s): State<FakeState>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                        let token = bearer(&headers).unwrap_or_default();
                        if token != "guest_tok" && token != "compte" {
                            return Json(json!({ "status": "error-notPremium" }));
                        }
                        s.lock().unwrap().deleted.push((body["contentsId"].as_str().unwrap().to_owned(), token));
                        Json(json!({ "status": "ok", "data": {} }))
                    },
                ),
            )
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, state)
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[tokio::test]
    async fn guest_folder_gathers_the_files_and_can_be_deleted() {
        let (base, state) = fake_server().await;
        let gofile = Gofile::for_tests(&base, None);
        let http = reqwest::Client::new();
        let (path, data) = sandbox_file(600 * 1024);
        let mut progress = 0;
        let first = gofile.upload(&http, &path, "Film \"test\".mkv", 2, None, &mut |p| progress = p).await.unwrap();
        assert_eq!(progress, data.len() as u64);
        let folder = first.group.unwrap();
        assert_eq!(
            folder,
            Hosted { id: FOLDER.into(), url: "https://gofile.io/d/AbC12".into(), owner_token: "guest_tok".into() }
        );
        assert!(first.file.is_none(), "un seul lien : celui du dossier");
        // second fichier : même dossier, avec le jeton du compte invité ; pas de nouveau groupe
        let second = gofile.upload(&http, &path, "Film de test.mkv", 2, Some(&folder), &mut |_| {}).await.unwrap();
        assert!(second.group.is_none() && second.file.is_none());
        {
            let s = state.lock().unwrap();
            let (body, token) = &s.bodies[0];
            assert!(token.is_none());
            assert!(find(body, b"filename=\"Film %22test%22.mkv\"").is_some());
            let start = find(body, b"\r\n\r\n").unwrap() + 4;
            assert_eq!(&body[start..start + data.len()], &data[..], "fichier transmis à l'identique");
            let (body, token) = &s.bodies[1];
            assert_eq!(token.as_deref(), Some("guest_tok"));
            assert!(find(body, format!("name=\"folderId\"\r\n\r\n{FOLDER}\r\n").as_bytes()).is_some());
        }
        gofile.delete(&http, "collection", FOLDER, "guest_tok").await.unwrap();
        assert!(gofile.delete(&http, "collection", FOLDER, "mauvais").await.is_err());
        for id in ["../accounts", "a/b", "x?y=1", ""] {
            assert!(gofile.delete(&http, "collection", id, "guest_tok").await.is_err(), "{id}");
        }
        assert_eq!(state.lock().unwrap().deleted, vec![(FOLDER.to_owned(), "guest_tok".into())]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn account_token_stays_out_of_the_saved_link() {
        let (base, state) = fake_server().await;
        let gofile = Gofile::for_tests(&base, Some("compte"));
        let http = reqwest::Client::new();
        let (path, _) = sandbox_file(10);
        let folder = gofile.upload(&http, &path, "a.mkv", 1, None, &mut |_| {}).await.unwrap().group.unwrap();
        assert_eq!(folder.owner_token, "");
        assert_eq!(state.lock().unwrap().bodies[0].1.as_deref(), Some("compte"));
        gofile.delete(&http, "collection", &folder.id, "").await.unwrap();
        let without_account = Gofile::for_tests(&base, None);
        assert!(without_account.delete(&http, "collection", &folder.id, "").await.is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
