//! Résolution de noms remplaçable (ajout de Pocket Torrent).
//!
//! Par défaut : résolveur du système. L'application peut installer le sien (DNS chiffré passant par le VPN)
//! pour que les noms des trackers et des nœuds DHT ne fuient pas en clair vers le fournisseur d'accès.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

pub type ResolveFuture = Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>;
pub type Resolver = Arc<dyn Fn(String, u16) -> ResolveFuture + Send + Sync>;

static RESOLVER: RwLock<Option<Resolver>> = RwLock::new(None);

/// Installe (ou retire, avec `None`) le résolveur utilisé par les trackers et la DHT.
pub fn set_resolver(resolver: Option<Resolver>) {
    *RESOLVER.write().unwrap_or_else(|e| e.into_inner()) = resolver;
}

/// Résout « hôte » et renvoie ses adresses avec le port donné.
pub async fn lookup_host(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let resolver = RESOLVER.read().unwrap_or_else(|e| e.into_inner()).clone();
    match resolver {
        Some(resolver) => resolver(host.to_owned(), port).await,
        None => Ok(tokio::net::lookup_host((host, port)).await?.collect()),
    }
}

/// Résout « hôte:port ».
pub async fn lookup_host_port(host_port: &str) -> std::io::Result<Vec<SocketAddr>> {
    if let Ok(addr) = host_port.parse::<SocketAddr>() {
        return Ok(vec![addr]);
    }
    let (host, port) = host_port
        .rsplit_once(':')
        .and_then(|(h, p)| {
            p.parse::<u16>()
                .ok()
                .map(|p| (h.trim_matches(['[', ']']), p))
        })
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "adresse sans port")
        })?;
    lookup_host(host, port).await
}
