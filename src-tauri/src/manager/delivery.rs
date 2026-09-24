//! Envoi sur storage.to une fois le téléchargement fini.

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
        let storage = StorageTo::from_settings(&settings.storage_to)?;

        let (files, name) = {
            let data = self.data.lock();
            match key {
                JobKey::Torrent(hash) => {
                    let entry = data.torrents.get(hash).context("téléchargement retiré")?;
                    (None, entry.name.clone())
                }
                JobKey::Direct(id) => {
                    let entry = data.directs.iter().find(|d| &d.id == id).context("téléchargement retiré")?;
                    let path = entry.final_path.clone().context("fichier téléchargé introuvable")?;
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(entry.total_bytes);
                    let rel = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (Some(vec![(rel, path, size)]), entry.name.clone())
                }
                JobKey::Pending(_) => bail!("rien à envoyer"),
            }
        };
        let files = match (files, key) {
            (Some(files), _) => files,
            (None, JobKey::Torrent(hash)) => self.engine.selected_file_paths(hash)?,
            _ => unreachable!(),
        };
        let name = name.unwrap_or_else(|| "fichiers".into());
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

        let http = self.http();
        let mut links = Vec::new();
        // plusieurs fichiers : une collection, un seul lien pour le tout (en tête de liste)
        let collection = if files.len() > 1 {
            let collection = storage.create_collection(&http, files.len()).await?;
            links.push(Link {
                name: format!("{name} (tous les fichiers)"),
                size: total,
                key: collection.id.clone(),
                url: collection.url.clone(),
                kind: "collection".into(),
                owner_token: collection.owner_token.clone(),
            });
            Some(collection)
        } else {
            None
        };

        let mut done_before = 0u64;
        for (rel, path, size) in &files {
            let file_name =
                Path::new(rel).file_name().map_or_else(|| rel.clone(), |n| n.to_string_lossy().into_owned());
            let shared = storage
                .upload(&http, path, &file_name, collection.as_ref(), |sent| {
                    self.with_common(key, |c| {
                        if let Some(d) = c.delivery.as_mut() {
                            d.sent_bytes = done_before + sent;
                        }
                    });
                })
                .await
                .with_context(|| format!("envoi de « {rel} »"))?;
            done_before += size;
            links.push(Link {
                name: rel.clone(),
                size: *size,
                key: shared.id,
                url: shared.url,
                kind: "file".into(),
                owner_token: shared.owner_token,
            });
            // liens enregistrés au fil de l'eau : un arrêt en cours d'envoi permet quand même
            // de supprimer ce qui est déjà en ligne
            self.with_common(key, |c| {
                if let Some(d) = c.delivery.as_mut() {
                    d.links = links.clone();
                }
            });
        }

        let expires_at = Some(now_ms() + u64::from(settings.storage_to.expiry_days) * 86_400_000);
        self.with_common(key, |c| {
            c.delivery =
                Some(Delivery { state: DeliveryState::Done, sent_bytes: total, total_bytes: total, links, expires_at });
        });

        if settings.storage_to.delete_after_upload {
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
                tracing::info!("envoyé sur storage.to : {name}");
                self.notify(Notify::Done, "Envoyé sur storage.to", &name);
            }
            Err(err) => {
                let message = format!("Envoi en ligne : {err:#}");
                tracing::error!("{name} : {message}");
                self.with_common(key, |c| {
                    c.error = Some(message.clone());
                    if let Some(d) = c.delivery.as_mut() {
                        d.state = DeliveryState::Error;
                    }
                });
                self.notify(Notify::Error, "Envoi sur storage.to impossible", &format!("{name} : {message}"));
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
