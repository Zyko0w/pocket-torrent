//! Boucle d'une seconde : synchronisation avec le moteur, fins, erreurs, disque.

use super::*;

impl Manager {
    pub(super) fn spawn_loop(self: &Arc<Self>) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let mut round: u64 = 0;
            loop {
                interval.tick().await;
                if manager.closing.load(Ordering::SeqCst) {
                    break;
                }
                round += 1;
                // arrêt automatique en tête : rien ne démarre tant que le VPN n'est pas vérifié
                if round % 2 == 1
                    && let Err(err) = manager.check_vpn().await
                {
                    tracing::error!("bascule VPN impossible : {err:#}");
                }
                manager.maintain_forwarding();
                let views = manager.engine.torrents();
                manager.sync_torrents(&views);
                manager.restore_file_priorities(&views);
                manager.sync_directs();
                manager.record_traffic();
                manager.record_speeds(&views);
                manager.check_shutdown_when_done();
                manager.update_sleep_prevention(&views);
                if round % 5 == 1 {
                    manager.check_game().await;
                }
                if round.is_multiple_of(10) {
                    manager.check_disk(&views).await;
                }
                manager.check_seeding_limits(&views).await;
                manager.schedule(&views).await;
                manager.balance_bandwidth(&views);
                // planificateur de limites (§5) : réévalué chaque tick pour suivre l'heure
                if manager.settings.read().schedule.enabled {
                    manager.apply_limits();
                }
                manager.save_if_due(false);
            }
        });
    }

    /// Réapplique l'ordre des fichiers enregistré aux torrents qui tournent (§4.1).
    fn restore_file_priorities(&self, views: &[TorrentView]) {
        let wanted: Vec<(String, Vec<usize>)> = {
            let data = self.data.lock();
            views
                .iter()
                .filter(|v| v.state == EngineState::Live && !v.finished)
                .filter_map(|v| Some((v.info_hash.clone(), data.torrents.get(&v.info_hash)?.file_priorities.clone()?)))
                .collect()
        };
        for (hash, order) in wanted {
            if let Err(err) = self.engine.restore_file_priorities(&hash, &order) {
                // ordre devenu invalide (autre nombre de fichiers) : oublié, retour à l'ordre du moteur
                tracing::warn!("ordre des fichiers de {hash} non réappliqué : {err:#}");
                if let Some(entry) = self.data.lock().torrents.get_mut(&hash) {
                    entry.file_priorities = None;
                }
                self.mark_dirty();
            }
        }
    }

    /// Relie les torrents du moteur à leurs informations, et détecte fins et erreurs.
    fn sync_torrents(self: &Arc<Self>, views: &[TorrentView]) {
        let mut finished = Vec::new();
        let mut errors = Vec::new();
        {
            let adding = self.adding.lock();
            let mut data = self.data.lock();
            let now = now_ms();
            for view in views {
                let entry = data.torrents.entry(view.info_hash.clone()).or_insert_with(|| {
                    self.dirty.store(true, Ordering::Relaxed);
                    // torrent inconnu (ajouté avant cette version) : on le reprend tel quel
                    TorrentEntry {
                        common: Common {
                            added_at: now,
                            finished_at: view.finished.then_some(now),
                            hold: (view.state == EngineState::Paused && !view.finished).then_some(Hold::User),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });
                if entry.name != view.name || entry.wanted_bytes != view.wanted_bytes {
                    self.dirty.store(true, Ordering::Relaxed);
                }
                entry.name = view.name.clone();
                entry.wanted_bytes = view.wanted_bytes;
                entry.file_count = view.file_count;
                entry.selected_files = view.selected_files;
                entry.folder = Some(view.output_folder.clone());
                // relancé par le moteur (redémarrage, bascule VPN) : plus de raison d'attente
                if view.state == EngineState::Live && entry.common.hold.is_some() && !view.finished {
                    entry.common.hold = None;
                    self.dirty.store(true, Ordering::Relaxed);
                }

                if view.state != EngineState::Initializing {
                    if view.finished && entry.common.finished_at.is_none() {
                        entry.common.finished_at = Some(now);
                        entry.common.hold = None;
                        entry.common.forced = false;
                        entry.common.warning = None;
                        self.dirty.store(true, Ordering::Relaxed);
                        finished.push(view.info_hash.clone());
                    } else if !view.finished && entry.common.finished_at.is_some() && entry.common.delivery.is_none() {
                        // nouveaux fichiers choisis après la fin
                        entry.common.finished_at = None;
                    }
                }
                if view.state == EngineState::Error {
                    if !entry.common.error_notified {
                        entry.common.error_notified = true;
                        errors.push((view.name.clone().unwrap_or_default(), view.error.clone().unwrap_or_default()));
                    }
                } else if view.state == EngineState::Live {
                    entry.common.error_notified = false;
                }
            }
            // torrents retirés du moteur autrement que par l'application
            let present: HashSet<&str> = views.iter().map(|v| v.info_hash.as_str()).collect();
            let before = data.torrents.len();
            data.torrents
                .retain(|hash, entry| entry.removed || present.contains(hash.as_str()) || adding.contains(hash));
            if data.torrents.len() != before {
                self.dirty.store(true, Ordering::Relaxed);
            }
        }

        for (name, error) in errors {
            self.notify(Notify::Error, "Problème de téléchargement", &format!("{name} : {error}"));
        }
        for hash in finished {
            let manager = self.clone();
            tauri::async_runtime::spawn(async move { manager.on_torrent_finished(&hash).await });
        }
    }

    async fn on_torrent_finished(self: &Arc<Self>, hash: &str) {
        let (name, dest) = {
            let data = self.data.lock();
            let Some(entry) = data.torrents.get(hash) else { return };
            (entry.name.clone().unwrap_or_else(|| "Téléchargement".into()), entry.common.dest)
        };
        tracing::info!("torrent terminé : {name}");
        self.notify(Notify::Done, "Téléchargement terminé", &name);
        if let Ok(path) = self.engine.content_path(hash) {
            self.maybe_scan(name.clone(), path);
        }
        let seed = self.settings.read().seed_when_done;
        if dest == Dest::StorageTo || !seed {
            let _ = self.engine.pause(hash).await;
        }
        if dest == Dest::StorageTo {
            self.start_delivery(JobKey::Torrent(hash.to_owned()));
        }
        self.save_now();
    }

    /// Analyse un téléchargement terminé avec Microsoft Defender si activé (§10.4). Ne
    /// retarde jamais le flux normal : lancée en tâche de fond, ne fait que notifier en cas
    /// de menace (rapport seulement, rien n'est supprimé automatiquement).
    pub(super) fn maybe_scan(self: &Arc<Self>, name: String, path: PathBuf) {
        if !self.settings.read().scan_downloads {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(result) = tauri::async_runtime::spawn_blocking(move || crate::antivirus::scan(&path)).await else {
                return;
            };
            if let crate::antivirus::ScanResult::ThreatFound(threats) = result {
                manager.notify(Notify::Error, "Menace détectée", &format!("{name} : {}", threats.join(", ")));
            }
        });
    }

    /// Met à jour vitesse et progression des téléchargements directs.
    fn sync_directs(&self) {
        // toujours `data` puis `directs`, comme partout ailleurs (liste, file d'attente) : dans
        // l'autre ordre, la boucle et l'interface pourraient s'attendre l'une l'autre pour toujours
        let mut data = self.data.lock();
        let mut runtimes = self.directs.lock();
        if runtimes.is_empty() {
            return;
        }
        let mut changed = false;
        for (id, runtime) in runtimes.iter_mut() {
            let received = runtime.progress.received.load(Ordering::Relaxed);
            let elapsed = runtime.sample_at.elapsed().as_secs_f64();
            if elapsed >= 1.0 {
                let instant = received.saturating_sub(runtime.sample_bytes) as f64 / elapsed;
                // moyenne glissante pour une vitesse lisible
                runtime.speed = ((runtime.speed as f64 * 0.6) + instant * 0.4) as u64;
                runtime.sample_at = Instant::now();
                runtime.sample_bytes = received;
            }
            if let Some(entry) = data.direct_mut(id) {
                let before = entry.received_bytes;
                snapshot_progress(entry, &runtime.progress);
                changed |= entry.received_bytes != before;
            }
        }
        if changed {
            self.mark_dirty();
        }
    }
}

pub(super) fn snapshot_progress(entry: &mut DirectEntry, progress: &Progress) {
    entry.received_bytes = progress.received.load(Ordering::Relaxed);
    let total = progress.total.load(Ordering::Relaxed);
    if total > 0 {
        entry.total_bytes = total;
    }
    let file = progress.file.lock();
    if file.name.is_some() {
        entry.name = file.name.clone();
    }
    entry.part_path = file.part_path.clone();
}
