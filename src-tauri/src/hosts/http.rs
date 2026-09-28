//! Aides communes aux services d'hébergement : lecture du fichier au fil de l'envoi,
//! progression, contrôle des réponses et des adresses reçues.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use bytes::Bytes;
use futures::Stream;
use tokio::io::AsyncReadExt;

/// Morceaux lus sur le disque pendant un envoi : le fichier n'est jamais chargé en entier.
const CHUNK: u64 = 256 * 1024;

/// Lit `len` octets du fichier (depuis sa position actuelle) ; `sent` compte les octets partis.
pub fn file_stream(
    file: tokio::fs::File,
    len: u64,
    sent: Arc<AtomicU64>,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures::stream::try_unfold((file, len), move |(mut file, left)| {
        let sent = sent.clone();
        async move {
            if left == 0 {
                return Ok(None);
            }
            let mut buffer = vec![0u8; left.min(CHUNK) as usize];
            file.read_exact(&mut buffer).await?;
            sent.fetch_add(buffer.len() as u64, Ordering::Relaxed);
            let left = left - buffer.len() as u64;
            Ok(Some((Bytes::from(buffer), (file, left))))
        }
    })
}

/// Formulaire multipart lu au fil de l'eau : les champs, puis le fichier.
pub struct Multipart {
    boundary: String,
    head: String,
    tail: String,
    size: u64,
}

impl Multipart {
    /// Noms de champs et de fichier sans guillemets ni retours à la ligne (vérifiés avant, ou
    /// remplacés ici pour le nom du fichier).
    pub fn new(fields: &[(&str, &str)], file_field: &str, file_name: &str, size: u64) -> Self {
        let boundary = format!("pocket-torrent-{}", uuid::Uuid::new_v4().simple());
        let file_name = clean_name(file_name).replace('"', "%22");
        let mut head = String::new();
        for (name, value) in fields {
            head += &format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n");
        }
        head += &format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{file_name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        Self { boundary, head, tail, size }
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// Taille totale, annoncée d'avance (certains serveurs refusent un envoi sans elle).
    pub fn length(&self) -> u64 {
        self.head.len() as u64 + self.size + self.tail.len() as u64
    }

    pub fn body(self, file: tokio::fs::File, sent: Arc<AtomicU64>) -> reqwest::Body {
        use futures::StreamExt;
        let Self { head, tail, size, .. } = self;
        let stream = futures::stream::once(async move { Ok(Bytes::from(head)) })
            .chain(file_stream(file, size, sent))
            .chain(futures::stream::once(async move { Ok(Bytes::from(tail)) }));
        reqwest::Body::wrap_stream(stream)
    }
}

/// Attend la requête en donnant la progression (`sent`) toutes les demi-secondes.
pub async fn with_progress<T>(
    future: impl Future<Output = anyhow::Result<T>>,
    sent: &AtomicU64,
    on_progress: &mut (dyn FnMut(u64) + Send),
) -> anyhow::Result<T> {
    tokio::pin!(future);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = tick.tick() => on_progress(sent.load(Ordering::Relaxed)),
        }
    }
}

/// Taille du fichier à envoyer ; un fichier vide est refusé.
pub async fn file_size(path: &Path) -> anyhow::Result<u64> {
    let size =
        tokio::fs::metadata(path).await.with_context(|| format!("fichier introuvable : {}", path.display()))?.len();
    if size == 0 {
        bail!("fichier vide : rien à envoyer");
    }
    Ok(size)
}

pub fn clean_name(file_name: &str) -> String {
    file_name.chars().filter(|c| !c.is_control()).take(255).collect()
}

/// Identifiant de fichier ou de dossier renvoyé par un service : lettres, chiffres et tirets
/// (il est mis dans une requête).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Lien de partage renvoyé par un service : HTTPS seulement (il est copié et ouvert tel quel).
pub fn share_url(url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(url).context("lien de partage invalide dans la réponse")?;
    if parsed.scheme() != "https" {
        bail!("lien de partage non sécurisé refusé");
    }
    Ok(parsed.into())
}

/// Adresse d'envoi signée : HTTPS obligatoire (hors faux serveurs des tests).
pub fn signed_url(url: &str) -> anyhow::Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).context("adresse d'envoi invalide")?;
    if parsed.scheme() != "https" && !cfg!(test) {
        bail!("adresse d'envoi non sécurisée refusée");
    }
    Ok(parsed)
}

/// L'erreur ne contient jamais l'adresse (signée, donc secrète le temps de sa validité).
pub async fn send(request: reqwest::RequestBuilder, service: &str) -> anyhow::Result<reqwest::Response> {
    request.send().await.map_err(|err| anyhow::Error::from(err.without_url()).context(format!("{service} injoignable")))
}

/// Réponse en erreur : message du service (tronqué) et piste selon le code HTTP.
pub async fn check_status(response: reqwest::Response, service: &str, step: &str) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let reason = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| {
        ["error", "message", "status"]
            .iter()
            .find_map(|k| v.get(k).and_then(|e| e.as_str()).map(|e| e.chars().take(200).collect::<String>()))
    });
    let hint = match status.as_u16() {
        401 | 403 => " — jeton ou identifiant de compte refusé",
        429 => " — trop de requêtes, réessaie plus tard",
        _ => "",
    };
    bail!(
        "{service} a refusé l'étape « {step} » (HTTP {status}{}){hint}",
        reason.map(|r| format!(" : {r}")).unwrap_or_default()
    )
}

/// Fichier de test dans un dossier temporaire à part.
#[cfg(test)]
pub fn sandbox_file(size: usize) -> (std::path::PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let path = dir.join("Film de test.mkv");
    std::fs::write(&path, &data).unwrap();
    (path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_links_must_be_https() {
        assert!(share_url("https://gofile.io/d/AbC").is_ok());
        assert!(share_url("http://gofile.io/d/AbC").is_err());
        assert!(share_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn ids_cannot_escape_the_request() {
        assert!(valid_id("4f1c2d3e-aaaa-bbbb-cccc-0123456789ab"));
        for id in ["../accounts", "a/b", "x?y=1", "a b", ""] {
            assert!(!valid_id(id), "{id}");
        }
    }

    #[tokio::test]
    async fn file_is_read_in_chunks_and_counted() {
        use futures::StreamExt;
        let (path, data) = sandbox_file(600 * 1024);
        let sent = Arc::new(AtomicU64::new(0));
        let file = tokio::fs::File::open(&path).await.unwrap();
        let chunks: Vec<Bytes> = file_stream(file, data.len() as u64, sent.clone()).map(Result::unwrap).collect().await;
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.concat(), data);
        assert_eq!(sent.load(Ordering::Relaxed), data.len() as u64);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
