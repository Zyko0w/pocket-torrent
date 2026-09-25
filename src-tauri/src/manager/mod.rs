//! Chef d'orchestre : réunit torrents et téléchargements directs dans une seule liste,
//! et gère la file d'attente, le mode jeu, l'espace disque, l'envoi en ligne et les notifications.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::async_runtime::JoinHandle;

use crate::danger;
use crate::direct::{self, Download, DownloadError, Progress};
use crate::engine::{Binding, Engine, EngineState, FileView, PeerView, Preview, TorrentView};
use crate::secrets::{self, Secret};
use crate::settings::{GameAction, GameModeSwitch, Settings, SpeedLimits, VpnMode};
use crate::storage_to::StorageTo;
use crate::store::{
    Common, Delivery, DeliveryState, Dest, DirectEntry, Hold, Link, PendingTorrent, Priority, Scan, SendOptions,
    StoreData, TorrentEntry, now_ms,
};
use crate::system::{self, ProcessWatcher};
use crate::{dns, netif};

mod actions;
mod add;
mod bandwidth;
mod delivery;
mod directs;
mod feeds;
mod forwarding;
mod game;
mod history;
mod overview;
mod power;
mod prefs;
mod queue;
mod stats;
mod tick;
mod transfer;
mod vpn;

pub use add::ClipboardMagnet;
pub use feeds::{FeedPreviewItem, FeedStatus};
pub use forwarding::ForwardingView;
pub use history::SpeedHistory;
pub use overview::format_bytes;
pub use power::PowerView;
pub use stats::TrafficStats;
use tick::snapshot_progress;
pub use transfer::TransferReport;
pub use vpn::{IpCheck, VpnView};

const MB: u64 = 1024 * 1024;
/// Dossier de travail des téléchargements à envoyer en ligne (dans le dossier de téléchargement).
const UPLOAD_WORK_DIR: &str = ".envoi-en-ligne";

// ------------------------------------------------------------------ vues

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    /// torrent | http
    pub kind: &'static str,
    pub name: Option<String>,
    pub link: Option<String>,
    /// metadata | checking | connecting | queued | downloading | paused | seeding | uploading | done | error
    pub status: &'static str,
    pub hold: Option<Hold>,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub progress_bytes: u64,
    pub wanted_bytes: u64,
    pub uploaded_bytes: u64,
    pub down_speed: u64,
    pub up_speed: u64,
    pub eta_secs: Option<u64>,
    pub peers_live: u32,
    pub file_count: usize,
    pub selected_files: usize,
    pub category: Option<String>,
    pub priority: Priority,
    pub dest: Dest,
    pub queue_position: Option<usize>,
    pub added_at: u64,
    pub finished_at: Option<u64>,
    pub delivery: Option<Delivery>,
    pub limit_kbps: u32,
    /// Le torrent est dans le moteur : fichiers et pairs consultables.
    pub in_engine: bool,
    /// Fichier .torrent local d'origine, s'il existe encore (proposé à la suppression).
    pub source_file: Option<String>,
    /// Recherche des infos d'un magnet : secondes écoulées et pairs trouvés jusqu'ici.
    pub search_secs: Option<u64>,
    pub peers_found: Option<usize>,
    /// Analyse Microsoft Defender du téléchargement terminé.
    pub scan: Option<Scan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    pub active: bool,
    pub process: Option<String>,
    pub mode: GameModeSwitch,
    pub action: GameAction,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub jobs: Vec<JobView>,
    pub down_speed: u64,
    pub up_speed: u64,
    pub game: GameView,
    pub categories: Vec<CategoryView>,
    pub vpn: VpnView,
    pub power: PowerView,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AddOptions {
    pub category: Option<String>,
    pub priority: Priority,
    pub dest: Dest,
    pub limit_kbps: u32,
    pub only_files: Option<Vec<usize>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddReport {
    pub added: usize,
    pub failed: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSet {
    pub storage_to_token: bool,
    pub proxy_password: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub settings: Settings,
    pub secrets: SecretsSet,
    pub start_with_windows: bool,
    pub magnet_handler: bool,
    pub listen_port: Option<u16>,
    /// Port ouvert sur la box (UPnP) quand il diffère du port d'écoute.
    pub external_port: Option<u16>,
    /// Microsoft Defender (`MpCmdRun.exe`) trouvé sur ce PC, pour l'analyse optionnelle (§10.4).
    pub defender_available: bool,
    /// Réglages d'origine, pour les boutons « valeurs par défaut » de chaque section.
    pub defaults: Settings,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SecretsInput {
    /// `None` : inchangé ; chaîne vide : effacer.
    pub storage_to_token: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub settings: Settings,
    #[serde(default)]
    pub secrets: SecretsInput,
    pub start_with_windows: bool,
    pub magnet_handler: bool,
}

// ------------------------------------------------------------ état interne

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum JobKey {
    Torrent(String),
    Pending(String),
    Direct(String),
}

impl JobKey {
    fn parse(id: &str) -> anyhow::Result<Self> {
        match id.split_once(':') {
            Some(("t", hash)) => Ok(JobKey::Torrent(hash.to_owned())),
            Some(("p", id)) => Ok(JobKey::Pending(id.to_owned())),
            Some(("d", id)) => Ok(JobKey::Direct(id.to_owned())),
            _ => bail!("Téléchargement inconnu."),
        }
    }

    fn id(&self) -> String {
        match self {
            JobKey::Torrent(hash) => format!("t:{hash}"),
            JobKey::Pending(id) => format!("p:{id}"),
            JobKey::Direct(id) => format!("d:{id}"),
        }
    }
}

struct DirectRuntime {
    progress: Arc<Progress>,
    task: Option<JoinHandle<()>>,
    sample_at: Instant,
    sample_bytes: u64,
    speed: u64,
}

#[derive(Clone, Default)]
struct GameState {
    active: bool,
    process: Option<String>,
    /// Faux : à réévaluer au prochain passage (réglages modifiés).
    checked: bool,
}

#[derive(Clone, Copy)]
enum Notify {
    Done,
    Error,
}

/// Téléchargement en cours ou en attente, pour la file d'attente.
struct Slot {
    key: JobKey,
    priority: Priority,
    added_at: u64,
    forced: bool,
    folder: PathBuf,
    needed: u64,
    /// En cours (état « Live ») mais en dessous du seuil de vitesse lente : voir `QueueSettings::ignore_slow`.
    slow: bool,
}

/// Recherche des infos d'un magnet, affichée pendant « Recherche ».
#[derive(Clone)]
pub(crate) struct Search {
    started: Instant,
    peers_seen: Arc<std::sync::atomic::AtomicUsize>,
}

pub struct Manager {
    app: AppHandle,
    data_dir: PathBuf,
    pub engine: Arc<Engine>,
    settings: RwLock<Settings>,
    settings_path: PathBuf,
    data: Mutex<StoreData>,
    store_path: PathBuf,
    dirty: AtomicBool,
    last_save: Mutex<Instant>,
    directs: Mutex<HashMap<String, DirectRuntime>>,
    /// Recherches de métadonnées et envois en ligne en cours, par identifiant de téléchargement.
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    /// Torrents en cours d'ajout au moteur (à ne pas confondre avec des torrents disparus).
    adding: Mutex<HashSet<String>>,
    /// Flux RSS : mémoire des éléments vus et état de chaque flux (voir feeds.rs).
    feeds: Mutex<feeds::Feeds>,
    /// Débits des 5 dernières minutes (graphique), voir history.rs.
    history: Mutex<history::History>,
    /// Empreinte du dernier magnet du presse-papiers déjà proposé (proposé une seule fois).
    clipboard_offered: Mutex<Option<String>>,
    /// Recherches des infos d'un magnet en cours (par identifiant d'attente) : début et pairs trouvés.
    searches: Mutex<HashMap<String, Search>>,
    /// Répartition de la bande passante selon la priorité (voir bandwidth.rs).
    bandwidth: Mutex<bandwidth::BandwidthState>,
    /// Redirection de port obtenue du VPN (voir forwarding.rs).
    forwarding: Mutex<forwarding::Forwarding>,
    game: Mutex<GameState>,
    processes: Mutex<ProcessWatcher>,
    /// Reconstruit quand le VPN change (adresse source, ou blocage s'il est coupé).
    network: RwLock<direct::Network>,
    launch_links: Mutex<Vec<String>>,
    /// Évite deux bascules du VPN en même temps.
    vpn_switching: tokio::sync::Mutex<()>,
    /// Adresse du VPN à laquelle le moteur est relié.
    bound_ip: Mutex<Option<std::net::IpAddr>>,
    /// Mot de passe du proxy SOCKS5, gardé en mémoire (le Gestionnaire d'identification n'est
    /// relu qu'à l'enregistrement des réglages).
    proxy_password: Mutex<String>,
    /// Dernière tentative de reconnexion automatique du tunnel intégré, et son erreur.
    tunnel_attempt: Mutex<Option<Instant>>,
    tunnel_error: Mutex<Option<String>>,
    /// Tunnel déconnecté à la main : pas de reconnexion automatique.
    tunnel_manual_off: AtomicBool,
    /// Tunnel monté mais serveur muet (aucune poignée de main récente).
    tunnel_warning: Mutex<Option<String>>,
    /// Depuis quand le moteur est relié au VPN, et dernière vérification de santé du tunnel.
    vpn_since: Mutex<Option<Instant>>,
    tunnel_health_at: Mutex<Option<Instant>>,
    /// Vrai si Windows a reçu l'ordre de ne pas se mettre en veille (transfert actif).
    sleep_blocked: AtomicBool,
    /// Derniers compteurs d'octets lus, pour les statistiques cumulées.
    traffic: Mutex<stats::TrafficSample>,
    /// « Éteindre le PC quand tout est terminé » (session en cours seulement).
    power: Mutex<power::PowerState>,
    /// Arrêt en cours (« Quitter », mise à jour, arrêt de Windows) : la boucle s'arrête et un
    /// second arrêt ne refait rien.
    closing: AtomicBool,
}

/// Liaison réseau voulue selon les réglages et l'état des cartes réseau.
/// Supprime les fichiers de `dir` plus vieux que `max_age` (dossiers de travail de l'appli).
fn remove_old_files(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
        if old.is_some_and(|age| age > max_age) && entry.path().is_file() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `proxy_password` : lu une fois dans le Gestionnaire d'identification (voir `Manager::proxy_password`),
/// pas à chaque vérification.
fn desired_binding(settings: &Settings, proxy_password: &str) -> Binding {
    binding_with(settings, proxy_password, netif::find)
}

/// `find` : carte réseau d'après son nom (`netif::find`, remplacée dans les tests). VPN exigé
/// mais carte absente, éteinte ou sans adresse : `Offline`, rien ne sort (arrêt d'urgence).
fn binding_with(settings: &Settings, proxy_password: &str, find: impl Fn(&str) -> Option<netif::Adapter>) -> Binding {
    if settings.vpn.mode == VpnMode::Proxy {
        let vpn = &settings.vpn;
        return Binding::Proxy(crate::engine::ProxyUrl::new(
            &vpn.proxy_host,
            vpn.proxy_port,
            &vpn.proxy_user,
            proxy_password,
        ));
    }
    match settings.vpn.required_adapter() {
        None => Binding::Free,
        Some(name) => match find(name) {
            Some(adapter) if adapter.usable() => Binding::Vpn { adapter: adapter.name, ipv6: adapter.ipv6.is_some() },
            _ => Binding::Offline,
        },
    }
}

/// Réseau d'une liaison : DNS chiffré (installé aussi pour les trackers et la DHT) et clients HTTP.
fn network_for(binding: &Binding) -> direct::Network {
    let (mode, local_ip, offline) = match binding {
        Binding::Free => (dns::Mode::Free, None, false),
        Binding::Vpn { adapter, .. } => match netif::find(adapter).and_then(|a| a.source_ip()) {
            Some(ip) => (dns::Mode::Vpn(ip), Some(ip), false),
            None => (dns::Mode::Blocked, None, true),
        },
        Binding::Offline => (dns::Mode::Blocked, None, true),
        Binding::Proxy(proxy) => {
            // tout passe par le proxy, noms compris (socks5h) ; la DHT et les trackers UDP, seuls
            // utilisateurs du résolveur global, sont coupés dans ce mode
            let resolver = dns::Resolver::new(dns::Mode::Blocked);
            dns::install_global(resolver.clone());
            return direct::Network::with_proxy(None, false, Some(proxy.url("socks5h")), resolver);
        }
    };
    let resolver = dns::Resolver::new(mode);
    dns::install_global(resolver.clone());
    direct::Network::new(local_ip, offline, resolver)
}

impl Manager {
    pub async fn start(app: AppHandle, settings_path: PathBuf, data_dir: PathBuf) -> anyhow::Result<Arc<Self>> {
        let settings = Settings::load(&settings_path);
        let proxy_password = secrets::get(Secret::ProxyPassword).unwrap_or_default();
        let binding = desired_binding(&settings, &proxy_password);
        tracing::info!(?binding, "liaison réseau au démarrage");
        let network = network_for(&binding);
        let bound_ip = match &binding {
            Binding::Vpn { adapter, .. } => netif::find(adapter).and_then(|a| a.source_ip()),
            _ => None,
        };
        let engine = Arc::new(Engine::start(&settings, &data_dir, binding, &network).await?);
        let store_path = data_dir.join("jobs.json");
        let mut data = StoreData::load(&store_path);
        // analyse interrompue par la fermeture de l'appli : pas de résultat
        let StoreData { torrents, directs, .. } = &mut data;
        for common in torrents.values_mut().map(|t| &mut t.common).chain(directs.iter_mut().map(|d| &mut d.common)) {
            if common.scan == Some(Scan::Running) {
                common.scan = None;
            }
        }
        remove_old_files(&data_dir.join(add::DROPPED_DIR), Duration::from_secs(7 * 24 * 3600));

        let manager = Arc::new(Self {
            app,
            data_dir: data_dir.clone(),
            engine,
            settings: RwLock::new(settings),
            settings_path,
            data: Mutex::new(data),
            store_path,
            dirty: AtomicBool::new(false),
            last_save: Mutex::new(Instant::now()),
            directs: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            adding: Mutex::new(HashSet::new()),
            searches: Mutex::new(HashMap::new()),
            clipboard_offered: Mutex::new(None),
            history: Mutex::new(Default::default()),
            feeds: Mutex::new(Default::default()),
            bandwidth: Mutex::new(Default::default()),
            forwarding: Mutex::new(Default::default()),
            game: Mutex::new(GameState::default()),
            processes: Mutex::new(ProcessWatcher::new()),
            network: RwLock::new(network),
            launch_links: Mutex::new(Vec::new()),
            traffic: Mutex::new(Default::default()),
            power: Mutex::new(Default::default()),
            vpn_switching: tokio::sync::Mutex::new(()),
            bound_ip: Mutex::new(bound_ip),
            proxy_password: Mutex::new(proxy_password),
            tunnel_attempt: Mutex::new(None),
            tunnel_error: Mutex::new(None),
            tunnel_manual_off: AtomicBool::new(false),
            tunnel_warning: Mutex::new(None),
            vpn_since: Mutex::new(Some(Instant::now())),
            tunnel_health_at: Mutex::new(None),
            sleep_blocked: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });

        // reprise de ce qui était en cours à la fermeture
        let (pending, uploads) = {
            let data = manager.data.lock();
            let pending: Vec<String> =
                data.pending.iter().filter(|p| p.common.error.is_none()).map(|p| p.id.clone()).collect();
            let uploading = |c: &Common| c.delivery.as_ref().is_some_and(|d| d.state == DeliveryState::Uploading);
            let uploads: Vec<JobKey> = data
                .torrents
                .iter()
                .filter(|(_, t)| uploading(&t.common))
                .map(|(hash, _)| JobKey::Torrent(hash.clone()))
                .chain(data.directs.iter().filter(|d| uploading(&d.common)).map(|d| JobKey::Direct(d.id.clone())))
                .collect();
            (pending, uploads)
        };
        for id in pending {
            manager.spawn_pending(id);
        }
        for key in uploads {
            manager.start_delivery(key);
        }
        manager.apply_limits();
        manager.spawn_loop();
        Ok(manager)
    }

    /// Empêche (ou autorise de nouveau) la mise en veille de Windows selon les réglages
    /// et l'activité des transferts en cours.
    fn update_sleep_prevention(&self, views: &[TorrentView]) {
        let settings = self.settings.read();
        let active = settings.prevent_sleep
            && (views.iter().any(|v| v.down_speed > 0 || v.up_speed > 0)
                || self.directs.lock().values().any(|r| r.speed > 0));
        drop(settings);
        if self.sleep_blocked.swap(active, Ordering::Relaxed) != active {
            system::prevent_sleep(active);
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn save_if_due(&self, force: bool) {
        if !self.dirty.load(Ordering::Relaxed) {
            return;
        }
        let mut last = self.last_save.lock();
        if !force && last.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.dirty.store(false, Ordering::Relaxed);
        *last = Instant::now();
        let snapshot = self.data.lock().clone();
        if let Err(err) = snapshot.save(&self.store_path) {
            tracing::error!("sauvegarde de jobs.json impossible : {err:#}");
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    fn save_now(&self) {
        self.mark_dirty();
        self.save_if_due(true);
    }

    fn notify(&self, kind: Notify, title: &str, body: &str) {
        let enabled = {
            let settings = self.settings.read();
            match kind {
                Notify::Done => settings.notifications.done,
                Notify::Error => settings.notifications.error,
            }
        };
        if enabled {
            self.show_notification(title, body);
        }
    }

    fn show_notification(&self, title: &str, body: &str) {
        let (id, title, body) = (self.app.config().identifier.clone(), title.to_owned(), body.to_owned());
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(err) = crate::toast::show(&id, &title, &body) {
                tracing::warn!("notification non affichée : {err:#}");
            }
        });
    }

    pub async fn shutdown(&self) {
        if self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        for (_, task) in self.tasks.lock().drain() {
            task.abort();
        }
        let ids: Vec<String> = self.directs.lock().keys().cloned().collect();
        for id in ids {
            self.stop_direct(&id);
        }
        self.save_now();
        self.engine.stop().await;
    }

    /// `shutdown` depuis un thread hors du moteur (fenêtre principale, à l'arrêt de Windows),
    /// en `limit` au plus : Windows tue le processus s'il répond trop tard.
    pub fn shutdown_blocking(self: &Arc<Self>, limit: Duration) {
        tracing::info!("Windows s'arrête : arrêt propre du moteur");
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.shutdown().await;
            let _ = done_tx.send(());
        });
        if done_rx.recv_timeout(limit).is_err() {
            tracing::warn!("arrêt du moteur trop long, Windows ferme l'appli quand même");
        }
    }

    pub fn push_launch_link(&self, link: String) {
        self.launch_links.lock().push(link);
    }

    pub fn take_launch_links(&self) -> Vec<String> {
        std::mem::take(&mut *self.launch_links.lock())
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use crate::settings::WIREGUARD_ADAPTER;

    fn adapter(name: &str, up: bool, ip: bool) -> netif::Adapter {
        netif::Adapter {
            name: name.into(),
            description: String::new(),
            up,
            ipv4: ip.then(|| std::net::Ipv4Addr::new(10, 2, 0, 2)),
            ipv6: None,
            likely_vpn: true,
        }
    }

    /// Arrêt d'urgence : VPN exigé mais carte absente, éteinte ou sans adresse → rien ne sort.
    #[test]
    fn required_vpn_never_falls_back_to_the_normal_network() {
        let mut settings = Settings::default();
        assert!(matches!(binding_with(&settings, "", |_| None), Binding::Free));

        settings.vpn.mode = VpnMode::Adapter;
        settings.vpn.adapter = "NordLynx".into();
        assert!(matches!(binding_with(&settings, "", |_| None), Binding::Offline), "carte absente");
        assert!(matches!(binding_with(&settings, "", |n| Some(adapter(n, false, true))), Binding::Offline), "éteinte");
        assert!(matches!(binding_with(&settings, "", |n| Some(adapter(n, true, false))), Binding::Offline), "sans IP");
        assert!(matches!(
            binding_with(&settings, "", |n| Some(adapter(n, true, true))),
            Binding::Vpn { adapter, .. } if adapter == "NordLynx"
        ));
        // aucune carte choisie : jamais « n'importe quelle carte »
        settings.vpn.adapter.clear();
        assert!(matches!(binding_with(&settings, "", |_| None), Binding::Offline));

        settings.vpn.mode = VpnMode::Wireguard;
        let asked = std::cell::RefCell::new(String::new());
        let binding = binding_with(&settings, "", |n| {
            *asked.borrow_mut() = n.to_owned();
            None
        });
        assert!(matches!(binding, Binding::Offline));
        assert_eq!(*asked.borrow(), WIREGUARD_ADAPTER, "tunnel intégré : sa propre carte");

        settings.vpn.mode = VpnMode::Proxy;
        settings.vpn.proxy_host = "proxy.exemple.com".into();
        settings.vpn.proxy_port = 1080;
        assert!(matches!(binding_with(&settings, "secret", |_| None), Binding::Proxy(_)));
    }
}
