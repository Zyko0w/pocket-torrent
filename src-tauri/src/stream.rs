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
    manager: Arc<Manager>,
    server: StreamServer,
}

/// Démarre le serveur sur un port libre de la boucle locale.
pub async fn start(manager: Arc<Manager>) -> anyhow::Result<StreamServer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let server = StreamServer { addr: listener.local_addr()?, token: uuid::Uuid::new_v4().simple().to_string().into() };
    let state = Arc::new(ServerState { manager, server: server.clone() });
    let app = axum::Router::new().route("/stream/{token}/{id}/{file}", get(stream_file)).with_state(state);
    tauri::async_runtime::spawn(async move {
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
    let path = match state.manager.stream_path(&id, file_index) {
        Ok(path) => path,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
