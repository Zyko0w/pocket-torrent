//! Moteur torrent : enveloppe autour de librqbit, exposant des vues simples.
//! Les torrents sont désignés par leur empreinte (info hash), stable d'un lancement à l'autre.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig, ListenerMode,
    ListenerOptions, ManagedTorrent, Session, SessionOptions, SessionPersistenceConfig, TorrentStatsState,
    generate_azereus_style,
};
use parking_lot::{Mutex, RwLock};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;

use librqbit::mse::EncryptionMode;

use crate::direct::{Network, safe_file_name};
use crate::settings::{EncryptionLevel, Settings, SpeedLimits};

/// Trackers publics ajoutés à chaque torrent pour trouver des pairs plus vite.
pub const EXTRA_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
];

/// Caractères laissés tels quels dans les paramètres d'un lien magnet.
const MAGNET_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

pub struct Engine {
    /// Remplacée au redémarrage du moteur (changement de port ou de VPN).
    session: RwLock<Arc<Session>>,
    binding: RwLock<Binding>,
    data_dir: PathBuf,
    /// Torrents déjà résolus lors d'un aperçu, pour ne pas rechercher les métadonnées deux fois.
    resolved: Mutex<HashMap<String, Resolved>>,
    /// Derniers compteurs de chaque pair, pour calculer sa vitesse.
    peer_samples: Mutex<HashMap<(String, String), PeerSample>>,
}

mod add;
mod config;
#[cfg(test)]
mod tests;
mod views;

pub use add::looks_like_path;
use add::*;
use config::*;
pub use config::{Binding, EngineConfig, ProxyUrl, limits_config};
use views::*;
pub use views::{EngineState, FileView, PeerView, PieceMap, Preview, TorrentView, TrackerView};

impl Engine {
    pub async fn start(
        settings: &Settings,
        data_dir: &Path,
        binding: Binding,
        network: &Network,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session: RwLock::new(create_session(settings, data_dir, &binding, network).await?),
            binding: RwLock::new(binding),
            data_dir: data_dir.to_path_buf(),
            resolved: Mutex::new(HashMap::new()),
            peer_samples: Mutex::new(HashMap::new()),
        })
    }

    pub fn session(&self) -> Arc<Session> {
        self.session.read().clone()
    }

    pub fn binding(&self) -> Binding {
        self.binding.read().clone()
    }

    /// Redémarre le moteur (nouveau port, VPN coupé ou rétabli).
    /// Les torrents en cours reprennent ; ceux mis en pause restent en pause.
    pub async fn restart(&self, settings: &Settings, binding: Binding, network: &Network) -> anyhow::Result<()> {
        self.session().stop().await;
        // les vitesses limites courantes (mode jeu compris) sont réappliquées par l'appelant
        let session = match create_session(settings, &self.data_dir, &binding, network).await {
            Ok(session) => session,
            // port déjà pris (autre programme, port imposé par le VPN…) : l'ancien moteur est déjà
            // arrêté, on repart sur un port libre plutôt que de laisser les torrents sans moteur
            Err(err) if settings.torrent_port != 0 => {
                tracing::warn!("port {} indisponible ({err:#}) : port libre choisi à la place", settings.torrent_port);
                let fallback = Settings { torrent_port: 0, ..settings.clone() };
                create_session(&fallback, &self.data_dir, &binding, network).await?
            }
            Err(err) => return Err(err),
        };
        *self.session.write() = session;
        *self.binding.write() = binding;
        self.peer_samples.lock().clear();
        self.resolved.lock().clear();
        Ok(())
    }

    pub async fn stop(&self) {
        self.session().stop().await;
    }

    /// Limite de réception propre à un torrent qui tourne (`None` : aucune), en octets/s.
    /// Sans effet s'il est en pause : la répartition est recalculée chaque seconde.
    pub fn set_torrent_download_limit(&self, hash: &str, bps: Option<u64>) {
        let Ok(torrent) = self.handle(hash) else { return };
        if let Some(live) = torrent.live() {
            live.set_download_limit(bps.and_then(|b| NonZeroU32::new(b.min(u64::from(u32::MAX)) as u32)));
        }
    }

    pub fn set_limits(&self, limits: &SpeedLimits) {
        let config = limits_config(limits);
        let session = self.session();
        session.ratelimits.set_download_bps(config.download_bps);
        session.ratelimits.set_upload_bps(config.upload_bps);
    }

    pub fn listen_port(&self) -> Option<u16> {
        let session = self.session();
        session.listen_addr().map(|a| a.port()).or_else(|| session.announce_port())
    }

    /// Port ouvert sur la box par UPnP, s'il diffère du port d'écoute (la box n'accepte parfois
    /// qu'une plage de ports, ex. Freebox en IPv4 partagée).
    pub fn external_port(&self) -> Option<u16> {
        let session = self.session();
        session.announce_port().filter(|p| Some(*p) != session.listen_addr().map(|a| a.port()))
    }

    fn handle(&self, hash: &str) -> anyhow::Result<Arc<ManagedTorrent>> {
        let id = TorrentIdOrHash::parse(hash).map_err(|_| anyhow!("Torrent inconnu."))?;
        self.session().get(id).ok_or_else(|| anyhow!("Ce torrent n'est plus dans le moteur."))
    }
}
