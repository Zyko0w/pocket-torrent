//! Vue d'ensemble envoyée à l'interface (liste des téléchargements).

use super::*;

impl Manager {
    pub fn overview(&self) -> Overview {
        let views = self.engine.torrents();
        let by_hash: HashMap<&str, &TorrentView> = views.iter().map(|v| (v.info_hash.as_str(), v)).collect();
        let settings = self.settings.read().clone();
        let data = self.data.lock();
        let runtimes = self.directs.lock();
        let mut jobs = Vec::new();

        for (hash, entry) in &data.torrents {
            jobs.push(torrent_job(hash, entry, by_hash.get(hash.as_str()).copied()));
        }
        let searches = self.searches.lock();
        for pending in &data.pending {
            jobs.push(pending_job(pending, searches.get(&pending.id)));
        }
        drop(searches);
        for entry in &data.directs {
            jobs.push(direct_job(entry, runtimes.get(&entry.id)));
        }

        let (_, queued) = {
            drop(runtimes);
            drop(data);
            self.slots(&views)
        };
        let positions: HashMap<String, usize> =
            queued.iter().enumerate().map(|(position, slot)| (slot.key.id(), position + 1)).collect();
        for job in &mut jobs {
            job.queue_position = positions.get(&job.id).copied();
        }
        jobs.sort_by_key(|job| std::cmp::Reverse(job.added_at));

        let game = self.game.lock().clone();
        Overview {
            down_speed: jobs.iter().map(|j| j.down_speed).sum(),
            up_speed: jobs.iter().map(|j| j.up_speed).sum(),
            jobs,
            game: GameView {
                active: game.active,
                process: game.process,
                mode: settings.game_mode.mode,
                action: settings.game_mode.action,
            },
            categories: settings
                .categories
                .iter()
                .map(|c| CategoryView { id: c.id.clone(), name: c.name.clone() })
                .collect(),
            r2_ready: settings.r2_ready() && self.r2_keys_set.load(Ordering::Relaxed),
            vpn: self.vpn_view(&settings),
            power: self.power_view(),
        }
    }
}

fn base_job(id: String, kind: &'static str, common: &Common) -> JobView {
    JobView {
        id,
        kind,
        name: None,
        link: None,
        status: "queued",
        hold: common.hold,
        error: common.error.clone(),
        warning: common.warning.clone(),
        progress_bytes: 0,
        wanted_bytes: 0,
        uploaded_bytes: 0,
        down_speed: 0,
        up_speed: 0,
        eta_secs: None,
        peers_live: 0,
        file_count: 0,
        selected_files: 0,
        category: common.category.clone(),
        priority: common.priority,
        dest: common.dest,
        queue_position: None,
        added_at: common.added_at,
        finished_at: common.finished_at,
        delivery: common.delivery.clone(),
        limit_kbps: 0,
        in_engine: false,
        can_share: false,
        source_file: None,
        search_secs: None,
        peers_found: None,
    }
}

fn delivery_status(common: &Common) -> Option<&'static str> {
    match common.delivery.as_ref()?.state {
        DeliveryState::Uploading => Some("uploading"),
        DeliveryState::Error => Some("error"),
        DeliveryState::Done => None,
    }
}

fn torrent_job(hash: &str, entry: &TorrentEntry, view: Option<&TorrentView>) -> JobView {
    let mut job = base_job(format!("t:{hash}"), "torrent", &entry.common);
    job.name = entry.name.clone();
    job.wanted_bytes = entry.wanted_bytes;
    job.file_count = entry.file_count;
    job.selected_files = entry.selected_files;
    job.source_file = entry.source_file.as_deref().filter(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned());

    let Some(view) = view else {
        job.progress_bytes = entry.wanted_bytes;
        job.status =
            delivery_status(&entry.common).unwrap_or(if entry.common.error.is_some() { "error" } else { "done" });
        return job;
    };
    job.in_engine = true;
    job.progress_bytes = view.progress_bytes;
    job.uploaded_bytes = view.uploaded_bytes;
    job.down_speed = view.down_speed;
    job.up_speed = view.up_speed;
    job.peers_live = view.peers_live;
    if view.error.is_some() {
        job.error = view.error.clone();
    }
    job.eta_secs = (view.down_speed > 0 && view.wanted_bytes > view.progress_bytes)
        .then(|| (view.wanted_bytes - view.progress_bytes) / view.down_speed);
    job.status = delivery_status(&entry.common).unwrap_or(match view.state {
        EngineState::Error => "error",
        EngineState::Initializing => "checking",
        EngineState::Paused if view.finished => "done",
        EngineState::Paused if entry.common.hold == Some(Hold::Queue) => "queued",
        EngineState::Paused => "paused",
        EngineState::Live if view.finished => "seeding",
        EngineState::Live => "downloading",
    });
    job
}

fn pending_job(pending: &PendingTorrent, search: Option<&Search>) -> JobView {
    let mut job = base_job(format!("p:{}", pending.id), "torrent", &pending.common);
    job.name = pending.name.clone();
    job.link = Some(pending.link.clone());
    job.source_file = Path::new(&pending.link).is_file().then(|| pending.link.clone());
    job.status = if pending.common.error.is_some() { "error" } else { "metadata" };
    if let Some(search) = search.filter(|_| pending.common.error.is_none()) {
        job.search_secs = Some(search.started.elapsed().as_secs());
        job.peers_found = Some(search.peers_seen.load(Ordering::Relaxed));
    }
    job
}

fn direct_job(entry: &DirectEntry, runtime: Option<&DirectRuntime>) -> JobView {
    let mut job = base_job(format!("d:{}", entry.id), "http", &entry.common);
    job.name = entry.name.clone();
    job.link = Some(entry.link.clone());
    job.limit_kbps = entry.limit_kbps;
    job.file_count = 1;
    job.selected_files = 1;
    job.wanted_bytes = entry.total_bytes;
    job.progress_bytes = entry.received_bytes;
    if let Some(runtime) = runtime {
        job.progress_bytes = runtime.progress.received.load(Ordering::Relaxed);
        job.wanted_bytes = runtime.progress.total.load(Ordering::Relaxed).max(entry.total_bytes);
        job.down_speed = runtime.speed;
        job.eta_secs = (runtime.speed > 0 && job.wanted_bytes > job.progress_bytes)
            .then(|| (job.wanted_bytes - job.progress_bytes) / runtime.speed);
        if let Some(name) = runtime.progress.file.lock().name.clone() {
            job.name = Some(name);
        }
    }
    let c = &entry.common;
    job.status = delivery_status(c).unwrap_or(if c.error.is_some() {
        "error"
    } else if c.finished_at.is_some() {
        "done"
    } else if let Some(runtime) = runtime {
        if runtime.progress.file.lock().started { "downloading" } else { "connecting" }
    } else if matches!(c.hold, Some(Hold::User | Hold::Game | Hold::Disk)) {
        "paused"
    } else {
        "queued"
    });
    // lecture du disque seulement quand la réponse peut servir (terminé, pas encore partagé)
    let file_on_disk =
        c.finished_at.is_some() && entry.shared_hash.is_none() && entry.final_path.as_ref().is_some_and(|p| p.exists());
    job.can_share = c.finished_at.is_some() && file_on_disk && entry.shared_hash.is_none();
    job
}

pub fn format_bytes(bytes: u64) -> String {
    let units = ["o", "Ko", "Mo", "Go", "To"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} o")
    } else {
        format!("{} {}", format!("{value:.1}").replace('.', ","), units[unit])
    }
}
