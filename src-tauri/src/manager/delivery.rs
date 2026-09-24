//! Envoi sur Cloudflare R2 une fois le téléchargement fini.

use super::*;

impl Manager {
    pub(super) fn start_delivery(self: &Arc<Self>, key: JobKey) {
        let manager = self.clone();
        let task_key = format!("upload:{}", key.id());
        let task = tauri::async_runtime::spawn(async move {
            let result = manager.deliver(&key).await;
            manager.tasks.lock().remove(&format!("upload:{}", key.id()));
            manager.on_delivery_done(&key, result);
        });
        self.tasks.lock().insert(task_key, task);
    }

    pub(super) fn with_common<R>(&self, key: &JobKey, f: impl FnOnce(&mut Common) -> R) -> Option<R> {
        let mut data = self.data.lock();
        match key {
            JobKey::Torrent(hash) => data.torrents.get_mut(hash).map(|e| f(&mut e.common)),
            JobKey::Direct(id) => data.direct_mut(id).map(|e| f(&mut e.common)),
            JobKey::Pending(id) => data.pending_mut(id).map(|e| f(&mut e.common)),
        }
    }

    async fn deliver(&self, key: &JobKey) -> anyhow::Result<()> {
        let settings = self.settings();
        let r2 = R2::from_settings(&settings.r2)
            .ok_or_else(|| anyhow!("Cloudflare R2 n'est pas configuré (voir Réglages)."))??;

        let (files, name, unique, category) = {
            let data = self.data.lock();
            match key {
                JobKey::Torrent(hash) => {
                    let entry = data.torrents.get(hash).context("téléchargement retiré")?;
                    (None, entry.name.clone(), hash.clone(), entry.common.category.clone())
                }
                JobKey::Direct(id) => {
                    let entry = data.directs.iter().find(|d| &d.id == id).context("téléchargement retiré")?;
                    let path = entry.final_path.clone().context("fichier téléchargé introuvable")?;
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(entry.total_bytes);
                    let rel = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (Some(vec![(rel, path, size)]), entry.name.clone(), id.clone(), entry.common.category.clone())
                }
                JobKey::Pending(_) => bail!("rien à envoyer"),
            }
        };
        let files = match (files, key) {
            (Some(files), _) => files,
            (None, JobKey::Torrent(hash)) => self.engine.selected_file_paths(hash)?,
            _ => unreachable!(),
        };
        let category_name = settings.category(category.as_deref()).map(|c| c.name.clone());
        let name = name.unwrap_or_else(|| "fichiers".into());
        let prefix = r2::key_prefix(category_name.as_deref(), &name, &unique);
        let total: u64 = files.iter().map(|(_, _, size)| size).sum();

        self.with_common(key, |c| {
            c.error = None;
            c.error_notified = false;
            c.delivery = Some(Delivery {
                state: DeliveryState::Uploading,
                sent_bytes: 0,
                total_bytes: total,
                links: Vec::new(),
                expires_at: None,
            });
        });
        self.save_now();

        let mut links = Vec::new();
        let mut done_before = 0u64;
        for (rel, path, size) in &files {
            let object = format!("{prefix}/{rel}");
            r2.upload(&self.http(), &object, path, |sent| {
                self.with_common(key, |c| {
                    if let Some(d) = c.delivery.as_mut() {
                        d.sent_bytes = done_before + sent;
                    }
                });
            })
            .await
            .with_context(|| format!("envoi de « {rel} »"))?;
            done_before += size;
            let file_name =
                Path::new(rel).file_name().map_or_else(|| rel.clone(), |n| n.to_string_lossy().into_owned());
            links.push(Link { name: rel.clone(), size: *size, url: r2.link(&object, &file_name), key: object });
        }

        let expires_at = (!r2.has_public_links()).then(|| now_ms() + r2.link_expiry().as_millis() as u64);
        self.with_common(key, |c| {
            c.delivery =
                Some(Delivery { state: DeliveryState::Done, sent_bytes: total, total_bytes: total, links, expires_at });
        });

        if settings.r2.delete_after_upload {
            match key {
                JobKey::Torrent(hash) => {
                    self.engine
                        .delete(hash, true)
                        .await
                        .context("fichiers envoyés, mais suppression locale impossible")?;
                    if let Some(entry) = self.data.lock().torrents.get_mut(hash) {
                        entry.removed = true;
                    }
                }
                JobKey::Direct(id) => {
                    let folder = self.data.lock().directs.iter().find(|d| &d.id == id).map(|d| d.folder.clone());
                    if let Some(folder) = folder {
                        self.safe_remove(&folder);
                    }
                    if let Some(entry) = self.data.lock().direct_mut(id) {
                        entry.final_path = None;
                    }
                }
                JobKey::Pending(_) => {}
            }
        }
        Ok(())
    }

    fn on_delivery_done(&self, key: &JobKey, result: anyhow::Result<()>) {
        let name = {
            let data = self.data.lock();
            match key {
                JobKey::Torrent(hash) => data.torrents.get(hash).and_then(|e| e.name.clone()),
                JobKey::Direct(id) => data.directs.iter().find(|d| &d.id == id).and_then(|d| d.name.clone()),
                JobKey::Pending(_) => None,
            }
            .unwrap_or_else(|| "Téléchargement".into())
        };
        match result {
            Ok(()) => {
                tracing::info!("envoyé sur R2 : {name}");
                self.notify(Notify::Done, "Envoyé sur Cloudflare R2", &name);
            }
            Err(err) => {
                let message = format!("Envoi R2 : {err:#}");
                tracing::error!("{name} : {message}");
                self.with_common(key, |c| {
                    c.error = Some(message.clone());
                    if let Some(d) = c.delivery.as_mut() {
                        d.state = DeliveryState::Error;
                    }
                });
                self.notify(Notify::Error, "Envoi sur R2 impossible", &format!("{name} : {message}"));
            }
        }
        self.save_now();
    }

    /// Supprime un fichier ou dossier, seulement s'il est dans un dossier de téléchargement.
    pub(super) fn safe_remove(&self, target: &Path) -> bool {
        use std::path::Component;
        let roots = self.settings.read().download_roots();
        // « Téléchargements\..\Windows » commence par le dossier mais en sort : chemins avec « .. » refusés
        let plain = |p: &Path| !p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir));
        let inside =
            plain(target) && roots.iter().any(|root| plain(root) && target.starts_with(root) && target != root);
        if !inside {
            tracing::error!("suppression refusée (hors des dossiers de téléchargement) : {}", target.display());
            return false;
        }
        let result = if target.is_dir() { std::fs::remove_dir_all(target) } else { std::fs::remove_file(target) };
        match result {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
            Err(err) => {
                tracing::warn!("suppression impossible de {} : {err}", target.display());
                false
            }
        }
    }
}
