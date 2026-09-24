//! Chef d'orchestre : réunit torrents et téléchargements directs dans une seule liste,
//! et gère la file d'attente, le mode jeu, l'espace disque, l'envoi sur R2 et les notifications.

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
use tauri_plugin_notification::NotificationExt;

use crate::danger;
use crate::direct::{self, Download, DownloadError, Progress};
use crate::engine::{Binding, Engine, EngineState, FileView, PeerView, Preview, ShareOptions, TorrentView};
use crate::r2::{self, R2};
use crate::secrets::{self, Secret};
use crate::settings::{GameAction, GameModeSwitch, Settings, SpeedLimits, VpnMode};
use crate::store::{
    Common, Delivery, DeliveryState, Dest, DirectEntry, Hold, Link, PendingTorrent, Priority, StoreData, TorrentEntry,
    now_ms,
};
use crate::system::{self, ProcessWatcher};
use crate::{dns, netif};

mod actions;
mod add;
mod delivery;
mod directs;
mod game;
mod overview;
mod power;
mod prefs;
mod queue;
mod stats;
mod tick;
mod transfer;
mod vpn;

pub use overview::format_bytes;
pub use power::PowerView;
pub use stats::TrafficStats;
use tick::snapshot_progress;
pub use transfer::TransferReport;
pub use vpn::{IpCheck, VpnView};

const MB: u64 = 1024 * 1024;
/// Dossier de travail des téléchargements destinés à R2 (dans le dossier de téléchargement).
const R2_WORK_DIR: &str = ".envoi-r2";
const LEGACY_1FICHIER: &str = "1fichier n'est plus pris en charge : supprime ce téléchargement de la liste.";

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
    /// Téléchargement direct terminé, pas encore partagé en torrent.
    pub can_share: bool,
    /// Fichier .torrent local d'origine, s'il existe encore (proposé à la suppression).
    pub source_file: Option<String>,
    /// Recherche des infos d'un magnet : secondes écoulées et pairs trouvés jusqu'ici.
    pub search_secs: Option<u64>,
    pub peers_found: Option<usize>,
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
    pub r2_ready: bool,
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
    pub r2_access_key_id: bool,
    pub r2_secret_access_key: bool,
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
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SecretsInput {
    /// `None` : inchangé ; chaîne vide : effacer.
    pub r2_access_key_id: Option<String>,
    pub r2_secret_access_key: Option<String>,
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
    /// Recherches de métadonnées et envois R2 en cours, par identifiant de téléchargement.
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    /// Torrents en cours d'ajout au moteur (à ne pas confondre avec des torrents disparus).
    adding: Mutex<HashSet<String>>,
    /// Recherches des infos d'un magnet en cours (par identifiant d'attente) : début et pairs trouvés.
    searches: Mutex<HashMap<String, Search>>,
    game: Mutex<GameState>,
    processes: Mutex<ProcessWatcher>,
    /// Reconstruit quand le VPN change (adresse source, ou blocage s'il est coupé).
    network: RwLock<direct::Network>,
    launch_links: Mutex<Vec<String>>,
    /// Évite deux bascules du VPN en même temps.
    vpn_switching: tokio::sync::Mutex<()>,
    /// Adresse du VPN à laquelle le moteur est relié.
    bound_ip: Mutex<Option<std::net::IpAddr>>,
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
    /// Les deux clés R2 sont enregistrées : tenu à jour ici plutôt que relu dans le Gestionnaire
    /// d'identification Windows à chaque rafraîchissement de la liste (chaque seconde).
    r2_keys_set: AtomicBool,
    /// Arrêt en cours (« Quitter », mise à jour, arrêt de Windows) : la boucle s'arrête et un
    /// second arrêt ne refait rien.
    closing: AtomicBool,
}

fn r2_keys_set() -> bool {
    secrets::is_set(Secret::R2AccessKeyId) && secrets::is_set(Secret::R2SecretAccessKey)
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

fn desired_binding(settings: &Settings) -> Binding {
    match settings.vpn.required_adapter() {
        None => Binding::Free,
        Some(name) => match netif::find(name) {
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
    };
    let resolver = dns::Resolver::new(mode);
    dns::install_global(resolver.clone());
    direct::Network::new(local_ip, offline, resolver)
}

impl Manager {
    pub async fn start(app: AppHandle, settings_path: PathBuf, data_dir: PathBuf) -> anyhow::Result<Arc<Self>> {
        let settings = Settings::load(&settings_path);
        let binding = desired_binding(&settings);
        tracing::info!(?binding, "liaison réseau au démarrage");
        let network = network_for(&binding);
        let bound_ip = match &binding {
            Binding::Vpn { adapter, .. } => netif::find(adapter).and_then(|a| a.source_ip()),
            _ => None,
        };
        let engine = Arc::new(Engine::start(&settings, &data_dir, binding, &network).await?);
        let store_path = data_dir.join("jobs.json");
        let mut data = StoreData::load(&store_path);
        remove_old_files(&data_dir.join(add::DROPPED_DIR), Duration::from_secs(7 * 24 * 3600));
        // 1fichier n'est plus pris en charge : les anciens téléchargements inachevés sont arrêtés
        let mut migrated = false;
        for entry in data.directs.iter_mut() {
            if entry.is_legacy_1fichier() && entry.common.finished_at.is_none() && entry.common.error.is_none() {
                entry.common.error = Some(LEGACY_1FICHIER.to_owned());
                entry.common.hold = None;
                migrated = true;
            }
        }
        if migrated {
            let _ = data.save(&store_path);
        }

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
            game: Mutex::new(GameState::default()),
            processes: Mutex::new(ProcessWatcher::new()),
            network: RwLock::new(network),
            launch_links: Mutex::new(Vec::new()),
            traffic: Mutex::new(Default::default()),
            power: Mutex::new(Default::default()),
            vpn_switching: tokio::sync::Mutex::new(()),
            bound_ip: Mutex::new(bound_ip),
            tunnel_attempt: Mutex::new(None),
            tunnel_error: Mutex::new(None),
            tunnel_manual_off: AtomicBool::new(false),
            tunnel_warning: Mutex::new(None),
            vpn_since: Mutex::new(Some(Instant::now())),
            tunnel_health_at: Mutex::new(None),
            sleep_blocked: AtomicBool::new(false),
            r2_keys_set: AtomicBool::new(r2_keys_set()),
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
        if let Err(err) = self.app.notification().builder().title(title).body(body).show() {
            tracing::warn!("notification impossible : {err}");
        }
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
