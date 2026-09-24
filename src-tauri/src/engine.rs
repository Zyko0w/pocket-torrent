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
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, CreateTorrentOptions, DhtSessionConfig,
    ListenerMode, ListenerOptions, ManagedTorrent, Session, SessionOptions, SessionPersistenceConfig,
    TorrentStatsState, generate_azereus_style,
};
use parking_lot::{Mutex, RwLock};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

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

#[derive(Clone)]
struct Resolved {
    preview: Preview,
    bytes: Bytes,
}

struct PeerSample {
    at: Instant,
    fetched: u64,
    uploaded: u64,
    down: f64,
    up: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Initializing,
    Live,
    Paused,
    Error,
}

pub struct TorrentView {
    pub info_hash: String,
    pub name: Option<String>,
    pub output_folder: PathBuf,
    pub state: EngineState,
    pub error: Option<String>,
    pub progress_bytes: u64,
    /// Taille des fichiers sélectionnés (toute la taille si aucun choix).
    pub wanted_bytes: u64,
    pub uploaded_bytes: u64,
    pub finished: bool,
    pub down_speed: u64,
    pub up_speed: u64,
    pub peers_live: u32,
    pub file_count: usize,
    pub selected_files: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub index: usize,
    pub path: String,
    pub length: u64,
    pub included: bool,
    pub progress_bytes: u64,
    /// Raison de se méfier de ce fichier (extension dangereuse, nom trompeur) : voir
    /// `Manager::flag_danger`, qui remplit ce champ à partir des réglages de sécurité.
    pub danger: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub info_hash: String,
    pub name: Option<String>,
    pub total_bytes: u64,
    pub files: Vec<FileView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub address: String,
    pub client: Option<String>,
    pub connection: Option<String>,
    pub downloaded: u64,
    pub uploaded: u64,
    pub down_speed: u64,
    pub up_speed: u64,
}

/// Réglages de création d'un torrent (§5, §6.1 du plan d'upgrade), fournis par l'interface.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareOptions {
    /// Trackers ajoutés à ceux par défaut ; ignorés si `is_private` (un torrent privé n'a
    /// rien à faire sur des trackers publics).
    pub extra_trackers: Vec<String>,
    /// Taille de pièce en Kio (puissance de deux), 0 = automatique.
    pub piece_length_kib: u32,
    /// BEP 27 : coupe DHT, PEX et découverte locale pour ce torrent (chez les clients qui le respectent).
    pub is_private: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceMap {
    pub total_pieces: u32,
    /// Empaqueté 8 pièces par octet, poids fort en premier ; 1 = pièce présente.
    pub bits: Vec<u8>,
}

pub fn limits_config(limits: &SpeedLimits) -> LimitsConfig {
    LimitsConfig {
        download_bps: NonZeroU32::new(limits.down_kbps.saturating_mul(1024)),
        upload_bps: NonZeroU32::new(limits.up_kbps.saturating_mul(1024)),
    }
}

/// Fichiers choisis sous forme de table (un booléen par fichier du torrent), `None` si tous le
/// sont. Chercher chaque fichier dans la liste serait quadratique : des secondes de calcul, à
/// chaque seconde, pour un torrent de dizaines de milliers de fichiers.
fn selection_mask(only_files: Option<&[usize]>, file_count: usize) -> Option<Vec<bool>> {
    only_files.map(|list| {
        let mut mask = vec![false; file_count];
        for &index in list {
            if let Some(slot) = mask.get_mut(index) {
                *slot = true;
            }
        }
        mask
    })
}

fn is_selected(mask: Option<&[bool]>, index: usize) -> bool {
    mask.is_none_or(|m| m.get(index).copied().unwrap_or(false))
}

/// Pairs indiqués directement dans un magnet (paramètres `x.pe=ip:port`).
fn magnet_peers(link: &str) -> Option<Vec<SocketAddr>> {
    let url = url::Url::parse(link.trim()).ok().filter(|u| u.scheme() == "magnet")?;
    let peers: Vec<SocketAddr> =
        url.query_pairs().filter(|(key, _)| key == "x.pe").filter_map(|(_, value)| value.parse().ok()).collect();
    (!peers.is_empty()).then_some(peers)
}

/// Magnet, lien http(s) vers un .torrent, ou chemin d'un fichier .torrent local.
fn add_source(link: &str) -> anyhow::Result<AddTorrent<'static>> {
    let link = link.trim();
    if link.starts_with("magnet:") || link.starts_with("http://") || link.starts_with("https://") {
        return Ok(AddTorrent::from_url(link.to_owned()));
    }
    if Path::new(link).is_file() {
        // même plafond que pour un .torrent téléchargé : le fichier est lu en entier en mémoire
        if std::fs::metadata(link).is_ok_and(|m| m.len() > librqbit::MAX_TORRENT_FILE_BYTES as u64) {
            bail!("Fichier .torrent trop gros pour être un vrai torrent.");
        }
        return AddTorrent::from_local_filename(link);
    }
    if looks_like_path(link) {
        bail!("Fichier .torrent introuvable (déplacé ou supprimé ?) : {link}");
    }
    bail!("Lien non reconnu : colle un magnet, un lien http(s) ou choisis un fichier .torrent.")
}

/// Chemin de fichier plutôt que lien (« C:\… », « \\serveur\… », « /… » ou « ….torrent »).
pub fn looks_like_path(link: &str) -> bool {
    let bytes = link.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || link.starts_with('\\') || link.starts_with('/') || link.to_lowercase().ends_with(".torrent")
}

/// Temps laissé aux pairs pour transmettre les infos d'un magnet avant d'abandonner (l'utilisateur
/// peut réessayer) : sans limite, un magnet sans source resterait « Recherche du nom… » à vie.
const MAGNET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Traduit les erreurs du moteur que l'utilisateur peut corriger lui-même.
fn explain_add_error(err: anyhow::Error, binding: &Binding) -> anyhow::Error {
    let text = format!("{err:#}");
    if text.contains("no known way to resolve peers") {
        return match binding {
            Binding::Offline => anyhow!("VPN coupé : impossible de chercher ce magnet tant que le VPN n'est pas revenu."),
            _ => anyhow!(
                "Impossible de chercher ce magnet : la DHT est coupée (mode discret ou confidentialité renforcée, \
                 Réglages › Confidentialité) et le lien ne contient aucun tracker HTTPS. Désactive ce mode ou \
                 utilise un fichier .torrent."
            ),
        };
    }
    if text.contains("input address stream exhausted") {
        return anyhow!("Aucun pair n'a transmis les infos de ce torrent. Réessaie plus tard.");
    }
    err
}

/// Par où le moteur a le droit de communiquer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Sans VPN : réseau normal.
    Free,
    /// Tout passe par cette carte réseau (VPN), rien ailleurs.
    Vpn { adapter: String, ipv6: bool },
    /// VPN exigé mais coupé : aucune communication possible.
    Offline,
}

/// TCP seul par défaut ; µTP en plus si activé dans les réglages (voir `Settings::utp_enabled`).
fn listener_mode(settings: &Settings) -> ListenerMode {
    if settings.utp_enabled { ListenerMode::TcpAndUtp } else { ListenerMode::TcpOnly }
}

fn encryption_mode(settings: &Settings) -> EncryptionMode {
    match settings.privacy.encryption {
        EncryptionLevel::Off => EncryptionMode::Disabled,
        EncryptionLevel::Prefer => EncryptionMode::Prefer,
        EncryptionLevel::Require => EncryptionMode::Require,
    }
}

async fn create_session(
    settings: &Settings,
    data_dir: &Path,
    binding: &Binding,
    network: &Network,
) -> anyhow::Result<Arc<Session>> {
    std::fs::create_dir_all(&settings.download_dir).ok();
    let hide_announces = settings.privacy.hide_announces || settings.privacy.hardened_mode;
    // annonces discrètes : seuls les trackers HTTPS (les annonces HTTP et UDP sont lisibles en clair)
    librqbit_tracker_comms::set_https_trackers_only(hide_announces);

    let mut options = match binding {
        Binding::Free => free_options(settings, data_dir),
        Binding::Vpn { adapter, ipv6 } => SessionOptions {
            bind_device_name: Some(adapter.clone()),
            // sans IPv6 dans le tunnel, rien ne doit tenter l'IPv6 hors du tunnel
            ipv4_only: !ipv6,
            // la découverte locale et l'UPnP parlent au réseau local, pas au VPN
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: listener_mode(settings),
                listen_addr: SocketAddr::from((Ipv6Addr::UNSPECIFIED, settings.torrent_port)),
                enable_upnp_port_forwarding: false,
                ..Default::default()
            }),
            ..free_options(settings, data_dir)
        },
        Binding::Offline => SessionOptions {
            dht: None,
            listen: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            // proxy injoignable : toute tentative de connexion échoue au lieu de sortir sans VPN
            connect: Some(ConnectionOptions {
                proxy_url: Some("socks5://127.0.0.1:9".into()),
                enable_tcp: false,
                peer_opts: None,
                encryption: EncryptionMode::Require,
            }),
            ..free_options(settings, data_dir)
        },
    };
    if hide_announces {
        // la DHT et la découverte locale échangent des messages BitTorrent non chiffrés
        options.dht = None;
        options.disable_local_service_discovery = true;
        // une source web en http:// serait lisible en clair, comme un tracker HTTP
        options.webseeds_https_only = true;
    }
    if settings.privacy.hardened_mode {
        if let Some(listen) = options.listen.as_mut() {
            // l'UPnP parle au routeur en clair et annonce un port ouvert vers l'extérieur
            listen.enable_upnp_port_forwarding = false;
        }
        // fait passer ce PC pour un client courant plutôt que Pocket Torrent
        options.peer_id = Some(generate_azereus_style(*b"UT", (3, 5, 5, 0)));
        options.client_name_and_version = Some("uTorrent/3.5.5".to_owned());
        if let Some(connect) = options.connect.as_mut() {
            connect.peer_opts.get_or_insert_with(Default::default).disable_yourip = true;
        }
    }
    options.webseed_reqwest_client = Some(network.webseed_client(options.webseeds_https_only));
    options.reqwest_client = Some(network.http.clone());
    Session::new_with_opts(settings.download_dir.clone(), options)
        .await
        .context("impossible de démarrer le moteur torrent")
}

/// Chemin local ou URL http(s)/file pour `SessionOptions::blocklist_url` (§9.7). Efficacité
/// faible en pratique (voir l'avertissement affiché dans les réglages), gardée à la demande
/// du plan d'upgrade.
fn blocklist_url(settings: &Settings) -> Option<String> {
    if !settings.blocklist.enabled {
        return None;
    }
    let source = settings.blocklist.source.trim();
    if source.is_empty() {
        return None;
    }
    if source.starts_with("http://") || source.starts_with("https://") || source.starts_with("file://") {
        return Some(source.to_owned());
    }
    url::Url::from_file_path(source).ok().map(|u| u.to_string())
}

fn free_options(settings: &Settings, data_dir: &Path) -> SessionOptions {
    SessionOptions {
        blocklist_url: blocklist_url(settings),
        peer_limit: (settings.max_peers_per_torrent > 0).then_some(settings.max_peers_per_torrent as usize),
        upload_slots: (settings.upload_slots > 0).then_some(settings.upload_slots as usize),
        upnp_description: Some("Pocket Torrent".into()),
        // reprise automatique des téléchargements au prochain lancement
        persistence: Some(SessionPersistenceConfig::Json { folder: Some(data_dir.join("session")) }),
        fastresume: true,
        dht: Some(DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                config_filename: Some(data_dir.join("dht.json")),
                dump_interval: None,
            }),
            ..Default::default()
        }),
        listen: Some(ListenerOptions {
            // µTP désactivé par défaut : le module µTP de librqbit reste jeune. Réglable (§9.3).
            mode: listener_mode(settings),
            listen_addr: SocketAddr::from((Ipv6Addr::UNSPECIFIED, settings.torrent_port)),
            enable_upnp_port_forwarding: true,
            ..Default::default()
        }),
        connect: Some(ConnectionOptions { encryption: encryption_mode(settings), ..Default::default() }),
        trackers: EXTRA_TRACKERS.iter().filter_map(|t| t.parse().ok()).collect(),
        ratelimits: limits_config(&settings.limits),
        ..Default::default()
    }
}

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
        let session = create_session(settings, &self.data_dir, &binding, network).await?;
        *self.session.write() = session;
        *self.binding.write() = binding;
        self.peer_samples.lock().clear();
        self.resolved.lock().clear();
        Ok(())
    }

    pub async fn stop(&self) {
        self.session().stop().await;
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

impl Engine {
    // ------------------------------------------------------------------ lecture

    /// Octets reçus et envoyés par tous les torrents depuis le démarrage du moteur
    /// (repartent de zéro à chaque redémarrage de session).
    pub fn traffic_counters(&self) -> (u64, u64) {
        let counters = self.session().stats_snapshot().counters;
        (counters.fetched_bytes, counters.uploaded_bytes)
    }

    pub fn torrents(&self) -> Vec<TorrentView> {
        self.session().with_torrents(|torrents| torrents.map(|(_, torrent)| view_of(torrent)).collect())
    }

    /// Fichiers du torrent, dans l'ordre de priorité de téléchargement courant (§4.1/§5 :
    /// avant, ordre alphabétique figé ; voir `move_file_priority`).
    pub fn files(&self, hash: &str) -> anyhow::Result<Vec<FileView>> {
        let torrent = self.handle(hash)?;
        let only_files = torrent.only_files();
        let progress = torrent.stats().file_progress;
        torrent.with_metadata(|meta| {
            let mask = selection_mask(only_files.as_deref(), meta.file_infos.len());
            let order = torrent
                .file_priorities()
                .filter(|o| o.len() == meta.file_infos.len())
                .unwrap_or_else(|| (0..meta.file_infos.len()).collect());
            order
                .into_iter()
                .filter_map(|index| meta.file_infos.get(index).map(|file| (index, file)))
                .filter(|(_, file)| !file.attrs.padding)
                .map(|(index, file)| FileView {
                    index,
                    path: file.relative_filename.to_string_lossy().replace('\\', "/"),
                    length: file.len,
                    included: is_selected(mask.as_deref(), index),
                    progress_bytes: progress.get(index).copied().unwrap_or(0),
                    danger: None,
                })
                .collect()
        })
    }

    /// Remet l'ordre de priorité enregistré si le torrent tourne avec un autre (après une pause
    /// ou un redémarrage, le moteur repart de l'ordre alphabétique). Sans effet sinon.
    /// Erreur seulement si l'ordre ne correspond pas aux fichiers du torrent.
    pub fn restore_file_priorities(&self, hash: &str, order: &[usize]) -> anyhow::Result<()> {
        let Ok(torrent) = self.handle(hash) else { return Ok(()) };
        let Some(current) = torrent.file_priorities() else { return Ok(()) };
        if current == order {
            return Ok(());
        }
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        anyhow::ensure!(
            sorted.iter().copied().eq(0..current.len()),
            "ordre enregistré incompatible avec les fichiers du torrent"
        );
        if let Err(err) = self.session().update_file_priorities(&torrent, order.to_vec()) {
            // torrent mis en pause entre-temps : on réessaiera à sa reprise
            tracing::debug!("ordre des fichiers de {hash} pas encore réappliqué : {err:#}");
        }
        Ok(())
    }

    /// Fait remonter (`up`) ou descendre le fichier `index` d'un cran dans la priorité de
    /// téléchargement et renvoie le nouvel ordre. Sans effet si le fichier est déjà en
    /// première/dernière position ; erreur si le torrent n'est pas en cours de téléchargement
    /// (la priorité n'existe que là, §4.1).
    pub async fn move_file_priority(&self, hash: &str, index: usize, up: bool) -> anyhow::Result<Vec<usize>> {
        let torrent = self.handle(hash)?;
        let (file_count, is_padding) = torrent.with_metadata(|m| {
            (m.file_infos.len(), m.file_infos.iter().map(|f| f.attrs.padding).collect::<Vec<_>>())
        })?;
        let mut order =
            torrent.file_priorities().filter(|o| o.len() == file_count).unwrap_or_else(|| (0..file_count).collect());
        let pos = order.iter().position(|&i| i == index).context("fichier introuvable dans l'ordre de priorité")?;

        let step: isize = if up { -1 } else { 1 };
        let mut other = pos as isize + step;
        while (0..order.len() as isize).contains(&other) && is_padding[order[other as usize]] {
            other += step;
        }
        if !(0..order.len() as isize).contains(&other) {
            return Ok(order); // déjà en première/dernière position visible
        }

        order.swap(pos, other as usize);
        self.session().update_file_priorities(&torrent, order.clone())?;
        Ok(order)
    }

    pub fn peers(&self, hash: &str) -> anyhow::Result<Vec<PeerView>> {
        let torrent = self.handle(hash)?;
        let Some(live) = torrent.live() else {
            return Ok(Vec::new());
        };
        let snapshot = live.per_peer_stats_snapshot(Default::default());
        let now = Instant::now();
        let mut samples = self.peer_samples.lock();

        let mut peers: Vec<PeerView> = snapshot
            .peers
            .into_iter()
            .map(|(address, stats)| {
                let fetched = stats.counters.fetched_bytes;
                let uploaded = stats.counters.uploaded_bytes;
                let key = (hash.to_owned(), address.clone());
                let previous = samples.get(&key);
                let refresh = previous.is_none_or(|prev| now.duration_since(prev.at).as_secs_f64() >= 0.5);
                let (down, up) = match previous {
                    Some(prev) if refresh => {
                        let elapsed = now.duration_since(prev.at).as_secs_f64();
                        (
                            fetched.saturating_sub(prev.fetched) as f64 / elapsed,
                            uploaded.saturating_sub(prev.uploaded) as f64 / elapsed,
                        )
                    }
                    Some(prev) => (prev.down, prev.up),
                    None => (0.0, 0.0),
                };
                if refresh {
                    samples.insert(key, PeerSample { at: now, fetched, uploaded, down, up });
                }
                PeerView {
                    address,
                    client: stats.client_name,
                    connection: stats.conn_kind.map(|kind| kind.to_string()),
                    downloaded: fetched,
                    uploaded,
                    down_speed: down as u64,
                    up_speed: up as u64,
                }
            })
            .collect();

        // oublie les pairs déconnectés de ce torrent
        samples.retain(|(torrent, address), _| torrent != hash || peers.iter().any(|p| &p.address == address));
        peers.sort_by(|a, b| {
            (b.down_speed + b.up_speed).cmp(&(a.down_speed + a.up_speed)).then(b.downloaded.cmp(&a.downloaded))
        });
        peers.truncate(200);
        Ok(peers)
    }

    /// Dossier (ou fichier seul) du torrent sur le disque.
    pub fn content_path(&self, hash: &str) -> anyhow::Result<PathBuf> {
        let torrent = self.handle(hash)?;
        let base = torrent.output_folder().to_path_buf();
        let single = torrent
            .with_metadata(|meta| {
                let files: Vec<_> = meta.file_infos.iter().filter(|f| !f.attrs.padding).collect();
                (files.len() == 1).then(|| base.join(&files[0].relative_filename))
            })
            .ok()
            .flatten();
        Ok(single.filter(|p| p.exists()).unwrap_or(base))
    }

    /// Carte des pièces d'un torrent (§5) : un octet par groupe de 8 pièces, bit à 1 (du
    /// poids fort au poids faible) si la pièce est présente sur le disque — même format que
    /// le message `Bitfield` du protocole (BEP 3).
    pub fn piece_map(&self, hash: &str) -> anyhow::Result<PieceMap> {
        let torrent = self.handle(hash)?;
        let total_pieces = torrent.with_metadata(|meta| meta.info.lengths().total_pieces())?;
        let bits = torrent.with_chunk_tracker(|ct| ct.get_have_pieces().as_bytes().to_vec())?;
        Ok(PieceMap { total_pieces, bits })
    }

    /// Chemin sur le disque du fichier `index` d'un torrent (streaming, §7.6). `index` est
    /// celui du torrent, comme dans `FileView`, pas une position dans une liste filtrée.
    pub fn file_path(&self, hash: &str, index: usize) -> anyhow::Result<PathBuf> {
        let torrent = self.handle(hash)?;
        let base = torrent.output_folder().to_path_buf();
        let rel = torrent
            .with_metadata(|meta| meta.file_infos.get(index).map(|f| f.relative_filename.clone()))?
            .ok_or_else(|| anyhow!("Fichier introuvable."))?;
        // jamais hors du dossier du torrent (le serveur de streaming sert ce chemin)
        if !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            bail!("Fichier introuvable.");
        }
        Ok(base.join(rel))
    }

    /// Fichiers choisis du torrent : chemin relatif, chemin sur le disque et taille.
    pub fn selected_file_paths(&self, hash: &str) -> anyhow::Result<Vec<(String, PathBuf, u64)>> {
        let torrent = self.handle(hash)?;
        let base = torrent.output_folder().to_path_buf();
        let only_files = torrent.only_files();
        torrent.with_metadata(|meta| {
            let mask = selection_mask(only_files.as_deref(), meta.file_infos.len());
            meta.file_infos
                .iter()
                .enumerate()
                .filter(|(index, file)| !file.attrs.padding && is_selected(mask.as_deref(), *index))
                // jamais un fichier hors du dossier du torrent (l'envoi R2 lirait n'importe quoi)
                .filter(|(_, file)| {
                    file.relative_filename.components().all(|c| matches!(c, std::path::Component::Normal(_)))
                })
                .map(|(_, file)| {
                    let rel = file.relative_filename.to_string_lossy().replace('\\', "/");
                    (rel, base.join(&file.relative_filename), file.len)
                })
                .collect()
        })
    }

    /// URLs des trackers configurés pour ce torrent, triées. Pas d'état par tracker (pairs,
    /// erreur, prochaine annonce) : librqbit ne le fait pas encore remonter.
    pub fn trackers(&self, hash: &str) -> anyhow::Result<Vec<String>> {
        let torrent = self.handle(hash)?;
        let mut trackers: Vec<String> = torrent.shared().trackers.iter().map(|t| t.to_string()).collect();
        trackers.sort();
        Ok(trackers)
    }

    /// Fichier .torrent d'origine et fichiers choisis, pour l'export de la liste.
    pub fn export_info(&self, hash: &str) -> Option<(Bytes, Option<Vec<usize>>)> {
        let torrent = self.handle(hash).ok()?;
        let bytes = torrent.with_metadata(|meta| meta.torrent_bytes.clone()).ok()?;
        Some((bytes, torrent.only_files()))
    }

    pub fn magnet(&self, hash: &str) -> anyhow::Result<String> {
        let torrent = self.handle(hash)?;
        let mut link = format!("magnet:?xt=urn:btih:{}", torrent.info_hash().as_string());
        if let Some(name) = torrent.name() {
            link.push_str(&format!("&dn={}", utf8_percent_encode(&name, MAGNET_SET)));
        }
        let mut trackers: Vec<String> = torrent.shared().trackers.iter().map(|t| t.to_string()).collect();
        trackers.sort();
        for tracker in trackers.iter().take(10) {
            link.push_str(&format!("&tr={}", utf8_percent_encode(tracker, MAGNET_SET)));
        }
        Ok(link)
    }

    // ---------------------------------------------------------------- actions

    /// Récupère le nom et la liste des fichiers sans lancer le téléchargement.
    pub async fn preview(&self, link: &str) -> anyhow::Result<Preview> {
        self.preview_counting(link, None).await
    }

    /// Comme `preview`, en comptant dans `peers_seen` les pairs trouvés pendant la recherche des
    /// infos d'un magnet (affichés pendant « Recherche »).
    pub async fn preview_counting(
        &self,
        link: &str,
        peers_seen: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> anyhow::Result<Preview> {
        let key = link.trim().to_owned();
        if let Some(resolved) = self.resolved.lock().get(&key) {
            return Ok(resolved.preview.clone());
        }
        let session = self.session();
        let lookup = session.add_torrent(
            add_source(link)?,
            Some(AddTorrentOptions {
                list_only: true,
                initial_peers: magnet_peers(link),
                magnet_peers_seen: peers_seen,
                ..Default::default()
            }),
        );
        let response = match tokio::time::timeout(MAGNET_TIMEOUT, lookup).await {
            Ok(result) => result.map_err(|err| explain_add_error(err, &self.binding())),
            Err(_) => Err(anyhow!(
                "Aucun pair n'a transmis les infos de ce torrent en {} min (torrent sans sources ?). Réessaie plus tard.",
                MAGNET_TIMEOUT.as_secs() / 60
            )),
        }
        .context("impossible de lire ce torrent")?;
        match response {
            AddTorrentResponse::ListOnly(list) => {
                let files = list
                    .info
                    .iter_file_details_ext()
                    .enumerate()
                    .filter(|(_, file)| !file.details.attrs().padding)
                    .map(|(index, file)| FileView {
                        index,
                        path: file.details.filename.to_pathbuf().to_string_lossy().replace('\\', "/"),
                        length: file.details.len,
                        included: true,
                        progress_bytes: 0,
                        danger: None,
                    })
                    .collect();
                let preview = Preview {
                    info_hash: list.info_hash.as_string(),
                    name: list.info.name().map(|n| n.into_owned()),
                    total_bytes: list.info.lengths().total_length(),
                    files,
                };
                let mut cache = self.resolved.lock();
                if cache.len() > 50 {
                    cache.clear();
                }
                cache.insert(key, Resolved { preview: preview.clone(), bytes: list.torrent_bytes });
                Ok(preview)
            }
            AddTorrentResponse::AlreadyManaged(..) => bail!("Ce torrent est déjà dans la liste."),
            AddTorrentResponse::Added(..) => bail!("Réponse inattendue du moteur torrent."),
        }
    }

    /// Ajoute un torrent (résolu au besoin). Un torrent à plusieurs fichiers
    /// est rangé dans son propre sous-dossier de `base_folder`.
    pub async fn add(
        &self,
        link: &str,
        base_folder: &Path,
        only_files: Option<Vec<usize>>,
        paused: bool,
    ) -> anyhow::Result<String> {
        if only_files.as_ref().is_some_and(|files| files.is_empty()) {
            bail!("Choisis au moins un fichier.");
        }
        self.preview(link).await?;
        let resolved = self.resolved.lock().remove(link.trim()).ok_or_else(|| anyhow!("Torrent introuvable."))?;
        let folder = if resolved.preview.files.len() > 1 {
            let name = resolved.preview.name.clone().unwrap_or_else(|| resolved.preview.info_hash.clone());
            base_folder.join(safe_file_name(&name))
        } else {
            base_folder.to_path_buf()
        };
        let response = self
            .session()
            .add_torrent(
                AddTorrent::from_bytes(resolved.bytes),
                Some(AddTorrentOptions {
                    paused,
                    only_files,
                    output_folder: Some(folder.to_string_lossy().into_owned()),
                    initial_peers: magnet_peers(link),
                    // permet de reprendre des fichiers partiellement présents sur le disque
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("impossible d'ajouter ce torrent")?;
        match response {
            AddTorrentResponse::Added(_, handle) => Ok(handle.info_hash().as_string()),
            AddTorrentResponse::AlreadyManaged(..) => bail!("Ce torrent est déjà dans la liste."),
            AddTorrentResponse::ListOnly(_) => bail!("Réponse inattendue du moteur torrent."),
        }
    }

    /// Crée un torrent à partir de fichiers du disque et commence à les partager.
    /// Le fichier .torrent est aussi enregistré dans `torrents-crees`.
    pub async fn share(&self, path: &Path, share: &ShareOptions) -> anyhow::Result<(String, PathBuf)> {
        // un torrent privé n'a rien à faire sur des trackers publics
        let mut trackers: Vec<String> =
            if share.is_private { Vec::new() } else { EXTRA_TRACKERS.iter().map(|t| (*t).to_owned()).collect() };
        for tracker in &share.extra_trackers {
            let tracker = tracker.trim();
            if !tracker.is_empty() && !trackers.iter().any(|t| t == tracker) {
                trackers.push(tracker.to_owned());
            }
        }
        let options = CreateTorrentOptions {
            trackers,
            piece_length: (share.piece_length_kib > 0).then(|| share.piece_length_kib.saturating_mul(1024)),
            is_private: share.is_private,
            ..Default::default()
        };
        let (created, handle) = self
            .session()
            .create_and_serve_torrent(path, options)
            .await
            .map_err(|err| anyhow!("création du torrent impossible : {err}"))?;
        let hash = handle.info_hash().as_string();
        let folder = self.data_dir.join("torrents-crees");
        std::fs::create_dir_all(&folder)?;
        let name = path.file_name().map_or_else(|| "partage".into(), |n| n.to_string_lossy().into_owned());
        let torrent_path = folder.join(format!("{}-{}.torrent", safe_file_name(&name), &hash[..8]));
        std::fs::write(&torrent_path, created.as_bytes()?)?;
        Ok((hash, torrent_path))
    }

    pub async fn pause(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        if !torrent.is_paused() {
            self.session().pause(&torrent).await?;
        }
        Ok(())
    }

    pub async fn resume(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        if torrent.is_paused() {
            self.session().unpause(&torrent).await?;
        }
        Ok(())
    }

    /// Retire le torrent du moteur ; avec `delete_files`, efface aussi ses fichiers et dossiers vides.
    pub async fn delete(&self, hash: &str, delete_files: bool) -> anyhow::Result<()> {
        let id = TorrentIdOrHash::parse(hash)?;
        self.session().delete(id, delete_files).await?;
        self.peer_samples.lock().retain(|(torrent, _), _| torrent != hash);
        Ok(())
    }

    /// Revérifie un torrent depuis le disque (§5, « revérification forcée ») : le retire du
    /// moteur (fichiers conservés) puis le rajoute avec les mêmes métadonnées, fichiers
    /// choisis et dossier — ce qui force une nouvelle vérification complète de ce qui est
    /// déjà présent sur le disque, sans retélécharger les métadonnées auprès des pairs.
    pub async fn recheck(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        let bytes = torrent.with_metadata(|meta| meta.torrent_bytes.clone())?;
        let only_files = torrent.only_files();
        let output_folder = torrent.output_folder().to_path_buf();
        let paused = torrent.is_paused();
        let id = TorrentIdOrHash::parse(hash).map_err(|_| anyhow!("Torrent inconnu."))?;
        self.session().delete(id, false).await?;
        self.session()
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    paused,
                    only_files,
                    output_folder: Some(output_folder.to_string_lossy().into_owned()),
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("impossible de relancer la vérification")?;
        Ok(())
    }

    pub async fn set_files(&self, hash: &str, files: Vec<usize>) -> anyhow::Result<()> {
        let files: HashSet<usize> = files.into_iter().collect();
        if files.is_empty() {
            bail!("Choisis au moins un fichier.");
        }
        let torrent = self.handle(hash)?;
        self.session().update_only_files(&torrent, &files).await
    }
}

fn view_of(torrent: &Arc<ManagedTorrent>) -> TorrentView {
    let stats = torrent.stats();
    let only_files = torrent.only_files();
    let (file_count, wanted_bytes) = torrent
        .with_metadata(|meta| {
            let mask = selection_mask(only_files.as_deref(), meta.file_infos.len());
            let mut count = 0;
            let mut wanted = 0;
            for (index, file) in meta.file_infos.iter().enumerate() {
                if file.attrs.padding {
                    continue;
                }
                count += 1;
                if is_selected(mask.as_deref(), index) {
                    wanted += file.len;
                }
            }
            (count, wanted)
        })
        .unwrap_or((0, stats.total_bytes));

    let (down_speed, up_speed, peers_live) = match &stats.live {
        Some(live) => (live.download_speed.as_bytes(), live.upload_speed.as_bytes(), live.snapshot.peer_stats.live),
        None => (0, 0, 0),
    };
    // les morceaux à cheval sur des fichiers non choisis peuvent dépasser la taille voulue
    let progress = if wanted_bytes > 0 { stats.progress_bytes.min(wanted_bytes) } else { stats.progress_bytes };
    let state = match stats.state {
        TorrentStatsState::Initializing { .. } => EngineState::Initializing,
        TorrentStatsState::Paused => EngineState::Paused,
        TorrentStatsState::Error => EngineState::Error,
        TorrentStatsState::Live => EngineState::Live,
    };

    TorrentView {
        info_hash: torrent.info_hash().as_string(),
        name: torrent.name(),
        output_folder: torrent.output_folder().to_path_buf(),
        state,
        error: stats.error.clone(),
        progress_bytes: progress,
        wanted_bytes,
        uploaded_bytes: stats.uploaded_bytes,
        finished: stats.finished,
        down_speed,
        up_speed,
        peers_live,
        file_count,
        selected_files: only_files.as_ref().map_or(file_count, |list| list.len()),
    }
}

/// Essais de bout en bout dans un bac à sable : dossiers temporaires dédiés (jamais le vrai
/// dossier de téléchargement ni les données de l'appli) et moteur hors ligne (`Binding::Offline` :
/// ni DHT, ni trackers, ni écoute, connexions vers un proxy injoignable).
#[cfg(test)]
mod sandbox_tests {
    use super::*;

    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        async fn engine(&self) -> Engine {
            let settings = Settings { download_dir: self.root.join("telechargements"), ..Default::default() };
            let network = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
            Engine::start(&settings, &self.root.join("donnees"), Binding::Offline, &network).await.unwrap()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Crée un vrai .torrent (contenu factice) rangé à un chemin plein d'espaces, comme un
    /// fichier déposé ou choisi dans l'explorateur.
    async fn make_torrent(sandbox: &Sandbox, engine: &Engine) -> PathBuf {
        let content = sandbox.root.join("contenu source").join("Film de test 2026.mkv");
        std::fs::create_dir_all(content.parent().unwrap()).unwrap();
        std::fs::write(&content, vec![7u8; 300_000]).unwrap();
        let (hash, created) = engine.share(&content, &ShareOptions::default()).await.unwrap();
        // on ne garde que le fichier .torrent : le torrent partagé est retiré du moteur
        engine.delete(&hash, false).await.unwrap();
        let target = sandbox.root.join("Mes torrents").join("Cocorico 2 TRUEFRENCH 4K (1).torrent");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(created, &target).unwrap();
        target
    }

    #[tokio::test]
    async fn local_torrent_with_spaces_is_added_then_deleted_with_its_files() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let torrent = make_torrent(&sandbox, &engine).await;
        let link = torrent.to_string_lossy().into_owned();

        let preview = engine.preview(&link).await.expect("aperçu d'un .torrent local");
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.total_bytes, 300_000);

        let base = sandbox.root.join("telechargements");
        let hash = engine.add(&link, &base, None, false).await.expect("ajout d'un .torrent local");
        assert_eq!(hash, preview.info_hash);
        assert!(engine.torrents().iter().any(|t| t.info_hash == hash));

        // le .torrent d'origine n'est plus relu après l'ajout : il peut disparaître
        std::fs::remove_file(&torrent).unwrap();
        assert!(engine.files(&hash).is_ok());

        engine.delete(&hash, true).await.unwrap();
        assert!(engine.torrents().iter().all(|t| t.info_hash != hash));
        assert!(!base.join("Film de test 2026.mkv").exists(), "fichier commencé effacé");
        engine.stop().await;
    }

    #[tokio::test]
    async fn missing_torrent_file_says_so() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let missing = sandbox.root.join("Téléchargements").join("Cocorico.torrent");
        let err = engine.preview(&missing.to_string_lossy()).await.err().expect("erreur attendue");
        assert!(format!("{err:#}").contains("Fichier .torrent introuvable"), "{err:#}");
        let err = engine.preview("pas un lien").await.err().expect("erreur attendue");
        assert!(format!("{err:#}").contains("Lien non reconnu"), "{err:#}");
        engine.stop().await;
    }

    #[tokio::test]
    async fn magnet_without_any_peer_source_gives_a_readable_error() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let magnet = "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&tr=udp://tracker.opentrackr.org:1337/announce";
        let err = engine.preview(magnet).await.err().expect("erreur attendue");
        let text = format!("{err:#}");
        assert!(!text.contains("no known way"), "message brut du moteur : {text}");
        assert!(text.contains("VPN coupé"), "{text}");
        engine.stop().await;
    }

    #[tokio::test]
    async fn magnet_search_counts_the_peers_it_tries() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        // un seul pair, injoignable (moteur hors ligne) : compté, puis plus aucune source
        let magnet = "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&x.pe=127.0.0.1:9";
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let err = engine.preview_counting(magnet, Some(seen.clone())).await.err().expect("erreur attendue");
        assert_eq!(seen.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(format!("{err:#}").contains("Aucun pair n'a transmis"), "{err:#}");
        engine.stop().await;
    }

    #[test]
    fn paths_are_told_apart_from_links() {
        assert!(looks_like_path(r"C:\Users\x\Downloads\Cocorico.torrent"));
        assert!(looks_like_path(r"\\nas\partage\a.torrent"));
        assert!(looks_like_path("/home/x/a"));
        assert!(!looks_like_path("magnet:?xt=urn:btih:abc"));
        assert!(!looks_like_path("n'importe quoi"));
    }
}

#[cfg(test)]
mod selection_tests {
    #[test]
    fn selection_mask_ignores_out_of_range_indices() {
        let mask = super::selection_mask(Some(&[0, 2, 99]), 3).unwrap();
        assert_eq!(mask, vec![true, false, true]);
        assert!(super::is_selected(None, 5), "sans choix : tout est choisi");
        assert!(!super::is_selected(Some(&mask), 7));
    }
}
