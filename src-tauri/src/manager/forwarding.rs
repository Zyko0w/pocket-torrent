//! Redirection de port par le VPN (NAT-PMP, ProtonVPN) : renouvelée toutes les 45 s tant que le
//! VPN protège les téléchargements ; le moteur écoute sur le port public obtenu (un seul
//! redémarrage quand ce port apparaît ou change, pas à chaque renouvellement).

use super::*;
use crate::natpmp;

/// Renouvellement avant l'expiration de la redirection (60 s chez ProtonVPN).
const RENEW_EVERY: Duration = Duration::from_secs(45);
/// Après un échec : nouvel essai plus tard, sans insister toutes les secondes.
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct Forwarding {
    /// Port public obtenu ; le moteur écoute dessus.
    port: Option<u16>,
    error: Option<String>,
    last_attempt: Option<Instant>,
    busy: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardingView {
    pub enabled: bool,
    pub port: Option<u16>,
    pub error: Option<String>,
}

impl Manager {
    /// Réglages à donner au moteur : le port d'écoute devient le port public obtenu du VPN.
    pub(super) fn engine_settings(&self, base: &Settings) -> Settings {
        let mut settings = base.clone();
        if let Some(port) = self.forwarding.lock().port {
            settings.torrent_port = port;
        }
        settings
    }

    /// VPN changé ou coupé, option désactivée : le port obtenu ne vaut plus rien.
    pub(super) fn forget_forwarded_port(&self) {
        let mut forwarding = self.forwarding.lock();
        forwarding.port = None;
        forwarding.error = None;
        forwarding.last_attempt = None;
    }

    pub fn forwarding_view(&self) -> ForwardingView {
        let forwarding = self.forwarding.lock();
        ForwardingView {
            enabled: self.settings.read().vpn.port_forwarding,
            port: forwarding.port,
            error: forwarding.error.clone(),
        }
    }

    /// Appelé à chaque tick : lance un renouvellement quand il est dû.
    pub(super) fn maintain_forwarding(self: &Arc<Self>) {
        let enabled = self.settings.read().vpn.port_forwarding;
        let Some(local_ip) = *self.bound_ip.lock() else { return };
        if !enabled || !matches!(self.engine.binding(), Binding::Vpn { .. }) {
            return;
        }
        {
            let mut forwarding = self.forwarding.lock();
            let due = match forwarding.last_attempt {
                None => true,
                Some(at) if forwarding.error.is_some() => at.elapsed() >= RETRY_AFTER,
                Some(at) => at.elapsed() >= RENEW_EVERY,
            };
            if forwarding.busy || !due {
                return;
            }
            forwarding.busy = true;
            forwarding.last_attempt = Some(Instant::now());
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = manager.renew_forwarding(local_ip).await;
            let mut forwarding = manager.forwarding.lock();
            forwarding.busy = false;
            forwarding.error = result.err().map(|err| format!("{err:#}"));
            if let Some(error) = &forwarding.error {
                tracing::warn!("redirection de port : {error}");
            }
        });
    }

    async fn renew_forwarding(self: &Arc<Self>, local_ip: std::net::IpAddr) -> anyhow::Result<()> {
        let gateway = natpmp::gateway_for(local_ip)
            .ok_or_else(|| anyhow!("redirection de port impossible : le VPN n'a pas d'adresse IPv4"))?;
        let udp = natpmp::request(local_ip, gateway, natpmp::Protocol::Udp).await?;
        let tcp = natpmp::request(local_ip, gateway, natpmp::Protocol::Tcp).await?;
        if udp.public_port != tcp.public_port {
            bail!("le serveur VPN a donné deux ports différents ({} et {})", udp.public_port, tcp.public_port);
        }
        let port = tcp.public_port;
        let changed = self.forwarding.lock().port != Some(port);
        if !changed {
            return Ok(());
        }
        // VPN changé entre-temps : ce port appartient à l'ancien serveur
        let _switching = self.vpn_switching.lock().await;
        if *self.bound_ip.lock() != Some(local_ip) || !self.settings.read().vpn.port_forwarding {
            return Ok(());
        }
        tracing::info!(port, "redirection de port obtenue du VPN");
        self.forwarding.lock().port = Some(port);
        let settings = self.engine_settings(&self.settings());
        self.engine.restart(&settings, self.engine.binding(), &self.network()).await?;
        self.respawn_pending();
        self.apply_limits();
        Ok(())
    }
}
