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

    /// Envoi à la demande d'un téléchargement terminé gardé sur le PC, avec ses propres choix
    /// (durée de vie, limite de téléchargements, effacement local) à la place des réglages.
    pub fn send_online(self: &Arc<Self>, id: &str, options: SendOptions) -> anyhow::Result<()> {
        if !(1..=7).contains(&options.expiry_days) {
            bail!("Durée des liens entre 1 et 7 jours.");
        }
        if options.max_downloads > crate::settings::MAX_DOWNLOADS {
            bail!("Au plus {} téléchargements avant effacement.", crate::settings::MAX_DOWNLOADS);
        }
        let key = JobKey::parse(id)?;
        if self.tasks.lock().contains_key(&format!("upload:{}", key.id())) {
            bail!("Envoi déjà en cours.");
        }
        {
            let mut data = self.data.lock();
            let common = match &key {
                JobKey::Torrent(hash) => {
                    let entry = data.torrents.get_mut(hash).context("Téléchargement introuvable.")?;
                    if entry.removed {
                        bail!("Fichiers supprimés : rien à envoyer.");
                    }
                    if entry.final_folder.is_some() {
                        bail!("Fichiers pas encore rangés dans leur dossier final : réessaie dans un instant.");
                    }
                    &mut entry.common
                }
                JobKey::Direct(id) => {
                    let entry = data.direct_mut(id).context("Téléchargement introuvable.")?;
                    if !entry.final_path.as_deref().is_some_and(Path::is_file) {
                        bail!("Fichier téléchargé introuvable.");
                    }
                    &mut entry.common
                }
                JobKey::Pending(_) => bail!("Rien à envoyer."),
            };
            if common.finished_at.is_none() {
                bail!("Le téléchargement n'est pas terminé.");
            }
            if common.delivery.as_ref().is_some_and(|d| d.state != DeliveryState::Error) {
                bail!("Déjà envoyé sur storage.to.");
            }
            // gardé avec le téléchargement : un envoi repris au prochain lancement les respecte
            common.send_options = Some(options);
        }
        self.save_now();
        self.start_delivery(key);
        Ok(())
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
        let (files, name, dest, options) = {
            let data = self.data.lock();
            match key {
                JobKey::Torrent(hash) => {
                    let entry = data.torrents.get(hash).context("téléchargement retiré")?;
                    (None, entry.name.clone(), entry.common.dest, entry.common.send_options)
                }
                JobKey::Direct(id) => {
                    let entry = data.directs.iter().find(|d| &d.id == id).context("téléchargement retiré")?;
                    let path = entry.final_path.clone().context("fichier téléchargé introuvable")?;
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(entry.total_bytes);
                    let rel = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (Some(vec![(rel, path, size)]), entry.name.clone(), entry.common.dest, entry.common.send_options)
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
        let storage = StorageTo::from_settings(&settings.storage_to)?;
        let (storage, expiry_days, delete_local) = match options {
            Some(o) => (storage.with_limits(o.expiry_days, o.max_downloads), o.expiry_days, o.delete_local),
            // réglages : seulement pour « Envoyer sur storage.to » choisi à l'ajout
            None => {
                let delete = settings.storage_to.delete_after_upload && dest == Dest::StorageTo;
                (storage, settings.storage_to.expiry_days, delete)
            }
        };
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

        let expires_at = Some(now_ms() + u64::from(expiry_days) * 86_400_000);
        self.with_common(key, |c| {
            c.delivery =
                Some(Delivery { state: DeliveryState::Done, sent_bytes: total, total_bytes: total, links, expires_at });
        });

        if delete_local {
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
                    let target = self.data.lock().directs.iter().find(|d| &d.id == id).map(|d| match dest {
                        // dossier de travail propre à cet envoi
                        Dest::StorageTo => d.folder.clone(),
                        // gardé sur le PC : son dossier est celui des téléchargements, seul le fichier part
                        Dest::Local => d.final_path.clone().unwrap_or_default(),
                    });
                    if let Some(target) = target.filter(|t| !t.as_os_str().is_empty()) {
                        self.safe_remove(&target);
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
