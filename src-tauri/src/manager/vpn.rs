//! VPN : liaison à la carte réseau, arrêt automatique, tunnel intégré, vérification d'IP.

use super::*;

impl Manager {
    pub fn http(&self) -> reqwest::Client {
        self.network.read().http.clone()
    }

    pub(super) fn network(&self) -> direct::Network {
        self.network.read().clone()
    }

    pub(super) fn vpn_offline(&self) -> bool {
        self.engine.binding() == Binding::Offline
    }

    /// Arrêt automatique : compare la liaison voulue à celle du moteur, et bascule si besoin.
    /// VPN coupé → tout se met en pause et le moteur ne peut plus rien envoyer ;
    /// VPN revenu (ou nouvelle adresse) → moteur relié au VPN, téléchargements relancés.
    pub async fn check_vpn(self: &Arc<Self>) -> anyhow::Result<()> {
        let _switching = self.vpn_switching.lock().await;
        let settings = self.settings();
        let desired = desired_binding(&settings);
        if settings.vpn.mode == VpnMode::Wireguard && desired == Binding::Offline {
            self.reconnect_tunnel();
        }
        let current = self.engine.binding();
        let desired_ip = match &desired {
            Binding::Vpn { adapter, .. } => netif::find(adapter).and_then(|a| a.source_ip()),
            _ => None,
        };
        let ip_changed = matches!(desired, Binding::Vpn { .. }) && desired_ip != *self.bound_ip.lock();

        if desired == current && !ip_changed {
            if current == Binding::Offline {
                // torrents relancés par le moteur au démarrage : on les arrête aussitôt
                self.hold_running(Hold::Vpn).await;
            }
            if settings.vpn.mode == VpnMode::Wireguard && matches!(current, Binding::Vpn { .. }) {
                self.check_tunnel_health();
            } else {
                *self.tunnel_warning.lock() = None;
            }
            return Ok(());
        }
        *self.vpn_since.lock() = Some(Instant::now());
        *self.tunnel_warning.lock() = None;

        tracing::warn!(?current, ?desired, "bascule VPN");
        self.hold_running(Hold::Vpn).await;
        let network = network_for(&desired);
        self.engine.restart(&settings, desired.clone(), &network).await?;
        *self.network.write() = network;
        self.respawn_pending();
        *self.bound_ip.lock() = desired_ip;
        self.apply_limits();

        match desired {
            Binding::Offline => {
                if settings.vpn.mode != VpnMode::Off {
                    self.notify(
                        Notify::Error,
                        "VPN coupé",
                        "Les téléchargements sont en pause et reprendront quand le VPN reviendra.",
                    );
                }
            }
            _ => {
                let released = self.release_held(Hold::Vpn);
                if current == Binding::Offline && released > 0 {
                    self.notify(Notify::Done, "VPN rétabli", "Les téléchargements reprennent.");
                }
            }
        }
        self.save_now();
        Ok(())
    }

    /// Tunnel intégré absent : relance la connexion en arrière-plan (au plus toutes les 30 s).
    fn reconnect_tunnel(self: &Arc<Self>) {
        #[cfg(windows)]
        {
            {
                let mut attempt = self.tunnel_attempt.lock();
                if self.tunnel_manual_off.load(Ordering::Relaxed)
                    || attempt.is_some_and(|at| at.elapsed() < Duration::from_secs(30))
                    || !crate::tunnel::has_config()
                {
                    return;
                }
                *attempt = Some(Instant::now());
            }
            let manager = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = tauri::async_runtime::spawn_blocking(crate::tunnel::connect).await;
                let error = match result {
                    Ok(Ok(_)) => None,
                    Ok(Err(err)) => Some(format!("{err:#}")),
                    Err(err) => Some(err.to_string()),
                };
                if let Some(error) = &error {
                    tracing::warn!("reconnexion du tunnel impossible : {error}");
                }
                *manager.tunnel_error.lock() = error;
            });
        }
    }

    /// Toutes les 10 s : le serveur du tunnel répond-il ? (poignée de main WireGuard récente)
    fn check_tunnel_health(self: &Arc<Self>) {
        #[cfg(windows)]
        {
            {
                let mut at = self.tunnel_health_at.lock();
                if at.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                    return;
                }
                *at = Some(Instant::now());
            }
            let manager = self.clone();
            tauri::async_runtime::spawn(async move {
                let status = tauri::async_runtime::spawn_blocking(|| crate::tunnel::view().status).await.ok().flatten();
                let connected_for = manager.vpn_since.lock().map_or(Duration::ZERO, |since| since.elapsed());
                let warning = match status {
                    Some(s) if s.up => match s.handshake_age_secs {
                        None if connected_for > Duration::from_secs(30) => Some(
                            "Le serveur VPN ne répond pas : vérifie le fichier .conf ou ton abonnement.".to_owned(),
                        ),
                        Some(age) if age > 180 => {
                            Some(format!("Le serveur VPN ne répond plus depuis {} min.", age / 60))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                *manager.tunnel_warning.lock() = warning;
            });
        }
    }

    pub fn set_tunnel_manual_off(&self, off: bool) {
        self.tunnel_manual_off.store(off, Ordering::Relaxed);
    }

    pub fn tunnel_error(&self) -> Option<String> {
        self.tunnel_error.lock().clone()
    }

    /// Après une action sur le tunnel. `retry_now` : reconnexion automatique permise tout de suite
    /// (sinon on laisse 30 s, pour ne pas remonter le tunnel pendant qu'une connexion manuelle s'établit).
    pub fn reset_tunnel_attempt(&self, error: Option<String>, retry_now: bool) {
        *self.tunnel_attempt.lock() = if retry_now { None } else { Some(Instant::now()) };
        *self.tunnel_error.lock() = error;
    }

    /// Met en pause tout ce qui télécharge, pour la raison donnée.
    async fn hold_running(self: &Arc<Self>, reason: Hold) {
        let (running, _) = self.slots(&self.engine.torrents());
        for slot in running {
            self.hold(&slot.key, reason).await;
        }
    }

    /// Adresse publique vue par Internet, par le VPN et sans lui (bouton « Vérifier »).
    pub async fn check_public_ip(&self) -> IpCheck {
        async fn ask(client: reqwest::Client) -> Result<String, String> {
            let response = client
                .get("https://api.ipify.org")
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| format!("{:#}", anyhow::Error::from(e.without_url())))?;
            response.text().await.map(|t| t.trim().to_owned()).map_err(|e| e.to_string())
        }
        let binding = self.engine.binding();
        let without = ask(direct::http_client(None, false, dns::Resolver::new(dns::Mode::Free))).await;
        let through = match binding {
            Binding::Vpn { .. } => Some(ask(self.http()).await),
            _ => None,
        };
        IpCheck {
            normal: without.as_ref().ok().cloned(),
            vpn: through.as_ref().and_then(|r| r.as_ref().ok().cloned()),
            error: through.and_then(|r| r.err()).or(without.err()),
        }
    }

    pub(super) fn vpn_view(&self, settings: &Settings) -> VpnView {
        let binding = self.engine.binding();
        let (state, adapter) = match (&binding, settings.vpn.mode) {
            (_, VpnMode::Off) => ("off", None),
            (Binding::Vpn { adapter, .. }, _) => ("protected", Some(adapter.clone())),
            _ => ("blocked", settings.vpn.required_adapter().map(str::to_owned)),
        };
        VpnView {
            mode: settings.vpn.mode,
            state,
            adapter,
            ip: (*self.bound_ip.lock()).map(|ip| ip.to_string()),
            warning: if state == "protected" { self.tunnel_warning.lock().clone() } else { None },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnView {
    pub mode: VpnMode,
    /// off | protected | blocked
    pub state: &'static str,
    pub adapter: Option<String>,
    pub ip: Option<String>,
    /// Protégé mais inutilisable (serveur du tunnel muet).
    pub warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpCheck {
    pub vpn: Option<String>,
    pub normal: Option<String>,
    pub error: Option<String>,
}
