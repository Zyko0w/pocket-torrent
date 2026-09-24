//! Réglages : lecture et enregistrement.

use super::*;

impl Manager {
    pub fn settings_view(&self) -> SettingsView {
        SettingsView {
            settings: self.settings(),
            secrets: SecretsSet {
                r2_access_key_id: secrets::is_set(Secret::R2AccessKeyId),
                r2_secret_access_key: secrets::is_set(Secret::R2SecretAccessKey),
            },
            start_with_windows: system::start_with_windows_enabled(),
            magnet_handler: system::link_handler_enabled(),
            listen_port: self.engine.listen_port(),
            external_port: self.engine.external_port(),
            defender_available: crate::antivirus::available(),
        }
    }

    pub async fn save_settings(self: &Arc<Self>, input: SettingsInput) -> anyhow::Result<SettingsView> {
        let current = self.settings();
        let mut next = input.settings;
        next.normalize().map_err(anyhow::Error::msg)?;
        next.magnet_previous = current.magnet_previous.clone();
        next.torrent_previous = current.torrent_previous.clone();
        next.link_handler_asked = current.link_handler_asked;
        for dir in next.download_roots() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Impossible de créer le dossier « {} »", dir.display()))?;
        }

        for (secret, value) in [
            (Secret::R2AccessKeyId, &input.secrets.r2_access_key_id),
            (Secret::R2SecretAccessKey, &input.secrets.r2_secret_access_key),
        ] {
            if let Some(value) = value {
                secrets::set(secret, value)?;
            }
        }
        self.r2_keys_set.store(r2_keys_set(), Ordering::Relaxed);
        if input.start_with_windows != system::start_with_windows_enabled() {
            system::set_start_with_windows(input.start_with_windows)?;
        }
        if input.magnet_handler != system::link_handler_enabled() {
            let previous = (current.magnet_previous.clone(), current.torrent_previous.clone());
            let (magnet_previous, torrent_previous) = system::set_link_handler(input.magnet_handler, previous)?;
            next.magnet_previous = magnet_previous;
            next.torrent_previous = torrent_previous;
            // choisi dans les réglages : la question du premier lancement n'a plus lieu d'être
            next.link_handler_asked = true;
        }

        next.save(&self.settings_path)?;
        *self.settings.write() = next.clone();
        let vpn_changed = next.vpn.required_adapter() != current.vpn.required_adapter();
        #[cfg(windows)]
        if current.vpn.mode == VpnMode::Wireguard && next.vpn.mode != VpnMode::Wireguard {
            // tunnel intégré abandonné : on le démonte
            let _ = tauri::async_runtime::spawn_blocking(crate::tunnel::disconnect).await;
        }
        if next.vpn.mode == VpnMode::Wireguard {
            self.reset_tunnel_attempt(None, true);
        }
        if vpn_changed {
            self.check_vpn().await?;
        } else if crate::engine::EngineConfig::of(&next) != crate::engine::EngineConfig::of(&current) {
            // nouveau port, µTP, plafond de pairs, places d'envoi, chiffrement ou liste de blocage : le moteur repart
            let _switching = self.vpn_switching.lock().await;
            self.engine.restart(&next, self.engine.binding(), &self.network()).await?;
            self.respawn_pending();
        }
        self.game.lock().checked = false;
        self.apply_limits();
        self.check_game().await;
        tracing::info!("réglages enregistrés");
        Ok(self.settings_view())
    }

    pub fn test_notification(&self) {
        self.show_notification("Pocket Torrent", "Les notifications fonctionnent.");
    }

    pub async fn test_r2(&self) -> anyhow::Result<()> {
        let r2 = R2::from_settings(&self.settings().r2).ok_or_else(|| {
            anyhow!("Renseigne l'identifiant de compte, le bucket et les deux clés, puis enregistre.")
        })??;
        r2.check(&self.http()).await
    }

    /// Poser la question du premier lancement : jamais posée, et Pocket Torrent n'ouvre pas
    /// déjà les liens magnet.
    pub fn should_ask_link_handler(&self) -> bool {
        !self.settings().link_handler_asked && !system::link_handler_enabled()
    }

    /// Réponse à la question du premier lancement. En mode dev, rien n'est enregistré ni écrit
    /// dans Windows : les liens pointeraient vers l'exe de test, et la version installée ne
    /// poserait plus jamais la question (mêmes réglages).
    pub fn answer_link_handler(&self, enable: bool) -> anyhow::Result<()> {
        if cfg!(debug_assertions) {
            tracing::info!("mode dev : réponse « liens magnet » ({enable}) ni enregistrée ni appliquée");
            return Ok(());
        }
        let mut next = self.settings();
        if enable && !system::link_handler_enabled() {
            let previous = (next.magnet_previous.clone(), next.torrent_previous.clone());
            let (magnet_previous, torrent_previous) = system::set_link_handler(true, previous)?;
            next.magnet_previous = magnet_previous;
            next.torrent_previous = torrent_previous;
        }
        next.link_handler_asked = true;
        next.save(&self.settings_path)?;
        *self.settings.write() = next;
        Ok(())
    }
}
