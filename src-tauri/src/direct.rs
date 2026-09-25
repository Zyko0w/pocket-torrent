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
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
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
    http_client_builder(local_ip, offline, None, resolver).build().expect("client HTTP")
}

/// `proxy` : adresse `socks5h://` par laquelle tout doit passer (noms résolus par le proxy).
fn http_client_builder(
    local_ip: Option<std::net::IpAddr>,
    offline: bool,
    proxy: Option<&str>,
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
    } else if let Some(proxy) = proxy {
        // adresse invalide : proxy injoignable plutôt qu'une sortie directe
        let proxy = reqwest::Proxy::all(proxy)
            .unwrap_or_else(|_| reqwest::Proxy::all("socks5://127.0.0.1:9").expect("proxy local"));
        builder = builder.proxy(proxy);
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
    proxy: Option<String>,
    resolver: Arc<crate::dns::Resolver>,
}

impl Network {
    pub fn new(local_ip: Option<std::net::IpAddr>, offline: bool, resolver: Arc<crate::dns::Resolver>) -> Self {
        Self::with_proxy(local_ip, offline, None, resolver)
    }

    /// Réseau passant entièrement par un proxy (`socks5h://…`).
    pub fn with_proxy(
        local_ip: Option<std::net::IpAddr>,
        offline: bool,
        proxy: Option<String>,
        resolver: Arc<crate::dns::Resolver>,
    ) -> Self {
        let http =
            http_client_builder(local_ip, offline, proxy.as_deref(), resolver.clone()).build().expect("client HTTP");
        Self { http, local_ip, offline, proxy, resolver }
    }

    /// Client des sources web (BEP 19) : même sortie (VPN), même DNS chiffré, mais adresses du
    /// réseau local retirées après résolution et redirections contrôlées ; HTTPS seul si
    /// `https_only` (mode discret).
    pub fn webseed_client(&self, https_only: bool) -> reqwest::Client {
        librqbit::protect_webseed_client(
            http_client_builder(self.local_ip, self.offline, self.proxy.as_deref(), self.resolver.clone()),
            Arc::new(crate::dns::ReqwestResolver(self.resolver.clone())),
            https_only,
        )
        .build()
        .expect("client HTTP des sources web")
    }

    /// Client limité à Internet : mêmes protections que les sources web (adresses locales retirées
    /// après résolution, redirections contrôlées), pour les liens venus d'un tiers (flux RSS).
    pub fn public_client(&self) -> reqwest::Client {
        self.webseed_client(false)
    }
}

/// Octets reçus par tous les liens directs depuis le lancement (statistiques cumulées).
pub static RECEIVED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Lien écrit avec une adresse de ce PC ou du réseau local (NAS, box) : l'utilisateur vise
/// volontairement ce réseau. Tout autre lien passe par le client limité à Internet
/// (`Network::public_client`), pour qu'un site ne puisse pas rediriger vers la box ou un NAS,
/// ni se résoudre vers eux (« DNS rebinding »).
pub fn is_local_link(link: &str) -> bool {
    url::Url::parse(link).is_ok_and(|u| crate::rss::is_local_host(&u))
}

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
    /// Dossier du « .part » s'il diffère de `dir` (dossier « en cours ») : le fichier terminé
    /// est déplacé dans `dir`.
    pub part_dir: Option<PathBuf>,
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
    // « .part » déjà complet (coupure juste avant le renommage) : rien à recevoir
    if status == StatusCode::RANGE_NOT_SATISFIABLE
        && received > 0
        && received == progress.total.load(Ordering::Relaxed)
        && let Some(part) = existing_part
    {
        return finish(job, part, received);
    }
    if !status.is_success() {
        let error = anyhow!("le serveur a répondu HTTP {status}");
        return Err(if matches!(status.as_u16(), 401 | 404 | 410) {
            Attempt::Fatal(DownloadError::Failed(error))
        } else {
            retry(error)
        });
    }

    let resume = received > 0 && status == StatusCode::PARTIAL_CONTENT;
    // reprise : le serveur doit renvoyer exactement la suite, sinon ces octets seraient collés
    // au mauvais endroit (fichier corrompu). Le « .part » est vidé et tout est repris du début.
    if resume && range_start(&response) != Some(received) {
        if let Some(part) = &existing_part {
            let _ = std::fs::OpenOptions::new().write(true).open(part).and_then(|f| f.set_len(0));
        }
        progress.received.store(0, Ordering::Relaxed);
        return Err(retry(anyhow!("le serveur n'a pas renvoyé la suite demandée : reprise depuis le début")));
    }
    if !resume {
        received = 0;
    }
    let length =
        response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    if progress.total.load(Ordering::Relaxed) == 0
        && let Some(length) = length
    {
        progress.total.store(length.saturating_add(received), Ordering::Relaxed);
    }

    let part = match existing_part {
        Some(part) => part,
        None => {
            let name = progress.file.lock().name.clone().unwrap_or_else(|| safe_file_name(&file_name_from(&response)));
            let part_dir = job.part_dir.as_ref().unwrap_or(&job.dir);
            std::fs::create_dir_all(part_dir).map_err(|e| Attempt::Fatal(DownloadError::Failed(e.into())))?;
            let target = unique_path(part_dir, &name);
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
        (job.check_space)(job.part_dir.as_ref().unwrap_or(&job.dir), total - received)
            .map_err(|msg| Attempt::Fatal(DownloadError::NoSpace(msg)))?;
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
    let space_dir = job.part_dir.as_ref().unwrap_or(&job.dir);
    let mut since_space_check = 0u64;
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
        // taille inconnue ou disque partagé avec les torrents : place revérifiée tous les 64 Mio
        since_space_check += chunk.len() as u64;
        if since_space_check >= SPACE_CHECK_EVERY {
            since_space_check = 0;
            if let Err(msg) = (job.check_space)(space_dir, 0) {
                let _ = file.flush().await;
                return Err(Attempt::Fatal(DownloadError::NoSpace(msg)));
            }
        }
    }
    file.flush().await.map_err(|e| retry(e.into()))?;
    drop(file);

    let total = progress.total.load(Ordering::Relaxed);
    if total > 0 && received < total {
        return Err(retry(anyhow!("connexion coupée avant la fin")));
    }
    finish(job, part, received)
}

/// Octets reçus entre deux vérifications de la place libre.
const SPACE_CHECK_EVERY: u64 = 64 * 1024 * 1024;

/// Début de la plage renvoyée (`Content-Range: bytes 100-199/200`).
fn range_start(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?;
    value.trim().strip_prefix("bytes")?.trim_start().split('-').next()?.trim().parse().ok()
}

/// Range le « .part » terminé sous son nom final.
fn finish(job: &Download, part: PathBuf, received: u64) -> Result<PathBuf, Attempt> {
    let progress = &job.progress;
    // « film.mkv.part » → « film.mkv »
    let part_name = part.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let final_name = part_name.strip_suffix(".part").unwrap_or(&part_name).to_owned();
    // à côté du « .part », ou dans le dossier final s'il était dans le dossier « en cours »
    let final_dir =
        if job.part_dir.is_some() { job.dir.clone() } else { part.parent().unwrap_or(&job.dir).to_path_buf() };
    let mut final_path = final_dir.join(&final_name);
    if final_path.exists() {
        final_path = unique_path(&final_dir, &final_name);
    }
    crate::relocate::move_file(&part, &final_path)
        .with_context(|| format!("impossible de ranger {} dans {}", part.display(), final_dir.display()))
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

#[cfg(test)]
mod incomplete_dir_tests {
    use super::*;

    /// Lien direct avec dossier « en cours » : le « .part » s'y écrit, le fichier terminé part
    /// dans le dossier final. Faux serveur web sur 127.0.0.1, dossiers temporaires dédiés.
    #[tokio::test(flavor = "multi_thread")]
    async fn direct_download_lands_in_the_final_folder() {
        let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        let (incomplete, finale) = (root.join("en cours"), root.join("Films"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/film.mkv", axum::routing::get(|| async { vec![9u8; 200_000] }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let settings = crate::settings::Settings { download_dir: root.join("telechargements"), ..Default::default() };
        let network = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
        let engine =
            crate::engine::Engine::start(&settings, &root.join("donnees"), crate::engine::Binding::Offline, &network)
                .await
                .unwrap();
        let job = Download {
            url: format!("http://{addr}/film.mkv"),
            dir: finale.clone(),
            part_dir: Some(incomplete.clone()),
            progress: Arc::new(Progress::default()),
            session: engine.session(),
            check_space: Arc::new(|_: &Path, _: u64| Ok(())),
        };
        let path = match run(&reqwest::Client::new(), job).await {
            Ok(path) => path,
            Err(DownloadError::NoSpace(msg)) => panic!("{msg}"),
            Err(DownloadError::Failed(err)) => panic!("{err:#}"),
        };
        assert_eq!(path, finale.join("film.mkv"));
        assert_eq!(std::fs::read(&path).unwrap().len(), 200_000);
        assert_eq!(std::fs::read_dir(&incomplete).unwrap().count(), 0, "aucun « .part » laissé");
        engine.stop().await;
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod pentest_tests {
    //! Test d'intrusion : serveur web hostile (127.0.0.1, lancé par le test) contre les
    //! téléchargements directs. Dossiers temporaires dédiés ; rien ne sort de ce PC.
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::AsyncReadExt;

    const LEN: usize = 300_000;

    fn body() -> Vec<u8> {
        (0..LEN).map(|i| (i * 7 % 251) as u8).collect()
    }

    const HOSTILE_NAMES: &[&str] = &[
        "attachment; filename*=UTF-8''..%2F..%2F..%2Fevil.exe",
        r#"attachment; filename="..\..\Windows\evil.dll""#,
        r#"attachment; filename="C:\Users\Public\x.bat""#,
        r#"attachment; filename="\\serveur\partage\x.exe""#,
        r#"attachment; filename="rapport.pdf:flux.exe""#,
        r#"attachment; filename="CON""#,
        "attachment; filename*=UTF-8''film%E2%80%AE4pm.exe",
    ];

    async fn respond(stream: &mut tokio::net::TcpStream, head: &str, body: &[u8]) {
        let _ = stream.write_all(format!("HTTP/1.1 {head}\r\nConnection: close\r\n\r\n").as_bytes()).await;
        let _ = stream.write_all(body).await;
    }

    /// Faux serveur : noms piégés, reprises fausses, flux sans fin, taille absurde.
    async fn hostile_server() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let range: Option<usize> = request
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim().trim_end_matches('-').parse().ok());
                    let body = body();
                    match (path.as_str(), range) {
                        (p, _) if p.starts_with("/nom/") => {
                            let n: usize = p[5..].parse().unwrap();
                            let head =
                                format!("200 OK\r\nContent-Length: 1\r\nContent-Disposition: {}", HOSTILE_NAMES[n]);
                            respond(&mut stream, &head, b"x").await;
                        }
                        ("/reprise", Some(r)) => {
                            let head = format!(
                                "206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {r}-{}/{LEN}",
                                LEN - r,
                                LEN - 1
                            );
                            respond(&mut stream, &head, &body[r..]).await;
                        }
                        // ignore la plage demandée et renvoie tout en 206
                        ("/reprise-fausse", Some(_)) => {
                            let head = format!(
                                "206 Partial Content\r\nContent-Length: {LEN}\r\nContent-Range: bytes 0-{}/{LEN}",
                                LEN - 1
                            );
                            respond(&mut stream, &head, &body).await;
                        }
                        ("/reprise-sans-plage", Some(_)) => {
                            respond(&mut stream, &format!("206 Partial Content\r\nContent-Length: {LEN}"), &body).await;
                        }
                        ("/complet", Some(_)) => {
                            let head = format!(
                                "416 Range Not Satisfiable\r\nContent-Range: bytes */{LEN}\r\nContent-Length: 0"
                            );
                            respond(&mut stream, &head, b"").await;
                        }
                        ("/sans-fin", _) => {
                            respond(&mut stream, "200 OK", b"").await;
                            let chunk = vec![0u8; 1024 * 1024];
                            while stream.write_all(&chunk).await.is_ok() {}
                        }
                        ("/geant", _) => {
                            respond(&mut stream, "200 OK\r\nContent-Length: 1152921504606846976", b"debut").await;
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                        _ => respond(&mut stream, &format!("200 OK\r\nContent-Length: {LEN}"), &body).await,
                    }
                });
            }
        });
        addr
    }

    struct Case {
        url: String,
        dir: PathBuf,
        /// « .part » déjà présent (reprise) : son contenu.
        part: Option<Vec<u8>>,
        check_space: SpaceCheck,
    }

    async fn run_case(session: &Arc<Session>, case: Case) -> Result<PathBuf, DownloadError> {
        std::fs::create_dir_all(&case.dir).unwrap();
        let progress = Arc::new(Progress::default());
        if let Some(bytes) = &case.part {
            let part = case.dir.join("film.bin.part");
            std::fs::write(&part, bytes).unwrap();
            progress.total.store(LEN as u64, Ordering::Relaxed);
            let mut file = progress.file.lock();
            file.name = Some("film.bin".into());
            file.part_path = Some(part);
        }
        let job = Download {
            url: case.url,
            dir: case.dir,
            part_dir: None,
            progress,
            session: session.clone(),
            check_space: case.check_space,
        };
        tokio::time::timeout(Duration::from_secs(60), run(&reqwest::Client::new(), job))
            .await
            .unwrap_or_else(|_| panic!("téléchargement bloqué"))
    }

    fn ok(result: Result<PathBuf, DownloadError>) -> PathBuf {
        match result {
            Ok(path) => path,
            Err(DownloadError::NoSpace(msg)) => panic!("place : {msg}"),
            Err(DownloadError::Failed(err)) => panic!("{err:#}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hostile_web_server_cannot_escape_the_folder_nor_corrupt_files() {
        let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        let addr = hostile_server().await;
        let settings = crate::settings::Settings { download_dir: root.join("telechargements"), ..Default::default() };
        let network = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
        let engine =
            crate::engine::Engine::start(&settings, &root.join("donnees"), crate::engine::Binding::Offline, &network)
                .await
                .unwrap();
        let session = engine.session();
        let any_space: SpaceCheck = Arc::new(|_: &Path, _: u64| Ok(()));

        // 1. noms piégés annoncés par le serveur : le fichier reste dans le dossier choisi
        let names = root.join("noms");
        for (n, header) in HOSTILE_NAMES.iter().enumerate() {
            let case = Case {
                url: format!("http://{addr}/nom/{n}"),
                dir: names.clone(),
                part: None,
                check_space: any_space.clone(),
            };
            let path = ok(run_case(&session, case).await);
            assert_eq!(path.parent(), Some(names.as_path()), "{header}");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                !name.contains(':') && !name.chars().any(librqbit_core::torrent_metainfo::is_bidi_control),
                "{name:?}"
            );
        }
        assert_eq!(std::fs::read_dir(&names).unwrap().count(), HOSTILE_NAMES.len());
        assert!(!root.join("evil.exe").exists() && !std::env::temp_dir().join("evil.exe").exists());

        // 2. reprises : honnête, plage fausse, plage absente, fichier déjà complet
        let body = body();
        for (route, part) in [
            ("reprise", body[..100_000].to_vec()),
            ("reprise-fausse", body[..100_000].to_vec()),
            ("reprise-sans-plage", body[..100_000].to_vec()),
            ("complet", body.clone()),
        ] {
            let dir = root.join(route);
            let case = Case {
                url: format!("http://{addr}/{route}"),
                dir: dir.clone(),
                part: Some(part),
                check_space: any_space.clone(),
            };
            let path = ok(run_case(&session, case).await);
            assert_eq!(path, dir.join("film.bin"), "{route}");
            assert!(std::fs::read(&path).unwrap() == body, "{route} : fichier corrompu");
        }

        // 3. flux sans fin, sans taille annoncée : mis en pause dès que le disque est « plein »
        let checks = Arc::new(AtomicU32::new(0));
        let counted = checks.clone();
        let case = Case {
            url: format!("http://{addr}/sans-fin"),
            dir: root.join("sans-fin"),
            part: None,
            check_space: Arc::new(move |_: &Path, needed: u64| {
                counted.fetch_add(1, Ordering::Relaxed);
                if needed == 0 { Err("disque plein".into()) } else { Ok(()) }
            }),
        };
        assert!(matches!(run_case(&session, case).await, Err(DownloadError::NoSpace(_))), "flux sans fin");
        assert_eq!(checks.load(Ordering::Relaxed), 1);

        // 4. taille absurde annoncée : refus immédiat, sans débordement de calcul
        let case = Case {
            url: format!("http://{addr}/geant"),
            dir: root.join("geant"),
            part: None,
            check_space: Arc::new(
                |_: &Path, needed: u64| if needed > 1 << 40 { Err("trop gros".into()) } else { Ok(()) },
            ),
        };
        assert!(matches!(run_case(&session, case).await, Err(DownloadError::NoSpace(_))), "taille absurde");

        engine.stop().await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Résolveur de test : un nom public qui pointe vers ce PC (« DNS rebinding »).
    struct Rebinding;
    impl reqwest::dns::Resolve for Rebinding {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async {
                let addrs: reqwest::dns::Addrs = Box::new(std::iter::once(SocketAddr::from(([127, 0, 0, 1], 0))));
                Ok(addrs)
            })
        }
    }

    /// Lien public : client limité à Internet (le site ne peut viser ni la box ni un NAS) ;
    /// lien écrit avec une adresse locale : l'utilisateur vise ce réseau, client ordinaire.
    #[tokio::test]
    async fn public_links_cannot_reach_the_local_network() {
        for link in ["http://192.168.1.10/film.mkv", "http://nas.local/x", "http://localhost:8080/x", "http://[::1]/x"]
        {
            assert!(is_local_link(link), "{link}");
        }
        for link in ["https://example.com/x.iso", "http://8.8.8.8/x", "http://box.example.com/x"] {
            assert!(!is_local_link(link), "{link}");
        }
        let addr = hostile_server().await;
        let rebound = format!("http://site.example:{}/film", addr.port());
        let plain = reqwest::Client::builder().dns_resolver(Arc::new(Rebinding)).build().unwrap();
        assert!(plain.get(&rebound).send().await.is_ok(), "le danger existe");
        let public =
            librqbit::protect_webseed_client(reqwest::Client::builder(), Arc::new(Rebinding), false).build().unwrap();
        assert!(public.get(&rebound).send().await.is_err(), "nom public résolu vers ce PC : refusé");
    }
}
