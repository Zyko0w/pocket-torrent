//! Ajout de torrents : liens, aperçu, choix des fichiers, pause, reprise, suppression.

use super::*;

/// Torrent déjà résolu lors d'un aperçu (voir `Engine::resolved`).
#[derive(Clone)]
pub(super) struct Resolved {
    pub(super) preview: Preview,
    pub(super) bytes: Bytes,
}

/// Fichiers choisis sous forme de table (un booléen par fichier du torrent), `None` si tous le
/// sont. Chercher chaque fichier dans la liste serait quadratique : des secondes de calcul, à
/// chaque seconde, pour un torrent de dizaines de milliers de fichiers.
pub(super) fn selection_mask(only_files: Option<&[usize]>, file_count: usize) -> Option<Vec<bool>> {
    only_files.map(|list| {
        let mut mask = vec![false; file_count];
        for &index in list {
            if let Some(slot) = mask.get_mut(index) {
                *slot = true;
            }
        }
        mask
    })
}

pub(super) fn is_selected(mask: Option<&[bool]>, index: usize) -> bool {
    mask.is_none_or(|m| m.get(index).copied().unwrap_or(false))
}

/// Pairs indiqués directement dans un magnet (paramètres `x.pe=ip:port`).
pub(super) fn magnet_peers(link: &str) -> Option<Vec<SocketAddr>> {
    let url = url::Url::parse(link.trim()).ok().filter(|u| u.scheme() == "magnet")?;
    let peers: Vec<SocketAddr> =
        url.query_pairs().filter(|(key, _)| key == "x.pe").filter_map(|(_, value)| value.parse().ok()).collect();
    (!peers.is_empty()).then_some(peers)
}

/// Magnet, lien http(s) vers un .torrent, ou chemin d'un fichier .torrent local.
pub(super) fn add_source(link: &str) -> anyhow::Result<AddTorrent<'static>> {
    let link = link.trim();
    if link.starts_with("magnet:") {
        check_magnet(link)?;
    }
    if link.starts_with("magnet:") || link.starts_with("http://") || link.starts_with("https://") {
        return Ok(AddTorrent::from_url(link.to_owned()));
    }
    if Path::new(link).is_file() {
        // même plafond que pour un .torrent téléchargé : le fichier est lu en entier en mémoire
        if std::fs::metadata(link).is_ok_and(|m| m.len() > librqbit::MAX_TORRENT_FILE_BYTES as u64) {
            bail!("Fichier .torrent trop gros pour être un vrai torrent.");
        }
        return AddTorrent::from_local_filename(link);
    }
    if looks_like_path(link) {
        bail!("Fichier .torrent introuvable (déplacé ou supprimé ?) : {link}");
    }
    bail!("Lien non reconnu : colle un magnet, un lien http(s) ou choisis un fichier .torrent.")
}

/// Magnet lisible par le moteur : empreinte v1 (`urn:btih:`, hexadécimale ou base 32). Un magnet
/// hybride (v1 + v2 `urn:btmh:`) passe par son empreinte v1 ; un magnet v2 seul est refusé avec un
/// message clair, le moteur ne gérant pas encore BitTorrent v2.
pub(super) fn check_magnet(link: &str) -> anyhow::Result<()> {
    let magnet = librqbit::Magnet::parse(link).map_err(|_| anyhow!("Lien magnet invalide ou incomplet."))?;
    if magnet.as_id20().is_none() {
        bail!(
            "Ce magnet est au format BitTorrent v2 seul (urn:btmh), pas encore pris en charge.              Cherche la version hybride (v1 + v2) ou le fichier .torrent."
        );
    }
    Ok(())
}

/// Chemin de fichier plutôt que lien (« C:\… », « \\serveur\… », « /… » ou « ….torrent »).
pub fn looks_like_path(link: &str) -> bool {
    let bytes = link.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || link.starts_with('\\') || link.starts_with('/') || link.to_lowercase().ends_with(".torrent")
}

/// Temps laissé aux pairs pour transmettre les infos d'un magnet avant d'abandonner (l'utilisateur
/// peut réessayer) : sans limite, un magnet sans source resterait « Recherche du nom… » à vie.
pub(super) const MAGNET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Traduit les erreurs du moteur que l'utilisateur peut corriger lui-même.
pub(super) fn explain_add_error(err: anyhow::Error, binding: &Binding) -> anyhow::Error {
    let text = format!("{err:#}");
    if text.contains("no known way to resolve peers") {
        return match binding {
            Binding::Offline => {
                anyhow!("VPN coupé : impossible de chercher ce magnet tant que le VPN n'est pas revenu.")
            }
            _ => anyhow!(
                "Impossible de chercher ce magnet : la DHT est coupée (mode discret ou confidentialité renforcée, \
                 Réglages › Confidentialité) et le lien ne contient aucun tracker HTTPS. Désactive ce mode ou \
                 utilise un fichier .torrent."
            ),
        };
    }
    if text.contains("input address stream exhausted") {
        return anyhow!("Aucun pair n'a transmis les infos de ce torrent. Réessaie plus tard.");
    }
    err
}

impl Engine {
    // ---------------------------------------------------------------- actions

    /// Récupère le nom et la liste des fichiers sans lancer le téléchargement.
    pub async fn preview(&self, link: &str) -> anyhow::Result<Preview> {
        self.preview_counting(link, None).await
    }

    /// Comme `preview`, en comptant dans `peers_seen` les pairs trouvés pendant la recherche des
    /// infos d'un magnet (affichés pendant « Recherche »).
    pub async fn preview_counting(
        &self,
        link: &str,
        peers_seen: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> anyhow::Result<Preview> {
        let key = link.trim().to_owned();
        if let Some(resolved) = self.resolved.lock().get(&key) {
            return Ok(resolved.preview.clone());
        }
        let session = self.session();
        let lookup = session.add_torrent(
            add_source(link)?,
            Some(AddTorrentOptions {
                list_only: true,
                initial_peers: magnet_peers(link),
                magnet_peers_seen: peers_seen,
                ..Default::default()
            }),
        );
        let response = match tokio::time::timeout(MAGNET_TIMEOUT, lookup).await {
            Ok(result) => result.map_err(|err| explain_add_error(err, &self.binding())),
            Err(_) => Err(anyhow!(
                "Aucun pair n'a transmis les infos de ce torrent en {} min (torrent sans sources ?). Réessaie plus tard.",
                MAGNET_TIMEOUT.as_secs() / 60
            )),
        }
        .context("impossible de lire ce torrent")?;
        match response {
            AddTorrentResponse::ListOnly(list) => {
                let files = list
                    .info
                    .iter_file_details_ext()
                    .enumerate()
                    .filter(|(_, file)| !file.details.attrs().padding)
                    .map(|(index, file)| FileView {
                        index,
                        path: file.details.filename.to_pathbuf().to_string_lossy().replace('\\', "/"),
                        length: file.details.len,
                        included: true,
                        progress_bytes: 0,
                        danger: None,
                    })
                    .collect();
                let preview = Preview {
                    info_hash: list.info_hash.as_string(),
                    name: list.info.name().map(|n| n.into_owned()),
                    total_bytes: list.info.lengths().total_length(),
                    files,
                };
                let mut cache = self.resolved.lock();
                if cache.len() > 50 {
                    cache.clear();
                }
                cache.insert(key, Resolved { preview: preview.clone(), bytes: list.torrent_bytes });
                Ok(preview)
            }
            AddTorrentResponse::AlreadyManaged(..) => bail!("Ce torrent est déjà dans la liste."),
            AddTorrentResponse::Added(..) => bail!("Réponse inattendue du moteur torrent."),
        }
    }

    /// Ajoute un torrent (résolu au besoin). Un torrent à plusieurs fichiers
    /// est rangé dans son propre sous-dossier de `base_folder`.
    pub async fn add(
        &self,
        link: &str,
        base_folder: &Path,
        only_files: Option<Vec<usize>>,
        paused: bool,
    ) -> anyhow::Result<String> {
        if only_files.as_ref().is_some_and(|files| files.is_empty()) {
            bail!("Choisis au moins un fichier.");
        }
        self.preview(link).await?;
        let resolved = self.resolved.lock().remove(link.trim()).ok_or_else(|| anyhow!("Torrent introuvable."))?;
        let folder = if resolved.preview.files.len() > 1 {
            let name = resolved.preview.name.clone().unwrap_or_else(|| resolved.preview.info_hash.clone());
            base_folder.join(safe_file_name(&name))
        } else {
            base_folder.to_path_buf()
        };
        let response = self
            .session()
            .add_torrent(
                AddTorrent::from_bytes(resolved.bytes),
                Some(AddTorrentOptions {
                    paused,
                    only_files,
                    output_folder: Some(folder.to_string_lossy().into_owned()),
                    initial_peers: magnet_peers(link),
                    // permet de reprendre des fichiers partiellement présents sur le disque
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("impossible d'ajouter ce torrent")?;
        match response {
            AddTorrentResponse::Added(_, handle) => Ok(handle.info_hash().as_string()),
            AddTorrentResponse::AlreadyManaged(..) => bail!("Ce torrent est déjà dans la liste."),
            AddTorrentResponse::ListOnly(_) => bail!("Réponse inattendue du moteur torrent."),
        }
    }

    pub async fn pause(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        if !torrent.is_paused() {
            self.session().pause(&torrent).await?;
        }
        Ok(())
    }

    pub async fn resume(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        if torrent.is_paused() {
            self.session().unpause(&torrent).await?;
        }
        Ok(())
    }

    /// Retire le torrent du moteur ; avec `delete_files`, efface aussi ses fichiers et dossiers vides.
    pub async fn delete(&self, hash: &str, delete_files: bool) -> anyhow::Result<()> {
        let id = TorrentIdOrHash::parse(hash)?;
        self.session().delete(id, delete_files).await?;
        self.peer_samples.lock().retain(|(torrent, _), _| torrent != hash);
        Ok(())
    }

    /// Revérifie un torrent depuis le disque (§5, « revérification forcée ») : le retire du
    /// moteur (fichiers conservés) puis le rajoute avec les mêmes métadonnées, fichiers
    /// choisis et dossier — ce qui force une nouvelle vérification complète de ce qui est
    /// déjà présent sur le disque, sans retélécharger les métadonnées auprès des pairs.
    pub async fn recheck(&self, hash: &str) -> anyhow::Result<()> {
        let torrent = self.handle(hash)?;
        let bytes = torrent.with_metadata(|meta| meta.torrent_bytes.clone())?;
        let only_files = torrent.only_files();
        let output_folder = torrent.output_folder().to_path_buf();
        let paused = torrent.is_paused();
        let id = TorrentIdOrHash::parse(hash).map_err(|_| anyhow!("Torrent inconnu."))?;
        self.session().delete(id, false).await?;
        self.session()
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    paused,
                    only_files,
                    output_folder: Some(output_folder.to_string_lossy().into_owned()),
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("impossible de relancer la vérification")?;
        Ok(())
    }

    /// Déplace un torrent terminé de `from_base` vers `to_base` : retiré du moteur (fichiers
    /// fermés, sinon Windows refuse de les déplacer), fichiers déplacés (tout ou rien, voir
    /// `relocate`), puis rajouté à son nouvel emplacement, où sa vérification retrouve toutes
    /// les pièces. En cas d'échec, il est rajouté là où sont restés ses fichiers. Renvoie son
    /// nouveau dossier.
    pub async fn relocate(
        &self,
        hash: &str,
        from_base: &Path,
        to_base: &Path,
        paused: bool,
    ) -> anyhow::Result<PathBuf> {
        let torrent = self.handle(hash)?;
        let bytes = torrent.with_metadata(|meta| meta.torrent_bytes.clone())?;
        let rels: Vec<PathBuf> = torrent.with_metadata(|meta| {
            meta.file_infos.iter().filter(|f| !f.attrs.padding).map(|f| f.relative_filename.clone()).collect()
        })?;
        let only_files = torrent.only_files();
        let old = torrent.output_folder().to_path_buf();
        // dossier propre au torrent (plusieurs fichiers) gardé tel quel à l'arrivée
        let new = match old.strip_prefix(from_base) {
            Ok(rest) => to_base.join(rest),
            Err(_) => {
                old.file_name().filter(|_| old != from_base).map_or_else(|| to_base.to_path_buf(), |n| to_base.join(n))
            }
        };
        drop(torrent);
        let id = TorrentIdOrHash::parse(hash).map_err(|_| anyhow!("Torrent inconnu."))?;
        self.session().delete(id, false).await?;

        let moved = {
            let (old, new, rels, keep) = (old.clone(), new.clone(), rels.clone(), from_base.to_path_buf());
            tokio::task::spawn_blocking(move || {
                let result = crate::relocate::move_tree(&old, &new, &rels);
                if result.is_ok() {
                    crate::relocate::remove_empty_dirs(&old, &rels, &keep);
                }
                result
            })
            .await
            .map_err(|err| anyhow!("déplacement interrompu : {err}"))
            .and_then(|r| r)
        };
        let folder = if moved.is_ok() { &new } else { &old };
        self.session()
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    paused,
                    only_files,
                    output_folder: Some(folder.to_string_lossy().into_owned()),
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("impossible de reprendre le torrent après le déplacement")?;
        moved.map(|()| new)
    }

    pub async fn set_files(&self, hash: &str, files: Vec<usize>) -> anyhow::Result<()> {
        let files: HashSet<usize> = files.into_iter().collect();
        if files.is_empty() {
            bail!("Choisis au moins un fichier.");
        }
        let torrent = self.handle(hash)?;
        self.session().update_only_files(&torrent, &files).await
    }
}
