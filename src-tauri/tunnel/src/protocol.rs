//! Messages échangés entre l'application et le service, une ligne JSON par message.

use serde::{Deserialize, Serialize};

pub const SERVICE_NAME: &str = "TorrentCloudTunnel";
pub const SERVICE_DISPLAY_NAME: &str = "Pocket Torrent – tunnel WireGuard";
pub const PIPE_NAME: &str = r"\\.\pipe\TorrentCloudTunnel";
/// Nom de la carte réseau créée (doit correspondre à `settings::WIREGUARD_ADAPTER`).
pub const ADAPTER_NAME: &str = "TorrentCloud";
/// Change quand le protocole change : l'application propose alors de réinstaller le service.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase")]
pub enum Request {
    Version,
    /// Monte le tunnel avec ce fichier de configuration WireGuard (texte du .conf).
    Up {
        config: String,
    },
    Down,
    Status,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    pub error: Option<String>,
    pub version: Option<u32>,
    pub status: Option<TunnelStatus>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self { ok: false, error: Some(message.into()), ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub up: bool,
    /// Adresses du tunnel (ex. 10.2.0.2/32).
    pub addresses: Vec<String>,
    pub endpoint: Option<String>,
    /// Secondes depuis la dernière poignée de main avec le serveur (None : jamais).
    pub handshake_age_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}
