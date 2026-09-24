//! File d'attente : qui tourne, qui attend, selon la priorité et les limites.

use super::*;

impl Manager {
    pub(super) fn slots(&self, views: &[TorrentView]) -> (Vec<Slot>, Vec<Slot>) {
        let data = self.data.lock();
        let runtimes = self.directs.lock();
        let threshold_bps = u64::from(self.settings.read().queue.slow_threshold_kbps) * 1024;
        let mut running = Vec::new();
        let mut queued = Vec::new();

        for view in views {
            let Some(entry) = data.torrents.get(&view.info_hash) else { continue };
            let c = &entry.common;
            if view.finished || c.delivery.is_some() {
                continue;
            }
            let slot = Slot {
                key: JobKey::Torrent(view.info_hash.clone()),
                priority: c.priority,
                added_at: c.added_at,
                forced: c.forced,
                folder: view.output_folder.clone(),
                needed: view.wanted_bytes.saturating_sub(view.progress_bytes),
                // en cours de vérification (pas encore de pairs) : jamais « lent », c'est une autre phase
                slow: view.state == EngineState::Live && view.down_speed + view.up_speed < threshold_bps,
            };
            match view.state {
                EngineState::Live | EngineState::Initializing => running.push(slot),
                EngineState::Paused if c.hold == Some(Hold::Queue) => queued.push(slot),
                _ => {}
            }
        }
        for entry in &data.directs {
            let c = &entry.common;
            if c.finished_at.is_some() || c.error.is_some() || c.delivery.is_some() {
                continue;
            }
            let runtime = runtimes.get(&entry.id);
            let slot = Slot {
                key: JobKey::Direct(entry.id.clone()),
                priority: c.priority,
                added_at: c.added_at,
                forced: c.forced,
                folder: entry.folder.clone(),
                needed: entry.total_bytes.saturating_sub(entry.received_bytes),
                slow: runtime.is_some_and(|r| r.speed < threshold_bps),
            };
            if runtime.is_some() {
                running.push(slot);
            } else if matches!(c.hold, None | Some(Hold::Queue)) {
                queued.push(slot);
            }
        }
        queued.sort_by(|a, b| a.priority.rank().cmp(&b.priority.rank()).then(a.added_at.cmp(&b.added_at)));
        (running, queued)
    }

    fn game_pauses(&self) -> bool {
        self.game.lock().active && self.settings.read().game_mode.action == GameAction::Pause
    }

    /// Faut-il mettre un nouveau téléchargement en attente ?
    pub(super) fn queue_full(&self) -> bool {
        if self.game_pauses() || self.vpn_offline() {
            return true;
        }
        let queue = self.settings.read().queue.clone();
        let max = queue.max_active as usize;
        max > 0
            && self
                .slots(&self.engine.torrents())
                .0
                .iter()
                .filter(|s| !s.forced && !(queue.ignore_slow && s.slow))
                .count()
                >= max
    }

    pub(super) async fn schedule(self: &Arc<Self>, views: &[TorrentView]) {
        if self.game_pauses() || self.vpn_offline() {
            return;
        }
        let queue = self.settings.read().queue.clone();
        let max = queue.max_active as usize;
        let (running, queued) = self.slots(views);
        // « Démarrer maintenant » passe devant la file sans prendre la place des autres ;
        // un téléchargement lent (§ ignore_slow) non plus, pour ne pas bloquer les autres
        let mut candidates: Vec<&Slot> =
            running.iter().filter(|s| !s.forced && !(queue.ignore_slow && s.slow)).collect();

        if max > 0 && candidates.len() > max {
            // trop de téléchargements actifs (limite baissée, redémarrage…) : les moins prioritaires attendent
            let excess = candidates.len() - max;
            candidates.sort_by(|a, b| b.priority.rank().cmp(&a.priority.rank()).then(b.added_at.cmp(&a.added_at)));
            for slot in candidates.into_iter().take(excess) {
                self.hold(&slot.key, Hold::Queue).await;
            }
            return;
        }

        let mut count = candidates.len();
        for slot in queued {
            if !slot.forced && max > 0 && count >= max {
                continue;
            }
            if let Some(problem) = self.space_problem(&slot.folder, slot.needed) {
                self.hold_for_space(&slot.key, problem).await;
                continue;
            }
            self.release(&slot.key).await;
            count += 1;
        }
    }

    /// Remet dans la file ce qui était arrêté pour cette raison (VPN revenu, fin du jeu…).
    /// Renvoie le nombre de téléchargements concernés.
    pub(super) fn release_held(&self, reason: Hold) -> usize {
        let mut data = self.data.lock();
        let StoreData { torrents, directs, .. } = &mut *data;
        let held = torrents
            .values_mut()
            .map(|t| &mut t.common)
            .chain(directs.iter_mut().map(|d| &mut d.common))
            .filter(|c| c.hold == Some(reason));
        let mut count = 0;
        for common in held {
            common.hold = Some(Hold::Queue);
            count += 1;
        }
        self.mark_dirty();
        count
    }

    /// Arrête un téléchargement pour la raison donnée.
    pub(super) async fn hold(self: &Arc<Self>, key: &JobKey, reason: Hold) {
        match key {
            JobKey::Torrent(hash) => {
                if let Err(err) = self.engine.pause(hash).await {
                    tracing::warn!("pause impossible : {err:#}");
                    return;
                }
                if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                    entry.common.hold = Some(reason);
                }
            }
            JobKey::Direct(id) => {
                self.stop_direct(id);
                if let Some(entry) = self.data.lock().direct_mut(id) {
                    entry.common.hold = Some(reason);
                }
            }
            JobKey::Pending(_) => {}
        }
        self.mark_dirty();
    }

    async fn hold_for_space(self: &Arc<Self>, key: &JobKey, problem: String) {
        self.hold(key, Hold::Disk).await;
        let name = {
            let mut data = self.data.lock();
            match key {
                JobKey::Torrent(hash) => data.torrents.get_mut(hash).map(|e| {
                    e.common.warning = Some(problem.clone());
                    e.name.clone()
                }),
                JobKey::Direct(id) => data.direct_mut(id).map(|e| {
                    e.common.warning = Some(problem.clone());
                    e.name.clone().or_else(|| Some(e.link.clone()))
                }),
                JobKey::Pending(_) => None,
            }
        };
        let name = name.flatten().unwrap_or_else(|| "Un téléchargement".into());
        self.notify(Notify::Error, "Espace disque insuffisant", &format!("{name} a été mis en pause."));
        self.save_now();
    }

    /// Démarre un téléchargement en attente.
    pub(super) async fn release(self: &Arc<Self>, key: &JobKey) {
        match key {
            JobKey::Torrent(hash) => {
                if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                    entry.common.hold = None;
                    entry.common.warning = None;
                }
                if let Err(err) = self.engine.resume(hash).await {
                    tracing::warn!("reprise impossible : {err:#}");
                }
            }
            JobKey::Direct(id) => {
                if let Some(entry) = self.data.lock().direct_mut(id) {
                    entry.common.hold = None;
                    entry.common.warning = None;
                }
                self.start_direct(id);
            }
            JobKey::Pending(_) => {}
        }
        self.mark_dirty();
    }

    pub(super) fn space_problem(&self, folder: &Path, needed: u64) -> Option<String> {
        let free = system::free_space(folder)?;
        let margin_mb = self.settings.read().disk_margin_mb;
        if free >= needed.saturating_add(margin_mb * MB) {
            return None;
        }
        if needed == 0 {
            return Some(format!(
                "Disque presque plein : {} libres (marge de {margin_mb} Mo). Libère de la place puis reprends.",
                format_bytes(free)
            ));
        }
        Some(format!(
            "Espace disque insuffisant : {} nécessaires, {} libres (marge de {margin_mb} Mo). Libère de la place puis reprends.",
            format_bytes(needed),
            format_bytes(free)
        ))
    }

    /// Pendant un téléchargement : met en pause si le disque est presque plein.
    pub(super) async fn check_disk(self: &Arc<Self>, views: &[TorrentView]) {
        let (running, _) = self.slots(views);
        for slot in running {
            if let Some(problem) = self.space_problem(&slot.folder, 0) {
                self.hold_for_space(&slot.key, problem).await;
            }
        }
    }

    /// Arrête le partage d'un torrent terminé qui dépasse le ratio ou la durée fixés dans les
    /// réglages (0 = illimité) : utile sur les trackers privés, pour respecter un ratio minimum
    /// sans pour autant partager indéfiniment.
    pub(super) async fn check_seeding_limits(self: &Arc<Self>, views: &[TorrentView]) {
        let seeding = self.settings.read().seeding;
        if seeding.ratio_limit <= 0.0 && seeding.time_limit_minutes == 0 {
            return;
        }
        let now = now_ms();
        let time_limit_ms = u64::from(seeding.time_limit_minutes) * 60_000;
        let hits: Vec<(String, String)> = {
            let data = self.data.lock();
            views
                .iter()
                .filter(|view| view.finished)
                .filter_map(|view| {
                    let entry = data.torrents.get(&view.info_hash)?;
                    if entry.common.hold.is_some() || entry.common.delivery.is_some() {
                        return None;
                    }
                    let ratio_hit = seeding.ratio_limit > 0.0
                        && view.wanted_bytes > 0
                        && view.uploaded_bytes as f64 / view.wanted_bytes as f64 >= seeding.ratio_limit;
                    let time_hit = seeding.time_limit_minutes > 0
                        && entry.common.finished_at.is_some_and(|at| now.saturating_sub(at) >= time_limit_ms);
                    (ratio_hit || time_hit).then(|| (view.info_hash.clone(), entry.name.clone().unwrap_or_default()))
                })
                .collect()
        };
        for (hash, name) in hits {
            self.hold(&JobKey::Torrent(hash), Hold::Seeded).await;
            self.notify(Notify::Done, "Partage arrêté", &format!("{name} a atteint sa limite de partage."));
        }
    }
}
