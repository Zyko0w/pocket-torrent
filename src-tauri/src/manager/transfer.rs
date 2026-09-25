//! Export et import de la liste des téléchargements, pour changer de PC : un fichier JSON
//! lisible qui contient le .torrent d'origine de chaque torrent (pas besoin d'attendre les
//! métadonnées des pairs), sa catégorie, sa priorité et ses fichiers choisis, ainsi que les
//! liens directs pas encore terminés. Les fichiers téléchargés eux-mêmes ne sont pas dedans.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::*;

const FORMAT: &str = "pocket-torrent-liste";
const VERSION: u32 = 1;
/// Plafond du fichier importé (des milliers de .torrent tiennent largement dedans).
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

/// Lien d'une liste importée : magnet ou adresse web seulement. Une liste peut venir de
/// n'importe qui ; un chemin de fichier y serait ouvert par l'appli, et un chemin réseau
/// (« \\serveur\partage\x.torrent ») ferait envoyer par Windows l'empreinte du mot de passe
/// de la session au serveur désigné (connexion SMB automatique).
fn importable_link(link: &str) -> bool {
    let link = link.trim();
    link.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("magnet:")) || direct::is_http_url(link)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListFile {
    format: String,
    version: u32,
    exported_at: u64,
    torrents: Vec<ExportedTorrent>,
    directs: Vec<ExportedDirect>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedTorrent {
    name: Option<String>,
    /// Fichier .torrent en base64, ou à défaut un lien (magnet, http).
    torrent: Option<String>,
    link: Option<String>,
    category: Option<ExportedCategory>,
    priority: Priority,
    only_files: Option<Vec<usize>>,
    finished: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportedDirect {
    link: String,
    name: Option<String>,
    category: Option<ExportedCategory>,
    priority: Priority,
}

/// Catégorie par identifiant et par nom : sur l'autre PC, l'une ou l'autre peut correspondre.
#[derive(Serialize, Deserialize)]
struct ExportedCategory {
    id: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    pub added: usize,
    /// Déjà dans la liste, ou terminés quand on ne les importe pas.
    pub skipped: usize,
    pub errors: Vec<String>,
}

fn exported_category(settings: &Settings, id: Option<&str>) -> Option<ExportedCategory> {
    settings.category(id).map(|c| ExportedCategory { id: c.id.clone(), name: c.name.clone() })
}

fn local_category(settings: &Settings, category: Option<&ExportedCategory>) -> Option<String> {
    let category = category?;
    settings
        .categories
        .iter()
        .find(|c| c.id == category.id)
        .or_else(|| settings.categories.iter().find(|c| c.name.eq_ignore_ascii_case(&category.name)))
        .map(|c| c.id.clone())
}

impl Manager {
    /// Torrent déjà présent dans la liste (et pas seulement dans l'historique d'un envoi en ligne).
    fn is_listed(&self, hash: &str) -> bool {
        self.data.lock().torrents.get(hash).is_some_and(|e| !e.removed)
    }

    /// Écrit la liste dans `path` et renvoie le nombre de téléchargements exportés.
    pub fn export_list(&self, path: &Path) -> anyhow::Result<usize> {
        let settings = self.settings();
        let data = self.data.lock().clone();
        let mut torrents = Vec::new();
        for (hash, entry) in data.torrents.iter().filter(|(_, e)| !e.removed) {
            let (torrent, only_files, link) = match self.engine.export_info(hash) {
                Some((bytes, only_files)) => (Some(BASE64.encode(&bytes)), only_files, None),
                None => (None, None, self.engine.magnet(hash).ok()),
            };
            if torrent.is_none() && link.is_none() {
                continue;
            }
            torrents.push(ExportedTorrent {
                name: entry.name.clone(),
                torrent,
                link,
                category: exported_category(&settings, entry.common.category.as_deref()),
                priority: entry.common.priority,
                only_files,
                finished: entry.common.finished_at.is_some(),
            });
        }
        for pending in &data.pending {
            // un .torrent local n'a pas de sens sur un autre PC : son contenu est embarqué
            let local = Path::new(&pending.link).is_file().then(|| std::fs::read(&pending.link).ok()).flatten();
            torrents.push(ExportedTorrent {
                name: pending.name.clone(),
                torrent: local.as_ref().map(|bytes| BASE64.encode(bytes)),
                link: local.is_none().then(|| pending.link.clone()),
                category: exported_category(&settings, pending.common.category.as_deref()),
                priority: pending.common.priority,
                only_files: pending.only_files.clone(),
                finished: false,
            });
        }
        let directs: Vec<ExportedDirect> = data
            .directs
            .iter()
            .filter(|d| d.common.finished_at.is_none())
            .map(|d| ExportedDirect {
                link: d.link.clone(),
                name: d.name.clone(),
                category: exported_category(&settings, d.common.category.as_deref()),
                priority: d.common.priority,
            })
            .collect();
        let count = torrents.len() + directs.len();
        let file = ListFile { format: FORMAT.into(), version: VERSION, exported_at: now_ms(), torrents, directs };
        std::fs::write(path, serde_json::to_vec_pretty(&file)?).context("écriture du fichier impossible")?;
        tracing::info!("liste exportée : {count} téléchargement(s)");
        Ok(count)
    }

    /// Ajoute les téléchargements d'une liste exportée. Les terminés ne sont repris que si
    /// `include_finished` : sans leurs fichiers sur ce PC, ils seraient téléchargés à nouveau.
    pub async fn import_list(self: &Arc<Self>, path: &Path, include_finished: bool) -> anyhow::Result<TransferReport> {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_IMPORT_BYTES) {
            bail!("Fichier trop gros pour une liste Pocket Torrent.");
        }
        let text = std::fs::read(path).context("fichier illisible")?;
        let file: ListFile =
            serde_json::from_slice(&text).context("ce n'est pas une liste exportée par Pocket Torrent")?;
        if file.format != FORMAT || file.version > VERSION {
            bail!("Liste d'un format inconnu (exportée par une version plus récente ?).");
        }
        let settings = self.settings();
        let mut report = TransferReport { added: 0, skipped: 0, errors: Vec::new() };
        let fail = |report: &mut TransferReport, name: &str, err: anyhow::Error| {
            let message = format!("{name} : {err:#}");
            tracing::warn!("import : {message}");
            report.errors.push(message);
        };

        for torrent in file.torrents {
            let name = torrent.name.clone().unwrap_or_else(|| "Torrent".into());
            if torrent.finished && !include_finished {
                report.skipped += 1;
                continue;
            }
            let (link, hash) = match (&torrent.torrent, &torrent.link) {
                (Some(encoded), _) => {
                    let bytes = match BASE64.decode(encoded) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            fail(&mut report, &name, err.into());
                            continue;
                        }
                    };
                    let hash = librqbit::torrent_from_bytes(&bytes).ok().map(|t| t.info_hash.as_string());
                    if hash.as_deref().is_some_and(|h| self.is_listed(h)) {
                        report.skipped += 1;
                        continue; // déjà là : inutile de réécrire le .torrent
                    }
                    let saved = match self.save_dropped_torrent(&name, &bytes) {
                        Ok(path) => path.to_string_lossy().into_owned(),
                        Err(err) => {
                            fail(&mut report, &name, err);
                            continue;
                        }
                    };
                    (saved, hash)
                }
                (None, Some(link)) if importable_link(link) => (link.clone(), add::magnet_hash(link)),
                (None, Some(_)) => {
                    fail(&mut report, &name, anyhow!("lien refusé (magnet ou http(s) seulement)"));
                    continue;
                }
                (None, None) => continue,
            };
            if hash.as_deref().is_some_and(|h| self.is_listed(h)) {
                report.skipped += 1;
                continue;
            }
            let options = AddOptions {
                category: local_category(&settings, torrent.category.as_ref()),
                priority: torrent.priority,
                only_files: torrent.only_files.clone(),
                ..Default::default()
            };
            match self.add_link(&link, &options, &settings).await {
                Ok(()) => report.added += 1,
                Err(err) => fail(&mut report, &name, err),
            }
        }

        for direct in file.directs {
            if self.data.lock().directs.iter().any(|d| d.link == direct.link && d.common.finished_at.is_none()) {
                report.skipped += 1;
                continue;
            }
            let options = AddOptions {
                category: local_category(&settings, direct.category.as_ref()),
                priority: direct.priority,
                ..Default::default()
            };
            let name = direct.name.clone().unwrap_or_else(|| direct.link.clone());
            if !importable_link(&direct.link) {
                fail(&mut report, &name, anyhow!("lien refusé (http(s) seulement)"));
                continue;
            }
            match self.add_link(&direct.link, &options, &settings).await {
                Ok(()) => report.added += 1,
                Err(err) => fail(&mut report, &name, err),
            }
        }
        self.save_now();
        tracing::info!(
            "liste importée : {} ajouté(s), {} ignoré(s), {} erreur(s)",
            report.added,
            report.skipped,
            report.errors.len()
        );
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_lists_cannot_point_to_files_or_network_shares() {
        for link in [
            r"\\attaquant.example\partage\x.torrent",
            r"\\?\UNC\attaquant.example\partage\x.torrent",
            "//attaquant.example/partage/x.torrent",
            r"C:\Users\moi\Documents\secret.torrent",
            "file://attaquant.example/partage/x.torrent",
            "ftp://attaquant.example/x.torrent",
            "",
        ] {
            assert!(!importable_link(link), "{link}");
        }
        assert!(importable_link("magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a"));
        assert!(importable_link("https://site.example/x.torrent"));
        assert!(importable_link("http://site.example/film.mkv"));
    }

    #[test]
    fn list_file_round_trip() {
        let file = ListFile {
            format: FORMAT.into(),
            version: VERSION,
            exported_at: 1,
            torrents: vec![ExportedTorrent {
                name: Some("Film".into()),
                torrent: Some(BASE64.encode(b"d4:infodee")),
                link: None,
                category: Some(ExportedCategory { id: "films".into(), name: "Films".into() }),
                priority: Priority::High,
                only_files: Some(vec![0, 2]),
                finished: true,
            }],
            directs: vec![ExportedDirect {
                link: "https://x/y.iso".into(),
                name: None,
                category: None,
                priority: Priority::Low,
            }],
        };
        let text = serde_json::to_string(&file).unwrap();
        let back: ListFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.torrents[0].only_files, Some(vec![0, 2]));
        assert_eq!(BASE64.decode(back.torrents[0].torrent.as_ref().unwrap()).unwrap(), b"d4:infodee");
        assert_eq!(back.directs[0].priority, Priority::Low);
    }
}
