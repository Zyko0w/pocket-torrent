//! Hébergeur de fichiers ajouté par l'utilisateur (Réglages › Envoi en ligne), sur le modèle des
//! fichiers .sxcu de ShareX : une adresse d'envoi, un formulaire multipart (ou le fichier brut
//! en PUT), des en-têtes facultatifs (clés d'API, rangées dans le Gestionnaire d'identification
//! de Windows) et l'endroit où lire le lien dans la réponse.
//!
//! Un lien par fichier. Pas de suppression depuis Pocket Torrent : chaque service a sa propre
//! façon de faire, souvent aucune pour les envois anonymes.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::{Context, bail};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use super::http::{Multipart, check_status, file_size, file_stream, send, share_url, with_progress};
use super::{FileHost, Hosted, Uploaded};
use crate::secrets::{self, Secret};
use crate::settings::{CustomHost, CustomMethod};

/// Réponse lue au plus (un lien tient en quelques centaines d'octets).
const MAX_REPLY: usize = 1024 * 1024;

pub struct Custom {
    config: CustomHost,
    headers: HeaderMap,
}

impl Custom {
    pub fn open(config: &CustomHost) -> anyhow::Result<Self> {
        let text = secrets::get(Secret::CustomHostHeaders(config.id.clone())).unwrap_or_default();
        Ok(Self { config: config.clone(), headers: parse_headers(&text)? })
    }

    #[cfg(test)]
    fn for_tests(config: CustomHost, headers: &str) -> Self {
        Self { config, headers: parse_headers(headers).unwrap() }
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    fn url(&self, file_name: &str) -> anyhow::Result<reqwest::Url> {
        let url = self.config.url.replace("{filename}", &encode_path(file_name));
        let parsed = reqwest::Url::parse(&url).context("adresse d'envoi invalide")?;
        if parsed.scheme() != "https" && !cfg!(test) {
            bail!("adresse d'envoi non sécurisée refusée");
        }
        Ok(parsed)
    }
}

impl FileHost for Custom {
    /// Pas d'envoi d'essai (il publierait un fichier) : l'adresse répond, avec les en-têtes.
    async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        let request = client.get(self.url("test.txt")?).headers(self.headers.clone());
        let response = send(request, &self.config.name).await?;
        Ok(format!(
            "{} joignable (réponse HTTP {}). Fais un petit envoi pour vérifier le lien renvoyé.",
            self.config.name,
            response.status().as_u16()
        ))
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
        let name = &self.config.name;
        let size = file_size(path).await?;
        let sent = Arc::new(AtomicU64::new(0));
        let file = tokio::fs::File::open(path).await?;
        let request = match self.config.method {
            CustomMethod::Multipart => {
                let fields: Vec<(&str, &str)> =
                    self.config.form_fields.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
                let form = Multipart::new(&fields, &self.config.file_field, file_name, size);
                client
                    .post(self.url(file_name)?)
                    .headers(self.headers.clone())
                    .header(CONTENT_TYPE, form.content_type())
                    .header(CONTENT_LENGTH, form.length())
                    .body(form.body(file, sent.clone()))
            }
            CustomMethod::Put => {
                let mut request = client.put(self.url(file_name)?).headers(self.headers.clone());
                if !self.headers.contains_key(CONTENT_TYPE) {
                    request = request.header(CONTENT_TYPE, "application/octet-stream");
                }
                request.header(CONTENT_LENGTH, size).body(reqwest::Body::wrap_stream(file_stream(
                    file,
                    size,
                    sent.clone(),
                )))
            }
        };
        let reply = with_progress(
            async {
                let mut response = check_status(send(request, name).await?, name, "envoi du fichier").await?;
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    body.extend_from_slice(&chunk);
                    if body.len() > MAX_REPLY {
                        bail!("réponse de {name} trop longue pour contenir un lien");
                    }
                }
                Ok(String::from_utf8_lossy(&body).into_owned())
            },
            &sent,
            on_progress,
        )
        .await?;
        on_progress(size);
        let url = extract_link(&reply, &self.config.link_path, &self.config.link_template)
            .with_context(|| format!("lien introuvable dans la réponse de {name}"))?;
        let file = Hosted { id: uuid::Uuid::new_v4().simple().to_string(), url, owner_token: String::new() };
        Ok(Uploaded { group: None, file: Some(file) })
    }

    async fn delete(
        &self,
        _client: &reqwest::Client,
        _kind: &str,
        _id: &str,
        _owner_token: &str,
    ) -> anyhow::Result<()> {
        bail!("suppression non prise en charge pour un hébergeur personnalisé")
    }

    fn can_delete(&self) -> bool {
        false
    }
}

/// En-têtes « Nom: valeur », un par ligne. Ceux qui décrivent la requête elle-même sont refusés.
pub fn parse_headers(text: &str) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').context("en-tête sans « : » (format : Nom: valeur)")?;
        let name = HeaderName::from_bytes(name.trim().as_bytes()).context("nom d'en-tête invalide")?;
        if matches!(name.as_str(), "host" | "content-length" | "transfer-encoding" | "connection") {
            bail!("en-tête « {name} » réservé");
        }
        let mut value = HeaderValue::from_str(value.trim()).context("valeur d'en-tête invalide")?;
        value.set_sensitive(true);
        headers.append(name, value);
    }
    if headers.len() > 20 {
        bail!("20 en-têtes au plus");
    }
    Ok(headers)
}

/// Lien lu dans la réponse : la réponse entière (texte), ou la valeur au bout de `path` dans le
/// JSON (« data.url », « files.0.url »), mise dans `template` (« {value} ») s'il y en a un.
pub fn extract_link(reply: &str, path: &str, template: &str) -> anyhow::Result<String> {
    let excerpt = || reply.trim().chars().take(200).collect::<String>();
    let value = if path.is_empty() {
        reply.trim().lines().next().unwrap_or_default().trim().to_owned()
    } else {
        let json: Value = serde_json::from_str(reply)
            .with_context(|| format!("réponse qui n'est pas du JSON (« {} »)", excerpt()))?;
        let mut current = &json;
        for key in path.split('.') {
            current = match current {
                Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
                Value::Object(map) => map.get(key),
                _ => None,
            }
            .with_context(|| format!("« {path} » absent de la réponse (« {} »)", excerpt()))?;
        }
        match current {
            Value::String(text) => text.trim().to_owned(),
            Value::Number(number) => number.to_string(),
            _ => bail!("« {path} » n'est pas un texte dans la réponse"),
        }
    };
    if value.is_empty() {
        bail!("réponse vide");
    }
    let link = if template.is_empty() { value } else { template.replace("{value}", &value) };
    share_url(&link).with_context(|| format!("lien reçu : « {} »", link.chars().take(200).collect::<String>()))
}

/// Nom de fichier mis dans une adresse (« {filename} »).
fn encode_path(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Tests dans un bac à sable : un faux hébergeur local (aucune requête vers un vrai).
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::extract::{Path as UrlPath, State};
    use axum::http::HeaderMap as AxumHeaders;
    use axum::routing::{post, put};
    use axum::{Json, Router};
    use serde_json::json;

    use super::super::http::sandbox_file;
    use super::*;
    use crate::settings::FormField;

    #[derive(Default)]
    struct Fake {
        bodies: Vec<(String, Vec<u8>, Option<String>)>,
    }

    type FakeState = Arc<Mutex<Fake>>;

    async fn fake_server() -> (String, FakeState) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state: FakeState = Arc::default();
        let key = |headers: &AxumHeaders| headers.get("x-api-key").map(|h| h.to_str().unwrap().to_owned());
        let app = Router::new()
            .route(
                "/texte",
                post(move |State(s): State<FakeState>, headers: AxumHeaders, body: axum::body::Bytes| async move {
                    s.lock().unwrap().bodies.push(("texte".into(), body.to_vec(), key(&headers)));
                    "https://files.example/abc.mkv\n"
                }),
            )
            .route(
                "/json",
                post(move |State(s): State<FakeState>, headers: AxumHeaders, body: axum::body::Bytes| async move {
                    s.lock().unwrap().bodies.push(("json".into(), body.to_vec(), key(&headers)));
                    Json(json!({ "success": true, "files": [{ "id": "Xy12" }] }))
                }),
            )
            .route(
                "/put/{name}",
                put(
                    move |State(s): State<FakeState>,
                          UrlPath(name): UrlPath<String>,
                          headers: AxumHeaders,
                          body: axum::body::Bytes| async move {
                        s.lock().unwrap().bodies.push((name, body.to_vec(), key(&headers)));
                        "https://files.example/put"
                    },
                ),
            )
            .route(
                "/refus",
                post(|| async { (axum::http::StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad key" }))) }),
            )
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, state)
    }

    fn config(url: String) -> CustomHost {
        CustomHost {
            id: "h1".into(),
            name: "Mon hébergeur".into(),
            url,
            method: CustomMethod::Multipart,
            file_field: "fileToUpload".into(),
            form_fields: vec![FormField { name: "reqtype".into(), value: "fileupload".into() }],
            ..Default::default()
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[tokio::test]
    async fn multipart_form_with_text_reply() {
        let (base, state) = fake_server().await;
        let host = Custom::for_tests(config(format!("{base}/texte")), "X-Api-Key: secret123");
        let (path, data) = sandbox_file(300 * 1024);
        let mut progress = 0;
        let uploaded =
            host.upload(&reqwest::Client::new(), &path, "Film.mkv", 1, None, &mut |p| progress = p).await.unwrap();
        assert_eq!(uploaded.file.unwrap().url, "https://files.example/abc.mkv");
        assert!(uploaded.group.is_none());
        assert_eq!(progress, data.len() as u64);
        let s = state.lock().unwrap();
        let (_, body, key) = &s.bodies[0];
        assert_eq!(key.as_deref(), Some("secret123"));
        assert!(find(body, b"name=\"reqtype\"\r\n\r\nfileupload\r\n"));
        assert!(find(body, b"name=\"fileToUpload\"; filename=\"Film.mkv\""));
        assert!(find(body, &data), "fichier transmis à l'identique");
        drop(s);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn json_reply_goes_through_path_and_template() {
        let (base, _) = fake_server().await;
        let mut settings = config(format!("{base}/json"));
        settings.link_path = "files.0.id".into();
        settings.link_template = "https://files.example/f/{value}".into();
        let host = Custom::for_tests(settings, "");
        let (path, _) = sandbox_file(10);
        let uploaded = host.upload(&reqwest::Client::new(), &path, "a.mkv", 1, None, &mut |_| {}).await.unwrap();
        assert_eq!(uploaded.file.unwrap().url, "https://files.example/f/Xy12");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn raw_put_puts_the_file_name_in_the_address() {
        let (base, state) = fake_server().await;
        let mut settings = config(format!("{base}/put/{{filename}}"));
        settings.method = CustomMethod::Put;
        let host = Custom::for_tests(settings, "");
        let (path, data) = sandbox_file(1000);
        host.upload(&reqwest::Client::new(), &path, "Mon film.mkv", 1, None, &mut |_| {}).await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.bodies[0].0, "Mon film.mkv");
        assert_eq!(s.bodies[0].1, data, "fichier brut, sans formulaire");
        drop(s);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn refusal_is_reported_and_nothing_can_be_deleted() {
        let (base, _) = fake_server().await;
        let host = Custom::for_tests(config(format!("{base}/refus")), "");
        let (path, _) = sandbox_file(10);
        let err = host.upload(&reqwest::Client::new(), &path, "a.mkv", 1, None, &mut |_| {}).await.unwrap_err();
        assert!(format!("{err:#}").contains("bad key"), "{err:#}");
        assert!(!host.can_delete());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn links_are_read_safely() {
        assert_eq!(extract_link(" https://a.example/x \n", "", "").unwrap(), "https://a.example/x");
        assert_eq!(
            extract_link(r#"{"data":{"url":"https://a.example/y"}}"#, "data.url", "").unwrap(),
            "https://a.example/y"
        );
        assert_eq!(extract_link(r#"{"id":42}"#, "id", "https://a.example/{value}").unwrap(), "https://a.example/42");
        assert!(extract_link("http://a.example/x", "", "").is_err(), "lien non sécurisé");
        assert!(extract_link("<html>erreur</html>", "", "").is_err());
        assert!(extract_link(r#"{"data":{}}"#, "data.url", "").is_err());
        assert!(extract_link("", "", "").is_err());
    }

    #[test]
    fn headers_are_checked() {
        let headers = parse_headers("Authorization: Bearer abc\n\nX-Key:  v ").unwrap();
        assert_eq!(headers.len(), 2);
        assert!(headers["authorization"].is_sensitive());
        assert!(parse_headers("sans deux points").is_err());
        assert!(parse_headers("Host: evil.example").is_err());
        assert!(parse_headers("Content-Length: 1").is_err());
        assert!(parse_headers("Mauvais nom: x").is_err());
    }
}
