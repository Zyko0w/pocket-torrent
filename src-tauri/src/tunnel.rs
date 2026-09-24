//! Tunnel WireGuard intégré, côté application : configuration (rangée dans Windows),
//! service (installé une fois avec l'autorisation administrateur) et connexion.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use pocket_torrent_tunnel::client::{self, ServiceState};
use pocket_torrent_tunnel::config::{self, Summary};
use pocket_torrent_tunnel::protocol::{Request, TunnelStatus};
use serde::Serialize;

use crate::secrets::{self, Secret};

/// Taille maximale d'un secret dans le Gestionnaire d'identification Windows.
const MAX_CONFIG_BYTES: usize = 2560;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub service: ServiceState,
    /// Résumé de la configuration importée (sans la clé privée).
    pub config: Option<Summary>,
    pub status: Option<TunnelStatus>,
}

fn tunnel_exe() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name("pocket-torrent-tunnel.exe"))
}

fn stored_config() -> Option<String> {
    secrets::get(Secret::WireguardConfig)
}

pub fn has_config() -> bool {
    stored_config().is_some()
}

pub fn view() -> TunnelView {
    let service = client::service_state();
    let status = (service == ServiceState::Running)
        .then(|| client::call(&Request::Status).ok().and_then(|r| r.status))
        .flatten();
    TunnelView {
        service,
        config: stored_config().and_then(|text| config::parse(&text).ok()).map(|c| c.summary()),
        status,
    }
}

/// Vérifie puis range le fichier .conf dans le Gestionnaire d'identification Windows.
pub fn import(path: &Path) -> anyhow::Result<Summary> {
    let text = std::fs::read_to_string(path).context("fichier illisible")?;
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if text.len() > MAX_CONFIG_BYTES {
        bail!("fichier trop grand pour une configuration WireGuard");
    }
    let parsed = config::parse(&text).context("configuration WireGuard invalide")?;
    secrets::set(Secret::WireguardConfig, &text)?;
    Ok(parsed.summary())
}

pub fn forget() -> anyhow::Result<()> {
    let _ = disconnect();
    secrets::set(Secret::WireguardConfig, "")
}

pub fn install_service() -> anyhow::Result<()> {
    client::install(&tunnel_exe()?)
}

pub fn uninstall_service() -> anyhow::Result<()> {
    client::uninstall(&tunnel_exe()?)
}

pub fn connect() -> anyhow::Result<TunnelStatus> {
    let config = stored_config().context("importe d'abord le fichier .conf de ton VPN")?;
    match client::service_state() {
        ServiceState::Running => {}
        ServiceState::NotInstalled => bail!("installe d'abord le service du tunnel"),
        ServiceState::Outdated => bail!("le service du tunnel doit être mis à jour : réinstalle-le"),
        ServiceState::Stopped => bail!("le service du tunnel est arrêté"),
    }
    let response = client::call(&Request::Up { config })?;
    Ok(response.status.unwrap_or_default())
}

pub fn disconnect() -> anyhow::Result<()> {
    if client::service_state() != ServiceState::Running {
        return Ok(());
    }
    client::call(&Request::Down).map(|_| ())
}
