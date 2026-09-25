//! Actions de l'utilisateur : pause, reprise, suppression, fichiers, partage…

use super::*;

impl Manager {
    pub async fn pause(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        match JobKey::parse(id)? {
            JobKey::Torrent(hash) => {
                let finished = self.data.lock().torrents.get(&hash).is_some_and(|e| e.common.finished_at.is_some());
                self.engine.pause(&hash).await?;
                if let Some(entry) = self.data.lock().torrents.get_mut(&hash) {
                    // un torrent terminé qui partage redevient simplement « terminé »
                    entry.common.hold = (!finished).then_some(Hold::User);
                    entry.common.forced = false;
                }
            }
            JobKey::Direct(id) => {
                self.stop_direct(&id);
                let mut data = self.data.lock();
                let entry = data.direct_mut(&id).context("Téléchargement introuvable.")?;
                if entry.common.finished_at.is_some() || entry.common.error.is_some() {
                    bail!("Ce téléchargement ne peut pas être mis en pause.");
                }
                entry.common.hold = Some(Hold::User);
                entry.common.forced = false;
            }
            JobKey::Pending(_) => bail!("La recherche des infos du torrent ne peut pas être mise en pause."),
        }
        self.save_now();
        Ok(())
    }

    pub async fn resume(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.resume_inner(id, false).await
    }

    /// Démarre tout de suite, même si la file d'attente est pleine.
    pub async fn start_now(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.resume_inner(id, true).await
    }

    async fn resume_inner(self: &Arc<Self>, id: &str, force: bool) -> anyhow::Result<()> {
        if self.vpn_offline() {
            bail!("VPN coupé : reconnecte-le, les téléchargements reprendront tout seuls.");
        }
        let key = JobKey::parse(id)?;
        let failed_delivery = self
            .with_common(&key, |c| c.delivery.as_ref().is_some_and(|d| d.state == DeliveryState::Error))
            .unwrap_or(false);
        if failed_delivery {
            self.start_delivery(key);
            return Ok(());
        }

        match &key {
            JobKey::Torrent(hash) => {
                let view = self
                    .engine
                    .torrents()
                    .into_iter()
                    .find(|v| &v.info_hash == hash)
                    .context("Torrent introuvable.")?;
                if view.finished || view.state == EngineState::Error {
                    // partager un torrent terminé, ou relancer un torrent en erreur
                    self.engine.resume(hash).await?;
                    if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                        entry.common.hold = None;
                    }
                } else {
                    if let Some(problem) =
                        self.space_problem(&view.output_folder, view.wanted_bytes.saturating_sub(view.progress_bytes))
                    {
                        bail!(problem);
                    }
                    if force {
                        if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                            entry.common.forced = true;
                        }
                        self.release(&key).await;
                    } else if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                        entry.common.hold = Some(Hold::Queue);
                        entry.common.warning = None;
                    }
                }
            }
            JobKey::Direct(id) => {
                {
                    let mut data = self.data.lock();
                    let entry = data.direct_mut(id).context("Téléchargement introuvable.")?;
                    if entry.common.finished_at.is_some() {
                        bail!("Ce téléchargement est déjà terminé.");
                    }
                    entry.common.error = None;
                    entry.common.error_notified = false;
                    entry.common.warning = None;
                    entry.common.forced = force;
                    entry.common.hold = Some(Hold::Queue);
                }
                if force {
                    self.release(&key).await;
                }
            }
            JobKey::Pending(id) => {
                let retry = {
                    let mut data = self.data.lock();
                    let pending = data.pending_mut(id).context("Téléchargement introuvable.")?;
                    pending.common.error.take().is_some()
                };
                if retry && !self.tasks.lock().contains_key(&key.id()) {
                    self.spawn_pending(id.clone());
                }
            }
        }
        self.save_now();
        Ok(())
    }

    pub async fn pause_all(self: &Arc<Self>) -> usize {
        let ids: Vec<String> = self
            .overview()
            .jobs
            .into_iter()
            .filter(|j| matches!(j.status, "checking" | "connecting" | "queued" | "downloading"))
            .filter(|j| !j.id.starts_with("p:"))
            .map(|j| j.id)
            .collect();
        let mut count = 0;
        for id in ids {
            if self.pause(&id).await.is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn resume_all(self: &Arc<Self>) -> usize {
        let ids: Vec<String> =
            self.overview().jobs.into_iter().filter(|j| j.status == "paused").map(|j| j.id).collect();
        let mut count = 0;
        for id in ids {
            if self.resume(&id).await.is_ok() {
                count += 1;
            }
        }
        count
    }

    pub fn set_priority(&self, id: &str, priority: Priority) -> anyhow::Result<()> {
        self.with_common(&JobKey::parse(id)?, |c| c.priority = priority).context("Téléchargement introuvable.")?;
        self.save_now();
        Ok(())
    }

    pub fn set_limit(&self, id: &str, limit_kbps: u32) -> anyhow::Result<()> {
        if limit_kbps > 4_000_000 {
            bail!("Limite invalide (Ko/s, 0 = illimitée).");
        }
        let id = match JobKey::parse(id)? {
            JobKey::Direct(id) => id,
            JobKey::Torrent(hash) => {
                self.data.lock().torrents.get_mut(&hash).context("Téléchargement introuvable.")?.limit_kbps =
                    limit_kbps;
                // appliquée tout de suite ; la répartition par priorité en tient compte ensuite
                let cap = (limit_kbps > 0).then(|| u64::from(limit_kbps) * 1024);
                self.engine.set_torrent_download_limit(&hash, cap);
                self.save_now();
                return Ok(());
            }
            JobKey::Pending(_) => bail!("La limite s'applique une fois le torrent démarré."),
        };
        self.data.lock().direct_mut(&id).context("Téléchargement introuvable.")?.limit_kbps = limit_kbps;
        if let Some(runtime) = self.directs.lock().get(&id) {
            runtime.progress.limit_kbps.store(limit_kbps, Ordering::Relaxed);
        }
        self.save_now();
        Ok(())
    }

    /// Retire un téléchargement (et l'arrête). `delete_files` : fichiers téléchargés ;
    /// `delete_online` : copies sur storage.to ; `delete_source` : fichier .torrent local d'origine.
    pub async fn delete(
        self: &Arc<Self>,
        id: &str,
        delete_files: bool,
        delete_online: bool,
        delete_source: bool,
    ) -> anyhow::Result<()> {
        let key = JobKey::parse(id)?;
        let source = match &key {
            JobKey::Torrent(hash) => self.data.lock().torrents.get(hash).and_then(|e| e.source_file.clone()),
            JobKey::Pending(id) => {
                self.data.lock().pending.iter().find(|p| &p.id == id).map(|p| PathBuf::from(&p.link))
            }
            JobKey::Direct(_) => None,
        };
        if let Some(task) = self.tasks.lock().remove(&format!("upload:{}", key.id())) {
            task.abort();
        }
        if delete_online {
            let links =
                self.with_common(&key, |c| c.delivery.as_ref().map(|d| d.links.clone())).flatten().unwrap_or_default();
            if !links.is_empty() {
                let storage = StorageTo::from_settings(&self.settings().storage_to)?;
                let http = self.http();
                // une collection emporte ses fichiers : inutile de les supprimer un par un
                let collection = links.iter().any(|l| l.kind == "collection");
                for link in links.iter().filter(|l| !collection || l.kind == "collection") {
                    if link.owner_token.is_empty() {
                        continue;
                    }
                    storage
                        .delete(&http, &link.kind, &link.key, &link.owner_token)
                        .await
                        .with_context(|| format!("suppression de « {} » sur storage.to impossible", link.name))?;
                }
            }
        }

        match &key {
            JobKey::Torrent(hash) => {
                let removed = self.data.lock().torrents.get(hash).is_some_and(|e| e.removed);
                if !removed {
                    self.engine.delete(hash, delete_files).await?;
                }
                self.data.lock().torrents.remove(hash);
            }
            JobKey::Pending(id) => {
                if let Some(task) = self.tasks.lock().remove(&key.id()) {
                    task.abort();
                }
                self.searches.lock().remove(id);
                self.data.lock().pending.retain(|p| &p.id != id);
            }
            JobKey::Direct(id) => {
                self.stop_direct(id);
                let entry = self.data.lock().directs.iter().find(|d| &d.id == id).cloned();
                if let (true, Some(entry)) = (delete_files, entry) {
                    if entry.common.dest == Dest::StorageTo {
                        self.safe_remove(&entry.folder);
                    }
                    for path in [entry.part_path, entry.final_path].into_iter().flatten() {
                        self.safe_remove(&path);
                    }
                }
                self.data.lock().directs.retain(|d| &d.id != id);
            }
        }
        self.save_now();
        if delete_source && let Some(source) = source {
            remove_torrent_file(&source)?;
        }
        Ok(())
    }

    /// Retire de la liste les téléchargements terminés (sans toucher aux fichiers).
    pub async fn clear_finished(self: &Arc<Self>) -> anyhow::Result<usize> {
        let done: Vec<String> = self.overview().jobs.into_iter().filter(|j| j.status == "done").map(|j| j.id).collect();
        let mut count = 0;
        for id in done {
            self.delete(&id, false, false, false).await?;
            count += 1;
        }
        Ok(count)
    }

    fn torrent_hash(id: &str) -> anyhow::Result<String> {
        match JobKey::parse(id)? {
            JobKey::Torrent(hash) => Ok(hash),
            _ => bail!("Disponible seulement pour les torrents."),
        }
    }

    pub fn files(&self, id: &str) -> anyhow::Result<Vec<FileView>> {
        let mut files = self.engine.files(&Self::torrent_hash(id)?)?;
        self.flag_danger(&mut files);
        Ok(files)
    }

    pub async fn set_files(&self, id: &str, files: Vec<usize>) -> anyhow::Result<()> {
        self.engine.set_files(&Self::torrent_hash(id)?, files).await
    }

    pub async fn move_file_priority(&self, id: &str, index: usize, up: bool) -> anyhow::Result<()> {
        let hash = Self::torrent_hash(id)?;
        let order = self.engine.move_file_priority(&hash, index, up).await?;
        if let Some(entry) = self.data.lock().torrents.get_mut(&hash) {
            entry.file_priorities = Some(order);
        }
        self.save_now();
        Ok(())
    }

    pub async fn recheck(&self, id: &str) -> anyhow::Result<()> {
        let hash = Self::torrent_hash(id)?;
        // le torrent quitte le moteur un instant : sans cette marque, la boucle le croirait
        // retiré et oublierait sa catégorie, sa priorité et sa destination
        self.adding.lock().insert(hash.clone());
        let result = self.engine.recheck(&hash).await;
        self.adding.lock().remove(&hash);
        result
    }

    pub fn peers(&self, id: &str) -> anyhow::Result<Vec<PeerView>> {
        self.engine.peers(&Self::torrent_hash(id)?)
    }

    pub fn trackers(&self, id: &str) -> anyhow::Result<Vec<crate::engine::TrackerView>> {
        self.engine.trackers(&Self::torrent_hash(id)?)
    }

    pub fn piece_map(&self, id: &str) -> anyhow::Result<crate::engine::PieceMap> {
        self.engine.piece_map(&Self::torrent_hash(id)?)
    }

    /// Chemin sur le disque à servir pour le streaming (§7.6) : erreur si le torrent est
    /// inconnu ou le fichier introuvable, sans exiger qu'il soit déjà entièrement téléchargé.
    pub fn stream_path(&self, id: &str, file_index: usize) -> anyhow::Result<PathBuf> {
        self.engine.file_path(&Self::torrent_hash(id)?, file_index)
    }

    pub fn open_stream_in_player(&self, id: &str, file_index: usize, url: &str) -> anyhow::Result<()> {
        let path = self.stream_path(id, file_index)?;
        let name = path.file_name().map_or_else(|| "flux".to_owned(), |n| n.to_string_lossy().into_owned());
        crate::stream::open_in_player(&self.data_dir, &name, url)
    }

    pub async fn performance_snapshot(&self) -> crate::perf::Snapshot {
        crate::perf::snapshot(&self.engine, &self.settings().download_dir).await
    }

    pub fn log_tail(&self, max_lines: usize) -> anyhow::Result<String> {
        crate::logging::tail(&self.data_dir, max_lines)
    }

    pub fn export_log(&self, dest: &Path) -> anyhow::Result<()> {
        crate::logging::export(&self.data_dir, dest)
    }

    pub fn open_log_folder(&self) -> anyhow::Result<()> {
        let folder = crate::logging::folder(&self.data_dir);
        std::fs::create_dir_all(&folder)?;
        system::reveal(&folder)
    }

    pub fn magnet(&self, id: &str) -> anyhow::Result<String> {
        let hash = Self::torrent_hash(id)?;
        self.engine.magnet(&hash).or_else(|_| Ok(format!("magnet:?xt=urn:btih:{hash}")))
    }

    pub fn open_folder(&self, id: &str) -> anyhow::Result<()> {
        let path = match JobKey::parse(id)? {
            JobKey::Torrent(hash) => self
                .engine
                .content_path(&hash)
                .ok()
                .or_else(|| self.data.lock().torrents.get(&hash).and_then(|e| e.folder.clone())),
            JobKey::Direct(id) => {
                let data = self.data.lock();
                data.directs
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.final_path.clone().filter(|p| p.exists()).unwrap_or_else(|| d.folder.clone()))
            }
            JobKey::Pending(_) => None,
        };
        let path = path.filter(|p| p.exists()).context("Dossier introuvable sur le disque.")?;
        system::reveal(&path)
    }
}

/// Efface un fichier .torrent d'origine : jamais autre chose qu'un fichier « .torrent », pour
/// qu'un lien mal interprété ne puisse pas faire effacer un autre fichier.
pub(super) fn remove_torrent_file(path: &Path) -> anyhow::Result<()> {
    let is_torrent = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"));
    // « rapport.pdf:x.torrent » désigne un flux caché d'un autre fichier, pas un .torrent
    let stream = path.file_name().is_some_and(|n| n.to_string_lossy().contains(':'));
    if !is_torrent || stream || !path.is_file() {
        return Ok(());
    }
    std::fs::remove_file(path).with_context(|| format!("fichier .torrent non supprimé : {}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_torrent_files_are_removed_as_sources() {
        let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let torrent = root.join("Mon fichier (1).TORRENT");
        let other = root.join("document.txt");
        std::fs::write(&torrent, b"d4:infode").unwrap();
        std::fs::write(&other, b"garder").unwrap();

        super::remove_torrent_file(&other).unwrap();
        assert!(other.exists(), "un fichier qui n'est pas un .torrent n'est jamais effacé");
        let stream = root.join("document.txt:cache.torrent");
        if std::fs::write(&stream, b"flux").is_ok() {
            super::remove_torrent_file(&stream).unwrap();
            assert!(std::fs::read(&stream).is_ok(), "flux NTFS caché jamais effacé");
        }
        super::remove_torrent_file(&torrent).unwrap();
        assert!(!torrent.exists());
        // déjà absent : pas d'erreur
        super::remove_torrent_file(&torrent).unwrap();

        std::fs::remove_dir_all(&root).unwrap();
    }
}
