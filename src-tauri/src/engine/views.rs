//! Vues du moteur (torrents, fichiers, pairs, trackers, carte des pièces) et lecture de son état.

use super::*;

pub(super) struct PeerSample {
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

/// Un tracker d'un torrent et l'état de ses annonces.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerView {
    pub url: String,
    /// idle (torrent arrêté : pas d'annonce) | connecting | working | failing | skipped
    pub state: &'static str,
    /// Pairs renvoyés par la dernière annonce réussie.
    pub peers: usize,
    /// Sources complètes et clients de l'essaim selon le tracker (s'il les donne).
    pub seeders: Option<u64>,
    pub leechers: Option<u64>,
    pub error: Option<String>,
    /// Dernière annonce réussie et prochaine annonce prévue, en ms depuis 1970.
    pub last_success_at: Option<u64>,
    pub next_announce_at: Option<u64>,
}

pub(super) fn epoch_ms(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PieceMap {
    pub total_pieces: u32,
    /// Empaqueté 8 pièces par octet, poids fort en premier ; 1 = pièce présente.
    pub bits: Vec<u8>,
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
                // jamais un fichier hors du dossier du torrent (l'envoi en ligne lirait n'importe quoi)
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

    /// Trackers du torrent (les siens et ceux ajoutés par l'appli) avec l'état de leurs annonces,
    /// triés. Un torrent arrêté n'annonce rien : ses trackers sont « idle ».
    pub fn trackers(&self, hash: &str) -> anyhow::Result<Vec<TrackerView>> {
        use librqbit_tracker_comms::TrackerState;
        let torrent = self.handle(hash)?;
        let mut views: Vec<TrackerView> = librqbit_tracker_comms::tracker_statuses(&torrent.info_hash())
            .into_iter()
            .map(|status| TrackerView {
                url: status.url,
                state: match status.state {
                    TrackerState::Connecting => "connecting",
                    TrackerState::Working => "working",
                    TrackerState::Failing => "failing",
                    TrackerState::Skipped => "skipped",
                },
                peers: status.last_peers,
                seeders: status.swarm.map(|s| s.seeders),
                leechers: status.swarm.map(|s| s.leechers),
                error: status.error,
                last_success_at: status.last_success.and_then(epoch_ms),
                next_announce_at: status.next_announce.and_then(epoch_ms),
            })
            .collect();
        let extra = if torrent.with_metadata(|m| m.info.info().private).unwrap_or(false) {
            Vec::new()
        } else {
            EXTRA_TRACKERS.iter().map(|t| (*t).to_owned()).collect()
        };
        // même plafond que les annonces : un .torrent piégé peut en lister des milliers
        let listed = torrent.shared().trackers.iter().map(|t| t.to_string()).chain(extra);
        for url in listed.take(librqbit_tracker_comms::MAX_TRACKERS_PER_TORRENT) {
            if !views.iter().any(|v| v.url == url) {
                views.push(TrackerView {
                    url,
                    state: "idle",
                    peers: 0,
                    seeders: None,
                    leechers: None,
                    error: None,
                    last_success_at: None,
                    next_announce_at: None,
                });
            }
        }
        views.sort_by(|a, b| a.url.cmp(&b.url));
        Ok(views)
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
}

pub(super) fn view_of(torrent: &Arc<ManagedTorrent>) -> TorrentView {
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
