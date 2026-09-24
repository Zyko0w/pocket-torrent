//! Ajout : magnets, liens .torrent, fichiers locaux et liens directs.

use super::*;

/// Dossier où sont rangés les .torrent déposés dans la fenêtre, vidé à chaque démarrage.
pub(super) const DROPPED_DIR: &str = "deposes";

impl Manager {
    /// Range un .torrent déposé dans la fenêtre et renvoie son chemin, que l'interface met dans
    /// la zone d'ajout (l'utilisateur choisit encore catégorie, priorité et fichiers).
    pub fn save_dropped_torrent(&self, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
        if bytes.len() > librqbit::MAX_TORRENT_FILE_BYTES {
            bail!("Fichier trop gros pour être un vrai torrent.");
        }
        librqbit::torrent_from_bytes(bytes).map_err(|_| anyhow!("« {name} » n'est pas un fichier .torrent valide."))?;
        let dir = self.data_dir.join(DROPPED_DIR);
        std::fs::create_dir_all(&dir)?;
        let mut file_name = direct::safe_file_name(name);
        if !file_name.to_lowercase().ends_with(".torrent") {
            file_name.push_str(".torrent");
        }
        let path = direct::unique_path(&dir, &file_name);
        std::fs::write(&path, bytes)?;
        Ok(path)
    }

    pub async fn preview(&self, link: &str) -> anyhow::Result<Preview> {
        let mut preview = self.engine.preview(link).await?;
        if self.data.lock().torrents.get(&preview.info_hash).is_some_and(|e| !e.removed) {
            bail!("Ce torrent est déjà dans la liste.");
        }
        self.flag_danger(&mut preview.files);
        Ok(preview)
    }

    /// Signale (sans bloquer) les fichiers à l'extension dangereuse ou au nom trompeur.
    pub(super) fn flag_danger(&self, files: &mut [FileView]) {
        let excluded = self.settings().security.excluded_extensions;
        for file in files {
            file.danger = danger::warn_reason(&file.path, &excluded);
        }
    }

    pub async fn add_links(self: &Arc<Self>, links: Vec<String>, options: AddOptions) -> anyhow::Result<AddReport> {
        let settings = self.settings();
        if options.category.is_some() && settings.category(options.category.as_deref()).is_none() {
            bail!("Catégorie inconnue.");
        }
        if options.dest == Dest::R2 && R2::from_settings(&settings.r2).is_none() {
            bail!("Cloudflare R2 n'est pas configuré (voir Réglages).");
        }
        let mut report = AddReport { added: 0, failed: Vec::new(), errors: Vec::new() };
        for link in links.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            match self.add_link(link, &options, &settings).await {
                Ok(()) => report.added += 1,
                Err(err) => {
                    report.failed.push(link.to_owned());
                    let message = format!("{err:#}");
                    if !report.errors.contains(&message) {
                        report.errors.push(message);
                    }
                }
            }
        }
        self.save_now();
        Ok(report)
    }

    pub(super) async fn add_link(
        self: &Arc<Self>,
        link: &str,
        options: &AddOptions,
        settings: &Settings,
    ) -> anyhow::Result<()> {
        let common = Common::new(options.category.clone(), options.priority, options.dest);
        let lower = link.to_lowercase();
        let torrent_url = (lower.starts_with("http://") || lower.starts_with("https://"))
            && url::Url::parse(link).is_ok_and(|u| u.path().to_lowercase().ends_with(".torrent"));

        let local_file = Path::new(link).is_file();
        if lower.starts_with("magnet:") || torrent_url || local_file {
            if let Some(hash) = magnet_hash(link)
                && self.data.lock().torrents.get(&hash).is_some_and(|e| !e.removed)
            {
                bail!("Ce torrent est déjà dans la liste.");
            }
            if local_file {
                // rien à chercher sur le réseau : ajouté tout de suite, avant que le fichier ne
                // puisse être déplacé ou supprimé (il n'est plus relu ensuite)
                let preview = self.preview(link).await?;
                self.add_resolved(link, options.only_files.clone(), common, &preview).await?;
                return Ok(());
            }
            if options.only_files.is_some() {
                // fichiers déjà choisis dans l'aperçu : les métadonnées sont en cache
                let preview = self.engine.preview(link).await?;
                self.add_resolved(link, options.only_files.clone(), common, &preview).await?;
                return Ok(());
            }
            let id = short_id();
            self.data.lock().pending.push(PendingTorrent {
                id: id.clone(),
                link: link.to_owned(),
                name: magnet_name(link),
                only_files: None,
                common,
            });
            self.spawn_pending(id);
            return Ok(());
        }

        if !direct::is_http_url(link) {
            if crate::engine::looks_like_path(link) {
                bail!("Fichier .torrent introuvable (déplacé ou supprimé ?) : {link}");
            }
            bail!("Lien non reconnu : colle un magnet ou un lien http(s).");
        }
        let id = short_id();
        let folder = match options.dest {
            Dest::R2 => settings.download_dir.join(R2_WORK_DIR).join(&id),
            Dest::Local => settings.folder_for(options.category.as_deref()),
        };
        self.data.lock().directs.push(DirectEntry {
            id,
            link: link.to_owned(),
            kind: None,
            limit_kbps: options.limit_kbps,
            name: None,
            total_bytes: 0,
            received_bytes: 0,
            folder,
            part_path: None,
            final_path: None,
            shared_hash: None,
            common,
        });
        Ok(())
    }

    pub(super) fn spawn_pending(self: &Arc<Self>, id: String) {
        let manager = self.clone();
        let key = format!("p:{id}");
        let task = tauri::async_runtime::spawn(async move {
            let result = manager.resolve_pending(&id).await;
            manager.tasks.lock().remove(&format!("p:{id}"));
            if let Err(err) = result {
                let message = format!("{err:#}");
                let name = {
                    let mut data = manager.data.lock();
                    data.pending_mut(&id).map(|p| {
                        p.common.error = Some(message.clone());
                        p.name.clone().unwrap_or_else(|| "Torrent".into())
                    })
                };
                if let Some(name) = name {
                    manager.notify(Notify::Error, "Problème de téléchargement", &format!("{name} : {message}"));
                }
                manager.save_now();
            }
        });
        if let Some(previous) = self.tasks.lock().insert(key, task) {
            previous.abort();
        }
    }

    /// Relance la recherche des magnets en attente sur le moteur courant : après un redémarrage
    /// (réglages, VPN), les recherches lancées sur l'ancien moteur, arrêté, n'aboutiraient jamais.
    pub(super) fn respawn_pending(self: &Arc<Self>) {
        let ids: Vec<String> =
            self.data.lock().pending.iter().filter(|p| p.common.error.is_none()).map(|p| p.id.clone()).collect();
        for id in ids {
            self.spawn_pending(id);
        }
    }

    async fn resolve_pending(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let pending =
            self.data.lock().pending.iter().find(|p| p.id == id).cloned().context("Téléchargement retiré.")?;
        let search = Search { started: Instant::now(), peers_seen: Default::default() };
        self.searches.lock().insert(id.to_owned(), search.clone());
        let preview = self.engine.preview_counting(&pending.link, Some(search.peers_seen)).await;
        self.searches.lock().remove(id);
        let preview = preview?;
        self.add_resolved(&pending.link, pending.only_files.clone(), pending.common.clone(), &preview).await?;
        self.data.lock().pending.retain(|p| p.id != id);
        self.save_now();
        Ok(())
    }

    async fn add_resolved(
        self: &Arc<Self>,
        link: &str,
        only_files: Option<Vec<usize>>,
        mut common: Common,
        preview: &Preview,
    ) -> anyhow::Result<()> {
        let hash = preview.info_hash.clone();
        if self.data.lock().torrents.get(&hash).is_some_and(|e| !e.removed) {
            bail!("Ce torrent est déjà dans la liste.");
        }
        let settings = self.settings();
        let base = match common.dest {
            Dest::R2 => settings.download_dir.join(R2_WORK_DIR),
            Dest::Local => settings.folder_for(common.category.as_deref()),
        };
        let chosen: Option<HashSet<usize>> = only_files.as_ref().map(|list| list.iter().copied().collect());
        let wanted: u64 = preview
            .files
            .iter()
            .filter(|f| chosen.as_ref().is_none_or(|set| set.contains(&f.index)))
            .map(|f| f.length)
            .sum();
        let space = self.space_problem(&base, wanted);
        if self.vpn_offline() {
            common.hold = Some(Hold::Vpn);
        } else if let Some(problem) = &space {
            common.hold = Some(Hold::Disk);
            common.warning = Some(problem.clone());
        } else if self.queue_full() {
            common.hold = Some(Hold::Queue);
        }
        let paused = common.hold.is_some();

        self.adding.lock().insert(hash.clone());
        self.data.lock().torrents.insert(
            hash.clone(),
            TorrentEntry {
                common,
                name: preview.name.clone(),
                wanted_bytes: wanted,
                source_file: Path::new(link).is_file().then(|| PathBuf::from(link)),
                ..Default::default()
            },
        );
        let result = self.engine.add(link, &base, only_files, paused).await;
        self.adding.lock().remove(&hash);
        if let Err(err) = result {
            self.data.lock().torrents.remove(&hash);
            return Err(err);
        }
        let display_name = preview.name.clone().unwrap_or_else(|| hash.clone());
        hooks::run(&settings.hooks.on_add, &display_name, &base, &hash);
        if space.is_some() {
            let name = preview.name.clone().unwrap_or_else(|| "Le torrent".into());
            self.notify(Notify::Error, "Espace disque insuffisant", &format!("{name} a été mis en pause."));
        }
        self.save_now();
        Ok(())
    }
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// Empreinte hexadécimale d'un magnet (xt=urn:btih:…), si elle y figure sous cette forme.
pub(super) fn magnet_hash(link: &str) -> Option<String> {
    let url = url::Url::parse(link).ok()?;
    url.query_pairs().find_map(|(key, value)| {
        let hash = value.strip_prefix("urn:btih:")?;
        (key == "xt" && hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_lowercase())
    })
}

fn magnet_name(link: &str) -> Option<String> {
    let url = url::Url::parse(link).ok()?;
    url.query_pairs().find(|(key, _)| key == "dn").map(|(_, value)| value.into_owned())
}
