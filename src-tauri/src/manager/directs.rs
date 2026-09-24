//! Téléchargements directs (http/https) : démarrage, fin, erreurs.

use super::*;

impl Manager {
    pub(super) fn start_direct(self: &Arc<Self>, id: &str) {
        if self.directs.lock().contains_key(id) || self.vpn_offline() {
            return;
        }
        let Some(entry) = self.data.lock().directs.iter().find(|d| d.id == id).cloned() else { return };

        let progress = Arc::new(Progress::default());
        let already = entry.part_path.as_ref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        progress.received.store(already, Ordering::Relaxed);
        progress.total.store(entry.total_bytes, Ordering::Relaxed);
        progress.limit_kbps.store(entry.limit_kbps, Ordering::Relaxed);
        {
            let mut file = progress.file.lock();
            file.name = entry.name.clone();
            file.part_path = entry.part_path.clone();
        }
        self.directs.lock().insert(
            id.to_owned(),
            DirectRuntime {
                progress: progress.clone(),
                task: None,
                sample_at: Instant::now(),
                sample_bytes: already,
                speed: 0,
            },
        );

        let weak = Arc::downgrade(self);
        let check_space: direct::SpaceCheck =
            Arc::new(move |dir: &Path, needed: u64| match weak.upgrade().and_then(|m| m.space_problem(dir, needed)) {
                Some(problem) => Err(problem),
                None => Ok(()),
            });
        let download = Download {
            url: entry.link.clone(),
            dir: entry.folder.clone(),
            progress,
            session: self.engine.session(),
            check_space,
        };
        let manager = self.clone();
        let job_id = id.to_owned();
        let task = tauri::async_runtime::spawn(async move {
            let http = manager.http();
            let result = direct::run(&http, download).await;
            manager.on_direct_done(&job_id, result).await;
        });
        if let Some(runtime) = self.directs.lock().get_mut(id) {
            runtime.task = Some(task);
        }
    }

    /// Arrête la tâche d'un téléchargement direct en gardant sa progression.
    pub(super) fn stop_direct(&self, id: &str) {
        let Some(runtime) = self.directs.lock().remove(id) else { return };
        if let Some(task) = runtime.task {
            task.abort();
        }
        if let Some(entry) = self.data.lock().direct_mut(id) {
            snapshot_progress(entry, &runtime.progress);
        }
        self.mark_dirty();
    }

    fn fail_direct(&self, id: &str, message: String) {
        let name = {
            let mut data = self.data.lock();
            data.direct_mut(id).map(|entry| {
                entry.common.error = Some(message.clone());
                entry.name.clone().unwrap_or_else(|| entry.link.clone())
            })
        };
        if let Some(name) = name {
            tracing::error!("{name} : {message}");
            self.notify(Notify::Error, "Problème de téléchargement", &format!("{name} : {message}"));
        }
        self.save_now();
    }

    async fn on_direct_done(self: &Arc<Self>, id: &str, result: Result<PathBuf, DownloadError>) {
        let Some(runtime) = self.directs.lock().remove(id) else { return };
        if let Some(entry) = self.data.lock().direct_mut(id) {
            snapshot_progress(entry, &runtime.progress);
        }
        match result {
            Ok(path) => {
                let (name, dest) = {
                    let mut data = self.data.lock();
                    let Some(entry) = data.direct_mut(id) else { return };
                    entry.final_path = Some(path.clone());
                    entry.part_path = None;
                    entry.received_bytes = entry.total_bytes.max(entry.received_bytes);
                    entry.common.finished_at = Some(now_ms());
                    entry.common.hold = None;
                    entry.common.forced = false;
                    entry.common.warning = None;
                    (entry.name.clone().unwrap_or_else(|| entry.link.clone()), entry.common.dest)
                };
                tracing::info!("téléchargement direct terminé : {name}");
                self.notify(Notify::Done, "Téléchargement terminé", &name);
                self.maybe_scan(name.clone(), path.clone());
                if dest == Dest::R2 {
                    self.start_delivery(JobKey::Direct(id.to_owned()));
                }
                self.save_now();
            }
            Err(DownloadError::NoSpace(problem)) => {
                if let Some(entry) = self.data.lock().direct_mut(id) {
                    entry.common.hold = Some(Hold::Disk);
                    entry.common.warning = Some(problem.clone());
                }
                self.notify(Notify::Error, "Espace disque insuffisant", "Un téléchargement a été mis en pause.");
                self.save_now();
            }
            Err(DownloadError::Failed(err)) => self.fail_direct(id, format!("{err:#}")),
        }
    }
}
