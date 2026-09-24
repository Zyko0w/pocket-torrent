//! Serveur de streaming local (§7.6 du plan d'upgrade) : sert un fichier de torrent en HTTP
//! avec les requêtes partielles (Range, via `tower-http`, pas réimplémenté à la main), pour
//! le lire dans VLC, mpv ou le navigateur pendant le téléchargement.
//!
//! N'écoute que sur 127.0.0.1, et ça ne suffit pas : n'importe quelle page web ouverte dans
//! le navigateur peut viser 127.0.0.1, et le « DNS rebinding » lui permet même de lire la
//! réponse. D'où un jeton secret tiré au démarrage, obligatoire dans chaque URL, et un en-tête
//! `Host` qui doit être exactement l'adresse du serveur.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header::HOST};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::manager::Manager;

/// Ce que le serveur sert : le chemin sur le disque d'un fichier de téléchargement. Séparé du
/// gestionnaire pour pouvoir attaquer le serveur dans les tests sans lancer l'application.
pub trait StreamSource: Send + Sync + 'static {
    fn stream_path(&self, id: &str, file_index: usize) -> anyhow::Result<std::path::PathBuf>;
}

impl StreamSource for Manager {
    fn stream_path(&self, id: &str, file_index: usize) -> anyhow::Result<std::path::PathBuf> {
        Manager::stream_path(self, id, file_index)
    }
}

#[derive(Clone)]
pub struct StreamServer {
    addr: SocketAddr,
    /// 128 bits aléatoires (uuid v4, aléa cryptographique du système), connus de l'appli seule.
    token: Arc<str>,
}

impl StreamServer {
    pub fn url(&self, id: &str, file_index: usize) -> String {
        format!("http://{}/stream/{}/{id}/{file_index}", self.addr, self.token)
    }
}

struct ServerState {
    source: Arc<dyn StreamSource>,
    server: StreamServer,
}

/// Démarre le serveur sur un port libre de la boucle locale.
pub async fn start(source: Arc<dyn StreamSource>) -> anyhow::Result<StreamServer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let server = StreamServer { addr: listener.local_addr()?, token: uuid::Uuid::new_v4().simple().to_string().into() };
    let state = Arc::new(ServerState { source, server: server.clone() });
    let app = axum::Router::new().route("/stream/{token}/{id}/{file}", get(stream_file)).with_state(state);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!("serveur de streaming arrêté : {err:#}");
        }
    });
    Ok(server)
}

/// Écrit une petite liste de lecture pointant vers le flux, puis l'ouvre : si VLC ou un
/// autre lecteur est associé aux fichiers .m3u, il démarre directement dessus.
pub fn open_in_player(data_dir: &Path, name: &str, url: &str) -> anyhow::Result<()> {
    let dir = data_dir.join("streams");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.m3u", crate::direct::safe_file_name(name)));
    // le nom vient du torrent : un retour à la ligne y ajouterait une entrée à la liste de lecture
    let title: String = name.chars().filter(|c| !c.is_control()).collect();
    std::fs::write(&path, format!("#EXTM3U\n#EXTINF:-1,{title}\n{url}\n"))?;
    crate::system::open_with_default(&path)
}

/// Comparaison en temps constant : la durée ne révèle pas combien de caractères sont justes.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn stream_file(
    axum::extract::Path((token, id, file_index)): axum::extract::Path<(String, String, usize)>,
    State(state): State<Arc<ServerState>>,
    request: Request,
) -> Response {
    let host_ok =
        request.headers().get(HOST).and_then(|h| h.to_str().ok()) == Some(state.server.addr.to_string().as_str());
    if !host_ok || !same_secret(&token, &state.server.token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = match state.source.stream_path(&id, file_index) {
        Ok(path) => path,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Test d'intrusion du serveur de streaming, dans un bac à sable : un fichier temporaire à
/// servir, et des requêtes HTTP écrites à la main comme les enverrait une page web hostile.
#[cfg(test)]
mod pentest {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{StreamServer, StreamSource, start};

    /// Un seul fichier servi : téléchargement « t:ok », fichier 0.
    struct Sandbox(PathBuf);

    impl StreamSource for Sandbox {
        fn stream_path(&self, id: &str, file_index: usize) -> anyhow::Result<PathBuf> {
            anyhow::ensure!(id == "t:ok" && file_index == 0, "introuvable");
            Ok(self.0.clone())
        }
    }

    /// Dossier du bac à sable, effacé même si le test échoue.
    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn server() -> (StreamServer, Cleanup) {
        let dir = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("film.mkv");
        std::fs::write(&file, b"0123456789").unwrap();
        let server = start(Arc::new(Sandbox(file))).await.unwrap();
        (server, Cleanup(dir))
    }

    /// Envoie une requête brute et renvoie le code HTTP et le corps.
    async fn raw(server: &StreamServer, path: &str, host: &str, extra: &str) -> (u16, String) {
        let mut socket = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n");
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        socket.read_to_end(&mut answer).await.unwrap();
        let text = String::from_utf8_lossy(&answer).into_owned();
        let code = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        (code, body)
    }

    #[tokio::test]
    async fn only_the_app_can_read_a_stream() {
        let (server, _cleanup) = server().await;
        let host = server.addr.to_string();
        let token = server.token.to_string();

        // accès légitime (lecteur vidéo), avec lecture partielle
        assert_eq!(raw(&server, &format!("/stream/{token}/t:ok/0"), &host, "").await, (200, "0123456789".into()));
        let (code, body) = raw(&server, &format!("/stream/{token}/t:ok/0"), &host, "Range: bytes=2-4\r\n").await;
        assert_eq!((code, body.as_str()), (206, "234"));

        // sans jeton, mauvais jeton, jeton tronqué : rien
        for path in ["/stream//t:ok/0", "/stream/00000000000000000000000000000000/t:ok/0", "/", "/stream/"] {
            assert_eq!(raw(&server, path, &host, "").await.0, 404, "{path}");
        }
        assert_eq!(raw(&server, &format!("/stream/{}/t:ok/0", &token[..31]), &host, "").await.0, 404);

        // DNS rebinding : une page de evil.example qui résout vers 127.0.0.1 garde son Host
        for bad_host in ["evil.example", "localhost", &format!("evil.example:{}", server.addr.port()), ""] {
            assert_eq!(raw(&server, &format!("/stream/{token}/t:ok/0"), bad_host, "").await.0, 404, "{bad_host}");
        }

        // autres fichiers : autre téléchargement, index hors limites, remontée de dossiers
        for path in [
            format!("/stream/{token}/t:autre/0"),
            format!("/stream/{token}/t:ok/1"),
            format!("/stream/{token}/t:ok/18446744073709551616"),
            format!("/stream/{token}/..%2F..%2Fsecrets/0"),
            format!("/stream/{token}/t:ok/0/../../../../Windows/win.ini"),
        ] {
            let (code, _) = raw(&server, &path, &host, "").await;
            assert!(code == 404 || code == 400, "{path} → {code}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::same_secret;

    #[test]
    fn secret_comparison() {
        assert!(same_secret("abc123", "abc123"));
        assert!(!same_secret("abc123", "abc124"));
        assert!(!same_secret("abc123", "abc12"));
        assert!(!same_secret("", "a"));
    }
}
