//! Envoi des fichiers terminés sur storage.to (https://storage.to/docs/api) et liens de partage.
//!
//! Déroulé : `upload/init` donne une ou plusieurs adresses d'envoi signées (Cloudflare R2 côté
//! storage.to), les octets y partent directement, puis `upload/confirm` crée le fichier et renvoie
//! son lien. Plusieurs fichiers (torrent à plusieurs fichiers) sont rangés dans une collection :
//! un seul lien pour le tout. Sans compte, les fichiers expirent (1 à 7 jours) ; un jeton de compte
//! gratuit lève la limite de 50 fichiers par jour.
//!
//! Chaque création renvoie un « jeton de propriétaire » : c'est lui qui permet ensuite de supprimer
//! le fichier, quels que soient le réseau ou le jeton de visiteur. Il est gardé avec le lien.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::secrets::{self, Secret};
use crate::settings::StorageToSettings;

/// Adresse de l'API : seule destination des requêtes (hors adresses d'envoi signées qu'elle fournit).
pub const API: &str = "https://storage.to/api";
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024 * 1024;

pub struct StorageTo {
    base: String,
    visitor_token: String,
    account_token: Option<String>,
    expiry_days: u8,
}

/// Fichier (ou collection) créé : lien de partage et preuve de propriété.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shared {
    pub id: String,
    pub url: String,
    pub owner_token: String,
    pub expires_at: Option<String>,
}

#[derive(Deserialize)]
struct Init {
    #[serde(rename = "type")]
    kind: String,
    upload_url: Option<String>,
    upload_id: Option<String>,
    r2_key: String,
    part_size: Option<u64>,
    total_parts: Option<u32>,
    #[serde(default)]
    initial_urls: BTreeMap<String, String>,
    owner_token: Option<String>,
}

#[derive(Deserialize)]
struct PartUrl {
    #[serde(rename = "partNumber")]
    part_number: u32,
    url: String,
}

#[derive(Deserialize)]
struct PartUrls {
    part_urls: Vec<PartUrl>,
}

#[derive(Deserialize)]
struct Resource {
    id: String,
    url: String,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct Confirmed {
    file: Resource,
    owner_token: String,
}

#[derive(Deserialize)]
struct CollectionCreated {
    collection: Resource,
    owner_token: String,
}

#[derive(Deserialize)]
struct Quota {
    authenticated: bool,
    remaining_gb: Option<f64>,
    plan: Option<String>,
}

impl StorageTo {
    /// Jeton de visiteur tiré une fois (aléa du système) puis gardé dans le Gestionnaire
    /// d'identification : il identifie les envois anonymes de ce PC.
    pub fn from_settings(settings: &StorageToSettings) -> anyhow::Result<Self> {
        let visitor_token = match secrets::get(Secret::StorageToVisitor) {
            Some(token) => token,
            None => {
                let token = format!("pocket-torrent-{}", uuid::Uuid::new_v4().simple());
                secrets::set(Secret::StorageToVisitor, &token)?;
                token
            }
        };
        Ok(Self {
            base: API.to_owned(),
            visitor_token,
            account_token: secrets::get(Secret::StorageToToken),
            expiry_days: settings.expiry_days.clamp(1, 7),
        })
    }

    #[cfg(test)]
    fn for_tests(base: &str) -> Self {
        Self { base: base.to_owned(), visitor_token: "visiteur".into(), account_token: None, expiry_days: 2 }
    }

    fn request(&self, client: &reqwest::Client, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = client.request(method, format!("{}{path}", self.base)).header("X-Visitor-Token", &self.visitor_token);
        if let Some(token) = &self.account_token {
            request = request.bearer_auth(token);
        }
        request
    }

    /// Requête d'un propriétaire : preuve par le jeton reçu à la création.
    fn owned(&self, request: reqwest::RequestBuilder, owner_token: &str) -> reqwest::RequestBuilder {
        if self.account_token.is_some() {
            request.header("X-Owner-Token", owner_token)
        } else {
            request.header(reqwest::header::AUTHORIZATION, format!("Owner {owner_token}"))
        }
    }

    /// Vérifie l'accès et renvoie une phrase sur le quota restant (bouton « Tester »).
    pub async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        let quota: Quota = api_json(self.request(client, reqwest::Method::GET, "/bandwidth/status"), "état du quota").await?;
        Ok(match (quota.authenticated, quota.remaining_gb, quota.plan) {
            (true, _, plan) => format!("Compte storage.to relié ({}).", plan.unwrap_or_else(|| "gratuit".into())),
            (false, Some(gb), _) => format!("storage.to joignable : {gb:.1} Go d'envoi restants sur 24 h (sans compte)."),
            (false, None, _) => "storage.to joignable (sans compte).".into(),
        })
    }

    /// Collection pour regrouper les fichiers d'un même téléchargement sous un seul lien.
    pub async fn create_collection(&self, client: &reqwest::Client, file_count: usize) -> anyhow::Result<Shared> {
        let request = self
            .request(client, reqwest::Method::POST, "/collection")
            .json(&json!({ "expected_file_count": file_count }));
        let created: CollectionCreated = api_json(request, "création de la collection").await?;
        let collection = created.collection;
        let shared =
            Shared { id: collection.id, url: collection.url, owner_token: created.owner_token, expires_at: collection.expires_at };
        self.set_expiry(client, "collection", &shared).await?;
        Ok(shared)
    }

    /// Envoie un fichier ; `on_progress` reçoit le nombre d'octets déjà envoyés.
    pub async fn upload(
        &self,
        client: &reqwest::Client,
        path: &Path,
        file_name: &str,
        collection: Option<&Shared>,
        mut on_progress: impl FnMut(u64),
    ) -> anyhow::Result<Shared> {
        let size = tokio::fs::metadata(path).await.with_context(|| format!("fichier introuvable : {}", path.display()))?.len();
        if size == 0 {
            bail!("fichier vide : rien à envoyer");
        }
        if size > MAX_FILE_BYTES {
            bail!("fichier de plus de 25 Go : trop gros pour storage.to");
        }
        let file_name: String = file_name.chars().filter(|c| !c.is_control()).take(255).collect();
        let content_type = content_type(&file_name);
        let request = self
            .request(client, reqwest::Method::POST, "/upload/init")
            .json(&json!({ "filename": file_name, "content_type": content_type, "size": size }));
        let init: Init = api_json(request, "début de l'envoi").await?;
        let mut file = tokio::fs::File::open(path).await?;

        match init.kind.as_str() {
            "single" => {
                let url = signed_url(init.upload_url.as_deref())?;
                let mut body = Vec::with_capacity(size as usize);
                file.read_to_end(&mut body).await?;
                put_with_retry(client, &url, bytes::Bytes::from(body)).await?;
                on_progress(size);
            }
            "multipart" => self.upload_parts(client, &init, &mut file, size, &mut on_progress).await?,
            other => bail!("réponse inattendue de storage.to (envoi « {other} »)"),
        }

        let mut body = json!({ "filename": file_name, "size": size, "content_type": content_type, "r2_key": init.r2_key });
        if let Some(collection) = collection {
            body["collection_id"] = json!(collection.id);
        }
        let request = self.request(client, reqwest::Method::POST, "/upload/confirm").json(&body);
        let confirmed: Confirmed = api_json(request, "fin de l'envoi").await?;
        let file = confirmed.file;
        let shared = Shared { id: file.id, url: file.url, owner_token: confirmed.owner_token, expires_at: file.expires_at };
        self.set_expiry(client, "file", &shared).await?;
        Ok(shared)
    }

    async fn upload_parts(
        &self,
        client: &reqwest::Client,
        init: &Init,
        file: &mut tokio::fs::File,
        size: u64,
        on_progress: &mut impl FnMut(u64),
    ) -> anyhow::Result<()> {
        let upload_id = init.upload_id.clone().context("réponse de storage.to sans identifiant d'envoi")?;
        let owner_token = init.owner_token.clone().context("réponse de storage.to sans jeton de propriétaire")?;
        let part_size = init.part_size.filter(|&s| s >= 5 * 1024 * 1024).context("taille des parties manquante")?;
        let total_parts = init.total_parts.unwrap_or_else(|| size.div_ceil(part_size) as u32);
        if u64::from(total_parts) != size.div_ceil(part_size) {
            bail!("découpage de l'envoi incohérent");
        }
        let mut urls: BTreeMap<u32, String> =
            init.initial_urls.iter().filter_map(|(n, url)| Some((n.parse().ok()?, url.clone()))).collect();

        let result = async {
            let mut etags = Vec::new();
            let mut sent = 0u64;
            for part in 1..=total_parts {
                if !urls.contains_key(&part) {
                    // adresses des parties suivantes, par lots
                    let wanted: Vec<u32> = (part..=total_parts.min(part + 49)).collect();
                    let request = self
                        .owned(self.request(client, reqwest::Method::POST, "/upload/parts"), &owner_token)
                        .json(&json!({ "upload_id": upload_id, "part_numbers": wanted }));
                    let more: PartUrls = api_json(request, "adresses des parties").await?;
                    urls.extend(more.part_urls.into_iter().map(|p| (p.part_number, p.url)));
                }
                let url = signed_url(urls.get(&part).map(String::as_str))?;
                let len = part_size.min(size - sent) as usize;
                let mut buffer = vec![0u8; len];
                file.read_exact(&mut buffer).await?;
                let response = put_with_retry(client, &url, bytes::Bytes::from(buffer)).await?;
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow!("storage.to n'a pas confirmé la partie {part}"))?
                    .to_owned();
                etags.push(json!({ "partNumber": part, "etag": etag }));
                sent += len as u64;
                on_progress(sent);
            }
            let request = self
                .owned(self.request(client, reqwest::Method::POST, "/upload/complete-multipart"), &owner_token)
                .json(&json!({ "upload_id": upload_id, "parts": etags }));
            api_json::<serde_json::Value>(request, "assemblage des parties").await?;
            anyhow::Ok(())
        }
        .await;

        if result.is_err() {
            let abort = self
                .owned(self.request(client, reqwest::Method::POST, "/upload/abort"), &owner_token)
                .json(&json!({ "upload_id": upload_id }));
            let _ = abort.send().await;
        }
        result
    }

    /// Durée de vie choisie dans les réglages (3 jours par défaut chez storage.to).
    async fn set_expiry(&self, client: &reqwest::Client, kind: &str, shared: &Shared) -> anyhow::Result<()> {
        // identifiant renvoyé par le serveur, mis dans l'adresse de la requête suivante
        if !valid_id(&shared.id) {
            bail!("identifiant storage.to invalide dans la réponse");
        }
        if self.expiry_days == 3 {
            return Ok(());
        }
        let request = self
            .owned(self.request(client, reqwest::Method::POST, &format!("/{kind}/{}/expiry", shared.id)), &shared.owner_token)
            .json(&json!({ "days": self.expiry_days }));
        api_json::<serde_json::Value>(request, "durée de vie du lien").await?;
        Ok(())
    }

    /// Supprime un fichier ou une collection (`kind` : « file » ou « collection »). Déjà absent : succès.
    pub async fn delete(&self, client: &reqwest::Client, kind: &str, id: &str, owner_token: &str) -> anyhow::Result<()> {
        if !valid_id(id) || !matches!(kind, "file" | "collection") {
            bail!("identifiant storage.to invalide");
        }
        let request = self.owned(self.request(client, reqwest::Method::DELETE, &format!("/{kind}/{id}")), owner_token);
        let response = send(request, "storage.to injoignable").await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(()); // expiré ou déjà supprimé
        }
        check_status(response, "suppression").await.map(|_| ())
    }
}

/// Identifiant de fichier ou de collection storage.to : lettres et chiffres seulement (il est mis
/// dans l'adresse de la requête).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Adresse d'envoi signée fournie par storage.to : HTTPS obligatoire.
fn signed_url(url: Option<&str>) -> anyhow::Result<reqwest::Url> {
    let url = url.context("réponse de storage.to sans adresse d'envoi")?;
    let parsed = reqwest::Url::parse(url).context("adresse d'envoi invalide")?;
    if parsed.scheme() != "https" && !cfg!(test) {
        bail!("adresse d'envoi non sécurisée refusée");
    }
    Ok(parsed)
}

fn content_type(file_name: &str) -> &'static str {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mkv" => "video/x-matroska",
        "mp4" | "m4v" => "video/mp4",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "srt" | "txt" | "nfo" => "text/plain",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// L'erreur d'envoi ne contient jamais l'adresse (signée, donc secrète le temps de sa validité).
async fn send(request: reqwest::RequestBuilder, context: &str) -> anyhow::Result<reqwest::Response> {
    request.send().await.map_err(|err| anyhow::Error::from(err.without_url()).context(context.to_owned()))
}

async fn check_status(response: reqwest::Response, step: &str) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let reason = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|e| e.chars().take(200).collect::<String>()));
    let hint = match status.as_u16() {
        429 => " — limite atteinte (50 fichiers ou 100 Go par 24 h sans compte), réessaie plus tard",
        403 => " — ce fichier n'appartient pas à ce PC",
        _ => "",
    };
    bail!("storage.to a refusé l'étape « {step} » (HTTP {status}{}){hint}", reason.map(|r| format!(" : {r}")).unwrap_or_default())
}

async fn api_json<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder, step: &str) -> anyhow::Result<T> {
    let response = check_status(send(request, "storage.to injoignable").await?, step).await?;
    response.json::<T>().await.with_context(|| format!("réponse de storage.to illisible ({step})"))
}

async fn put_with_retry(client: &reqwest::Client, url: &reqwest::Url, body: bytes::Bytes) -> anyhow::Result<reqwest::Response> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let result = match send(client.put(url.clone()).body(body.clone()), "storage.to injoignable").await {
            Ok(response) => check_status(response, "envoi des données").await,
            Err(err) => Err(err),
        };
        match result {
            Ok(response) => return Ok(response),
            Err(err) if attempt >= 4 => return Err(err),
            Err(err) => {
                tracing::warn!(attempt, "partie à renvoyer : {err:#}");
                tokio::time::sleep(Duration::from_secs(2 * attempt)).await;
            }
        }
    }
}

/// Tests dans un bac à sable : un faux serveur storage.to local (aucune requête vers le vrai).
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path as UrlPath, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{delete, post, put};
    use axum::{Json, Router};

    use super::*;

    #[derive(Default)]
    struct Fake {
        received: Vec<u8>,
        parts: Vec<(u32, usize)>,
        confirmed: Vec<serde_json::Value>,
        expiry: Vec<(String, u64)>,
        deleted: Vec<String>,
        owner_headers: Vec<String>,
    }

    type FakeState = Arc<Mutex<Fake>>;

    async fn fake_server(multipart: bool) -> (String, FakeState) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state: FakeState = Arc::default();
        let b = base.clone();
        let app = Router::new()
            .route(
                "/api/upload/init",
                post(move |Json(body): Json<serde_json::Value>| {
                    let b = b.clone();
                    async move {
                        assert!(body["size"].as_u64().unwrap() > 0);
                        if multipart {
                            Json(json!({ "success": true, "type": "multipart", "upload_id": "U1", "r2_key": "k1",
                                "part_size": 5 * 1024 * 1024, "total_parts": 3,
                                "initial_urls": { "1": format!("{b}/r2/1") }, "owner_token": "owner_upload" }))
                        } else {
                            Json(json!({ "success": true, "type": "single", "upload_url": format!("{b}/r2/single"), "r2_key": "k1" }))
                        }
                    }
                }),
            )
            .route(
                "/api/upload/parts",
                post({
                    let b = base.clone();
                    move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                        let b = b.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Owner owner_upload");
                            let urls: Vec<_> = body["part_numbers"].as_array().unwrap().iter()
                                .map(|n| json!({ "partNumber": n, "url": format!("{b}/r2/{n}") })).collect();
                            Json(json!({ "success": true, "part_urls": urls }))
                        }
                    }
                }),
            )
            .route(
                "/r2/{part}",
                put(|State(s): State<FakeState>, UrlPath(part): UrlPath<String>, body: axum::body::Bytes| async move {
                    let mut s = s.lock().unwrap();
                    s.received.extend_from_slice(&body);
                    if let Ok(n) = part.parse() {
                        s.parts.push((n, body.len()));
                    }
                    ([("etag", format!("\"etag-{part}\""))], "")
                }),
            )
            .route(
                "/api/upload/complete-multipart",
                post(|Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(body["parts"].as_array().unwrap().len(), 3);
                    Json(json!({ "success": true }))
                }),
            )
            .route(
                "/api/upload/confirm",
                post(|State(s): State<FakeState>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(headers["x-visitor-token"], "visiteur");
                    s.lock().unwrap().confirmed.push(body);
                    Json(json!({ "success": true, "file": { "id": "FQxyz1234", "url": "https://storage.to/FQxyz1234",
                        "expires_at": "2026-10-01T12:00:00Z" }, "owner_token": "owner_file" }))
                }),
            )
            .route(
                "/api/collection",
                post(|| async {
                    Json(json!({ "success": true, "collection": { "id": "ABC123xyz", "url": "https://storage.to/c/ABC123xyz",
                        "expires_at": null }, "owner_token": "owner_coll" }))
                }),
            )
            .route(
                "/api/{kind}/{id}/expiry",
                post(|State(s): State<FakeState>, UrlPath((_kind, id)): UrlPath<(String, String)>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    let mut s = s.lock().unwrap();
                    s.owner_headers.push(headers["authorization"].to_str().unwrap().to_owned());
                    s.expiry.push((id, body["days"].as_u64().unwrap()));
                    Json(json!({ "success": true }))
                }),
            )
            .route(
                "/api/{kind}/{id}",
                delete(|State(s): State<FakeState>, UrlPath((_kind, id)): UrlPath<(String, String)>, headers: HeaderMap| async move {
                    if headers.get("authorization").map(|h| h.to_str().unwrap()) != Some("Owner owner_file") {
                        return (StatusCode::FORBIDDEN, Json(json!({ "success": false, "error": "Not authorized" })));
                    }
                    s.lock().unwrap().deleted.push(id);
                    (StatusCode::OK, Json(json!({ "success": true })))
                }),
            )
            // parties de 5 Mio : au-delà de la limite par défaut d'axum (2 Mio)
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("{base}/api"), state)
    }

    fn sandbox_file(size: usize) -> (std::path::PathBuf, Vec<u8>) {
        let dir = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let path = dir.join("Film de test.mkv");
        std::fs::write(&path, &data).unwrap();
        (path, data)
    }

    #[tokio::test]
    async fn small_file_is_sent_in_one_piece_and_confirmed() {
        let (base, state) = fake_server(false).await;
        let client = StorageTo::for_tests(&base);
        let (path, data) = sandbox_file(4096);
        let mut progress = 0;
        let shared = client.upload(&reqwest::Client::new(), &path, "Film de test.mkv", None, |p| progress = p).await.unwrap();
        assert_eq!(shared.url, "https://storage.to/FQxyz1234");
        assert_eq!(shared.owner_token, "owner_file");
        assert_eq!(progress, 4096);
        let s = state.lock().unwrap();
        assert_eq!(s.received, data);
        assert_eq!(s.confirmed[0]["content_type"], "video/x-matroska");
        // durée de vie réglée à 2 jours, avec la preuve de propriété du fichier
        assert_eq!(s.expiry, vec![("FQxyz1234".into(), 2)]);
        assert_eq!(s.owner_headers, vec!["Owner owner_file"]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn large_file_is_sent_in_parts_in_order() {
        let (base, state) = fake_server(true).await;
        let client = StorageTo::for_tests(&base);
        let (path, data) = sandbox_file(12 * 1024 * 1024);
        let collection = client.create_collection(&reqwest::Client::new(), 2).await.unwrap();
        client.upload(&reqwest::Client::new(), &path, "Film de test.mkv", Some(&collection), |_| {}).await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.received, data, "fichier reconstitué à l'identique");
        assert_eq!(s.parts.iter().map(|p| p.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(s.confirmed[0]["collection_id"], "ABC123xyz");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn deletion_needs_the_owner_token_and_a_plain_id() {
        let (base, state) = fake_server(false).await;
        let client = StorageTo::for_tests(&base);
        let http = reqwest::Client::new();
        client.delete(&http, "file", "FQxyz1234", "owner_file").await.unwrap();
        let refused = client.delete(&http, "file", "FQxyz1234", "mauvais").await.unwrap_err();
        assert!(format!("{refused:#}").contains("403"), "{refused:#}");
        // identifiant piégé : jamais mis dans l'adresse de la requête
        for id in ["../user", "a/b", "x?y=1", ""] {
            assert!(client.delete(&http, "file", id, "owner_file").await.is_err(), "{id}");
        }
        assert!(client.delete(&http, "../admin", "FQxyz1234", "owner_file").await.is_err());
        assert_eq!(state.lock().unwrap().deleted, vec!["FQxyz1234"]);
    }

    #[test]
    fn upload_addresses_must_be_https_outside_tests() {
        assert!(signed_url(None).is_err());
        assert!(signed_url(Some("pas une adresse")).is_err());
        assert!(signed_url(Some("https://r2.example/x?sig=1")).is_ok());
    }
}
