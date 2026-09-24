//! Téléchargements directs : liens http(s) classiques. Reprise après coupure ou pause
//! grâce aux requêtes partielles.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use librqbit::Session;
use parking_lot::Mutex;
use percent_encoding::percent_decode_str;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, RANGE};
use tokio::io::AsyncWriteExt;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

/// Client HTTP des téléchargements directs et des envois en ligne.
/// `local_ip` : adresse du VPN d'où partent les requêtes. `offline` : VPN exigé mais coupé,
/// toute requête échoue (proxy injoignable) au lieu de sortir sans protection.
/// Les noms sont résolus par DNS chiffré (`resolver`) et les certificats vérifiés avec les autorités
/// intégrées, sans vérification de révocation en ligne par Windows (qui sortirait hors du VPN).
pub fn http_client(
    local_ip: Option<std::net::IpAddr>,
    offline: bool,
    resolver: Arc<crate::dns::Resolver>,
) -> reqwest::Client {
    http_client_builder(local_ip, offline, resolver).build().expect("client HTTP")
}

fn http_client_builder(
    local_ip: Option<std::net::IpAddr>,
    offline: bool,
    resolver: Arc<crate::dns::Resolver>,
) -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .local_address(local_ip)
        .tls_certs_only(crate::dns::root_certificates().iter().cloned())
        .dns_resolver(Arc::new(crate::dns::ReqwestResolver(resolver)));
    if offline {
        builder = builder.proxy(reqwest::Proxy::all("socks5://127.0.0.1:9").expect("proxy local"));
    }
    builder
}

/// Réseau d'une liaison (libre, VPN ou coupé) : le client HTTP général et de quoi construire
/// celui des sources web, qui dépend aussi des réglages de confidentialité.
#[derive(Clone)]
pub struct Network {
    pub http: reqwest::Client,
    local_ip: Option<std::net::IpAddr>,
    offline: bool,
    resolver: Arc<crate::dns::Resolver>,
}

impl Network {
    pub fn new(local_ip: Option<std::net::IpAddr>, offline: bool, resolver: Arc<crate::dns::Resolver>) -> Self {
        Self { http: http_client(local_ip, offline, resolver.clone()), local_ip, offline, resolver }
    }

    /// Client des sources web (BEP 19) : même sortie (VPN), même DNS chiffré, mais adresses du
    /// réseau local retirées après résolution et redirections contrôlées ; HTTPS seul si
    /// `https_only` (mode discret).
    pub fn webseed_client(&self, https_only: bool) -> reqwest::Client {
        librqbit::protect_webseed_client(
            http_client_builder(self.local_ip, self.offline, self.resolver.clone()),
            Arc::new(crate::dns::ReqwestResolver(self.resolver.clone())),
            https_only,
        )
        .build()
        .expect("client HTTP des sources web")
    }
}

/// Octets reçus par tous les liens directs depuis le lancement (statistiques cumulées).
pub static RECEIVED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub fn is_http_url(link: &str) -> bool {
    url::Url::parse(link).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Nom de fichier sans caractères interdits par Windows.
pub fn safe_file_name(name: &str) -> String {
    // mêmes règles que les noms de fichiers des torrents (librqbit-core) : caractères interdits,
    // lecteur « C: », flux NTFS « a:b », noms réservés de Windows, points et espaces finaux
    let short: String = name.trim().chars().take(180).collect();
    let cleaned = librqbit_core::torrent_metainfo::safe_component(&short);
    if cleaned.trim_matches(['_', ' ', '.']).is_empty() { "fichier".into() } else { cleaned }
}

/// Chemin libre dans `dir` : « nom (1).ext » si « nom.ext » (ou son .part) existe déjà.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let path = Path::new(file_name);
    let stem = path.file_stem().map_or_else(|| file_name.to_owned(), |s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let taken = |p: &Path| p.exists() || part_path(p).exists();
    let mut candidate = dir.join(file_name);
    let mut i = 1;
    while taken(&candidate) {
        candidate = dir.join(format!("{stem} ({i}){ext}"));
        i += 1;
    }
    candidate
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Nom annoncé par le serveur (Content-Disposition), sinon tiré de l'URL.
fn file_name_from(response: &reqwest::Response) -> String {
    if let Some(header) = response.headers().get(CONTENT_DISPOSITION).and_then(|h| h.to_str().ok()) {
        for part in header.split(';').map(str::trim) {
            if let Some(value) = part.strip_prefix("filename*=") {
                let encoded = value.splitn(3, '\'').nth(2).unwrap_or(value).trim_matches('"');
                if let Ok(decoded) = percent_decode_str(encoded).decode_utf8() {
                    return decoded.into_owned();
                }
            }
        }
        for part in header.split(';').map(str::trim) {
            if let Some(value) = part.strip_prefix("filename=") {
                return value.trim_matches('"').to_owned();
            }
        }
    }
    response
        .url()
        .path_segments()
        .and_then(|mut segments| segments.next_back().map(str::to_owned))
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode_str(&s).decode_utf8_lossy().into_owned())
        .unwrap_or_else(|| "fichier".into())
}

// ------------------------------------------------------------ téléchargement

/// État partagé entre la tâche de téléchargement et le gestionnaire.
#[derive(Default)]
pub struct Progress {
    pub received: AtomicU64,
    pub total: AtomicU64,
    /// Limite propre à ce téléchargement en Ko/s, 0 = aucune.
    pub limit_kbps: AtomicU32,
    pub file: Mutex<FileState>,
}

#[derive(Clone, Default)]
pub struct FileState {
    pub name: Option<String>,
    /// Fichier « .part » en cours d'écriture.
    pub part_path: Option<PathBuf>,
    /// Vrai quand le lien est résolu et que les octets arrivent.
    pub started: bool,
}

pub enum DownloadError {
    /// Pas assez de place : le téléchargement est mis en pause.
    NoSpace(String),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for DownloadError {
    fn from(err: anyhow::Error) -> Self {
        DownloadError::Failed(err)
    }
}

/// Vérifie qu'il reste la place de télécharger `needed` octets dans `dir`.
pub type SpaceCheck = Arc<dyn Fn(&Path, u64) -> Result<(), String> + Send + Sync>;

pub struct Download {
    pub url: String,
    pub dir: PathBuf,
    pub progress: Arc<Progress>,
    pub session: Arc<Session>,
    pub check_space: SpaceCheck,
}

/// Télécharge dans `dir` puis renomme le « .part ». Renvoie le chemin final.
pub async fn run(client: &reqwest::Client, job: Download) -> Result<PathBuf, DownloadError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match fetch(client, &job, &job.url).await {
            Ok(path) => return Ok(path),
            Err(Attempt::Fatal(err)) => return Err(err),
            Err(Attempt::Retry { error }) => {
                if attempt >= 5 {
                    return Err(DownloadError::Failed(error.context("téléchargement impossible")));
                }
                tracing::warn!(attempt, "téléchargement direct interrompu : {error:#}");
                tokio::time::sleep(Duration::from_secs(2 * attempt)).await;
            }
        }
    }
}

enum Attempt {
    Fatal(DownloadError),
    Retry { error: anyhow::Error },
}

async fn fetch(client: &reqwest::Client, job: &Download, url: &str) -> Result<PathBuf, Attempt> {
    let progress = &job.progress;
    let existing_part = progress.file.lock().part_path.clone().filter(|p| p.exists());
    let mut received = match &existing_part {
        Some(part) => std::fs::metadata(part).map(|m| m.len()).unwrap_or(0),
        None => 0,
    };

    let mut request = client.get(url);
    if received > 0 {
        request = request.header(RANGE, format!("bytes={received}-"));
    }
    let retry = |error: anyhow::Error| Attempt::Retry { error };
    let mut response = request.send().await.map_err(|e| retry(anyhow::Error::from(e.without_url())))?;
    let status = response.status();
    if !status.is_success() {
        let error = anyhow!("le serveur a répondu HTTP {status}");
        return Err(if matches!(status.as_u16(), 401 | 404 | 410) {
            Attempt::Fatal(DownloadError::Failed(error))
        } else {
            retry(error)
        });
    }

    let resume = received > 0 && status == StatusCode::PARTIAL_CONTENT;
    if !resume {
        received = 0;
    }
    let length =
        response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if progress.total.load(Ordering::Relaxed) == 0
        && let Some(length) = length
    {
        progress.total.store(length + received, Ordering::Relaxed);
    }

    let part = match existing_part {
        Some(part) => part,
        None => {
            let name = progress.file.lock().name.clone().unwrap_or_else(|| safe_file_name(&file_name_from(&response)));
            std::fs::create_dir_all(&job.dir).map_err(|e| Attempt::Fatal(DownloadError::Failed(e.into())))?;
            let target = unique_path(&job.dir, &name);
            let part = part_path(&target);
            let mut file = progress.file.lock();
            file.name = target.file_name().map(|n| n.to_string_lossy().into_owned());
            file.part_path = Some(part.clone());
            part
        }
    };
    progress.file.lock().started = true;
    progress.received.store(received, Ordering::Relaxed);

    let total = progress.total.load(Ordering::Relaxed);
    if total > received {
        (job.check_space)(&job.dir, total - received).map_err(|msg| Attempt::Fatal(DownloadError::NoSpace(msg)))?;
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true);
    if resume {
        options.append(true)
    } else {
        options.write(true).truncate(true)
    };
    let mut file = options
        .open(&part)
        .await
        .with_context(|| format!("impossible d'écrire {}", part.display()))
        .map_err(|e| Attempt::Fatal(DownloadError::Failed(e)))?;

    let mut pace = Pace::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(err) => {
                let _ = file.flush().await;
                return Err(retry(anyhow::Error::from(err.without_url()).context("connexion coupée")));
            }
        };
        // limite globale (partagée avec les torrents) puis limite propre au téléchargement
        for piece in chunk.chunks(16 * 1024) {
            if let Some(len) = NonZeroU32::new(piece.len() as u32) {
                let _ = job.session.ratelimits.prepare_for_download(len).await;
            }
        }
        pace.wait(chunk.len() as u64, progress.limit_kbps.load(Ordering::Relaxed)).await;
        file.write_all(&chunk)
            .await
            .context("écriture sur le disque impossible")
            .map_err(|e| Attempt::Fatal(DownloadError::Failed(e)))?;
        received += chunk.len() as u64;
        progress.received.store(received, Ordering::Relaxed);
        RECEIVED_TOTAL.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush().await.map_err(|e| retry(e.into()))?;
    drop(file);

    let total = progress.total.load(Ordering::Relaxed);
    if total > 0 && received < total {
        return Err(retry(anyhow!("connexion coupée avant la fin")));
    }

    // « film.mkv.part » → « film.mkv »
    let part_name = part.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let final_name = part_name.strip_suffix(".part").unwrap_or(&part_name).to_owned();
    let mut final_path = part.with_file_name(&final_name);
    if final_path.exists() {
        final_path = unique_path(part.parent().unwrap_or(&job.dir), &final_name);
    }
    std::fs::rename(&part, &final_path)
        .with_context(|| format!("impossible de renommer {}", part.display()))
        .map_err(|e| Attempt::Fatal(DownloadError::Failed(e)))?;
    {
        let mut state = progress.file.lock();
        state.part_path = None;
        state.name = final_path.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    progress.total.store(received, Ordering::Relaxed);
    Ok(final_path)
}

/// Limite de débit simple pour un seul téléchargement.
struct Pace {
    window_start: Instant,
    bytes: u64,
}

impl Pace {
    fn new() -> Self {
        Self { window_start: Instant::now(), bytes: 0 }
    }

    async fn wait(&mut self, len: u64, limit_kbps: u32) {
        if limit_kbps == 0 {
            self.bytes = 0;
            self.window_start = Instant::now();
            return;
        }
        if self.window_start.elapsed() > Duration::from_secs(2) {
            self.window_start = Instant::now();
            self.bytes = 0;
        }
        self.bytes += len;
        let allowed = Duration::from_secs_f64(self.bytes as f64 / (limit_kbps as f64 * 1024.0));
        let elapsed = self.window_start.elapsed();
        if allowed > elapsed {
            tokio::time::sleep(allowed - elapsed).await;
        }
    }
}

#[cfg(test)]
mod safe_name_tests {
    use super::safe_file_name;

    #[test]
    fn hostile_names_stay_plain_file_names() {
        for (name, expected) in [
            ("Film 2026.mkv", "Film 2026.mkv"),
            (r"..\..\Windows\win.ini", ".._.._Windows_win.ini"),
            ("C:", "C_"),
            ("rapport.pdf:cache.exe", "rapport.pdf_cache.exe"),
            ("CON", "_CON"),
            ("conin$.txt", "_conin$.txt"),
            ("fin. . ", "fin"),
            ("..", "fichier"),
            ("", "fichier"),
        ] {
            assert_eq!(safe_file_name(name), expected, "{name:?}");
        }
        assert!(safe_file_name(&"a".repeat(500)).chars().count() <= 180);
    }
}
