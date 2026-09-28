//! VikingFile (https://vikingfile.com/api).
//!
//! `get-upload-url` donne des adresses d'envoi signées, une par partie, où les octets partent
//! directement ; `complete-upload` crée ensuite le fichier et renvoie son lien. Un lien par
//! fichier. La suppression demande l'identifiant du compte (« user hash ») : sans compte, les
//! fichiers restent en ligne jusqu'à leur effacement par VikingFile.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde::Deserialize;
use tokio::io::AsyncSeekExt;

use super::http::{
    check_status, clean_name, file_size, file_stream, send, share_url, signed_url, valid_id, with_progress,
};
use super::{FileHost, Hosted, Uploaded};
use crate::secrets::{self, Secret};

/// Adresse de l'API : seule destination des requêtes (hors adresses d'envoi signées qu'elle fournit).
pub const API: &str = "https://vikingfile.com/api";

pub struct Vikingfile {
    api: String,
    user: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadUrls {
    upload_id: String,
    key: String,
    part_size: u64,
    number_parts: u32,
    urls: Vec<String>,
}

#[derive(Deserialize)]
struct Created {
    hash: String,
    url: String,
}

impl Vikingfile {
    pub fn from_secrets() -> Self {
        Self { api: API.to_owned(), user: secrets::get(Secret::VikingfileUser) }
    }

    #[cfg(test)]
    fn for_tests(base: &str, user: Option<&str>) -> Self {
        Self { api: base.to_owned(), user: user.map(str::to_owned) }
    }

    fn form(&self, client: &reqwest::Client, endpoint: &str, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish();
        client
            .post(format!("{}/{endpoint}", self.api))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
    }
}

impl FileHost for Vikingfile {
    async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        match &self.user {
            Some(user) => {
                let request = self.form(client, "list-files", &[("user", user), ("page", "1")]);
                api_json::<serde_json::Value>(request, "vérification du compte").await?;
                Ok("Compte VikingFile relié.".into())
            }
            None => {
                api_json::<serde_json::Value>(client.get(format!("{}/get-server", self.api)), "état du service")
                    .await?;
                Ok("VikingFile joignable (sans compte : les fichiers envoyés ne pourront pas être supprimés).".into())
            }
        }
    }

    async fn upload(
        &self,
        client: &reqwest::Client,
        path: &Path,
        file_name: &str,
        _file_count: usize,
        _group: Option<&Hosted>,
        on_progress: &mut (dyn FnMut(u64) + Send),
    ) -> anyhow::Result<Uploaded> {
        let size = file_size(path).await?;
        let size_text = size.to_string();
        let init: UploadUrls =
            api_json(self.form(client, "get-upload-url", &[("size", &size_text)]), "début de l'envoi").await?;
        if init.part_size == 0
            || u64::from(init.number_parts) != size.div_ceil(init.part_size)
            || init.urls.len() != init.number_parts as usize
        {
            bail!("découpage de l'envoi VikingFile incohérent");
        }

        let sent = Arc::new(AtomicU64::new(0));
        let mut etags = Vec::with_capacity(init.urls.len());
        for (index, url) in init.urls.iter().enumerate() {
            let url = signed_url(url)?;
            let offset = index as u64 * init.part_size;
            let len = init.part_size.min(size - offset);
            let etag = with_progress(put_part(client, &url, path, offset, len, &sent), &sent, on_progress)
                .await
                .with_context(|| format!("partie {} sur {}", index + 1, init.number_parts))?;
            etags.push(etag);
        }

        let mut fields: Vec<(String, String)> = vec![
            ("key".into(), init.key),
            ("uploadId".into(), init.upload_id),
            ("name".into(), clean_name(file_name)),
            // vide : envoi anonyme
            ("user".into(), self.user.clone().unwrap_or_default()),
        ];
        for (index, etag) in etags.into_iter().enumerate() {
            fields.push((format!("parts[{index}][PartNumber]"), (index + 1).to_string()));
            fields.push((format!("parts[{index}][ETag]"), etag));
        }
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let created: Created = api_json(self.form(client, "complete-upload", &fields), "fin de l'envoi").await?;
        on_progress(size);
        if !valid_id(&created.hash) {
            bail!("identifiant VikingFile invalide dans la réponse");
        }
        // la suppression passe par le compte des réglages : rien à garder avec le lien
        let file = Hosted { id: created.hash, url: share_url(&created.url)?, owner_token: String::new() };
        Ok(Uploaded { group: None, file: Some(file) })
    }

    async fn delete(&self, client: &reqwest::Client, _kind: &str, id: &str, _owner_token: &str) -> anyhow::Result<()> {
        if !valid_id(id) {
            bail!("identifiant VikingFile invalide");
        }
        let user =
            self.user.as_deref().context("identifiant de compte VikingFile manquant : suppression impossible")?;
        let request = self.form(client, "delete-file", &[("hash", id), ("user", user)]);
        api_json::<serde_json::Value>(request, "suppression").await.map(|_| ())
    }

    fn can_delete(&self) -> bool {
        self.user.is_some()
    }
}

/// Envoie une partie vers son adresse signée (jusqu'à 3 essais) ; renvoie son ETag.
async fn put_part(
    client: &reqwest::Client,
    url: &reqwest::Url,
    path: &Path,
    offset: u64,
    len: u64,
    sent: &Arc<AtomicU64>,
) -> anyhow::Result<String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        sent.store(offset, Ordering::Relaxed);
        let result = async {
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let body = reqwest::Body::wrap_stream(file_stream(file, len, sent.clone()));
            let request = client.put(url.clone()).header(CONTENT_LENGTH, len).body(body);
            let response = check_status(send(request, "VikingFile").await?, "VikingFile", "envoi des données").await?;
            response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .context("VikingFile n'a pas confirmé la partie")
        }
        .await;
        match result {
            Ok(etag) => return Ok(etag),
            Err(err) if attempt >= 3 => return Err(err),
            Err(err) => {
                tracing::warn!(attempt, "partie à renvoyer : {err:#}");
                tokio::time::sleep(Duration::from_secs(5 * attempt)).await;
            }
        }
    }
}

/// VikingFile signale ses erreurs par `{ "error": "…" }` (« success » en cas de réussite).
async fn api_json<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder, step: &str) -> anyhow::Result<T> {
    let response = check_status(send(request, "VikingFile").await?, "VikingFile", step).await?;
    let value: serde_json::Value =
        response.json().await.with_context(|| format!("réponse de VikingFile illisible ({step})"))?;
    if let Some(error) = value.get("error").and_then(|e| e.as_str()).filter(|e| *e != "success") {
        let error: String = error.chars().take(200).collect();
        bail!("VikingFile a refusé l'étape « {step} » : {error}");
    }
    serde_json::from_value(value).with_context(|| format!("réponse de VikingFile illisible ({step})"))
}

/// Tests dans un bac à sable : un faux serveur VikingFile local (aucune requête vers le vrai).
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use axum::extract::{Path as UrlPath, State};
    use axum::routing::{post, put};
    use axum::{Json, Router};
    use serde_json::json;

    use super::super::http::sandbox_file;
    use super::*;

    #[derive(Default)]
    struct Fake {
        parts: Vec<(u32, Vec<u8>)>,
        completed: Vec<HashMap<String, String>>,
        deleted: Vec<(String, String)>,
    }

    type FakeState = Arc<Mutex<Fake>>;

    /// Formulaire reçu (axum est compilé sans son extracteur de formulaires).
    fn form(body: &str) -> HashMap<String, String> {
        url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
    }

    async fn fake_server(part_size: u64) -> (String, FakeState) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state: FakeState = Arc::default();
        let b = base.clone();
        let app = Router::new()
            .route(
                "/get-upload-url",
                post(move |body: String| {
                    let b = b.clone();
                    async move {
                        let form = form(&body);
                        let size: u64 = form["size"].parse().unwrap();
                        let parts = size.div_ceil(part_size);
                        let urls: Vec<String> = (1..=parts).map(|n| format!("{b}/s3/{n}?sig=x")).collect();
                        Json(json!({ "uploadId": "UP1", "key": "rZ2h9ZqVQi", "partSize": part_size,
                            "numberParts": parts, "urls": urls }))
                    }
                }),
            )
            .route(
                "/s3/{part}",
                put(|State(s): State<FakeState>, UrlPath(part): UrlPath<u32>, body: axum::body::Bytes| async move {
                    s.lock().unwrap().parts.push((part, body.to_vec()));
                    ([("etag", format!("\"etag-{part}\""))], "")
                }),
            )
            .route(
                "/complete-upload",
                post(|State(s): State<FakeState>, body: String| async move {
                    s.lock().unwrap().completed.push(form(&body));
                    Json(json!({ "name": "Film de test.mkv", "size": 1, "hash": "TPRSfLvcIu",
                        "url": "https://vikingfile.com/f/TPRSfLvcIu" }))
                }),
            )
            .route(
                "/delete-file",
                post(|State(s): State<FakeState>, body: String| async move {
                    let form = form(&body);
                    if form["user"] != "hash_compte" {
                        return Json(json!({ "error": "wrong user" }));
                    }
                    s.lock().unwrap().deleted.push((form["hash"].clone(), form["user"].clone()));
                    Json(json!({ "error": "success" }))
                }),
            )
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, state)
    }

    #[tokio::test]
    async fn parts_are_sent_in_order_then_completed() {
        let (base, state) = fake_server(4000).await;
        let viking = Vikingfile::for_tests(&base, Some("hash_compte"));
        let (path, data) = sandbox_file(10_000);
        let mut progress = 0;
        let uploaded = viking
            .upload(&reqwest::Client::new(), &path, "Film de test.mkv", 1, None, &mut |p| progress = p)
            .await
            .unwrap();
        assert!(uploaded.group.is_none());
        let file = uploaded.file.unwrap();
        assert_eq!((file.id.as_str(), file.url.as_str()), ("TPRSfLvcIu", "https://vikingfile.com/f/TPRSfLvcIu"));
        assert_eq!(file.owner_token, "", "l'identifiant du compte n'est jamais gardé avec le lien");
        assert_eq!(progress, 10_000);
        let s = state.lock().unwrap();
        assert_eq!(s.parts.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(s.parts.iter().flat_map(|p| p.1.clone()).collect::<Vec<_>>(), data, "fichier reconstitué");
        let form = &s.completed[0];
        assert_eq!(form["key"], "rZ2h9ZqVQi");
        assert_eq!(form["uploadId"], "UP1");
        assert_eq!(form["name"], "Film de test.mkv");
        assert_eq!(form["user"], "hash_compte");
        assert_eq!(form["parts[2][PartNumber]"], "3");
        assert_eq!(form["parts[2][ETag]"], "\"etag-3\"");
        drop(s);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn deletion_needs_the_account() {
        let (base, state) = fake_server(4000).await;
        let http = reqwest::Client::new();
        let anonymous = Vikingfile::for_tests(&base, None);
        assert!(!anonymous.can_delete());
        assert!(anonymous.delete(&http, "file", "TPRSfLvcIu", "").await.is_err());
        let refused = Vikingfile::for_tests(&base, Some("autre")).delete(&http, "file", "TPRSfLvcIu", "").await;
        assert!(format!("{:#}", refused.unwrap_err()).contains("wrong user"));
        let viking = Vikingfile::for_tests(&base, Some("hash_compte"));
        assert!(viking.can_delete());
        viking.delete(&http, "file", "TPRSfLvcIu", "").await.unwrap();
        assert!(viking.delete(&http, "file", "../x", "").await.is_err());
        assert_eq!(state.lock().unwrap().deleted, vec![("TPRSfLvcIu".into(), "hash_compte".into())]);
    }
}
