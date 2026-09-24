//! Dossiers surveillés (§5 du plan d'upgrade) : tout fichier `.torrent` qui apparaît dans un
//! dossier configuré est ajouté automatiquement, puis déplacé dans un sous-dossier `.traités`
//! pour ne pas être repris deux fois. Surveillance native (`ReadDirectoryChangesW` sous
//! Windows, via la crate `notify`), pas de scrutation périodique.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tauri::async_runtime::JoinHandle;
use tokio::sync::mpsc;

use crate::manager::{AddOptions, Manager};
use crate::settings::WatchedFolder;

/// Sous-dossier où sont déplacés les fichiers `.torrent` déjà ajoutés.
const DONE_DIR: &str = ".traités";

pub struct WatcherHandle {
    _watcher: notify::RecommendedWatcher,
    task: JoinHandle<()>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Démarre la surveillance d'un dossier. `None` si le dossier est inaccessible (déjà
/// signalé au journal) : les autres dossiers surveillés continuent normalement.
pub fn watch(manager: Arc<Manager>, folder: WatchedFolder) -> Option<WatcherHandle> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })
    .map_err(|err| tracing::warn!("dossier surveillé « {} » : {err:#}", folder.path.display()))
    .ok()?;
    watcher
        .watch(&folder.path, RecursiveMode::NonRecursive)
        .map_err(|err| tracing::warn!("dossier surveillé « {} » : {err:#}", folder.path.display()))
        .ok()?;

    scan_existing(&manager, &folder);

    let category = folder.category.clone();
    let task = tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                continue;
            }
            for file in event.paths.iter().filter(|p| is_torrent_file(p)) {
                add_and_move(&manager, file, category.as_deref()).await;
            }
        }
    });
    Some(WatcherHandle { _watcher: watcher, task })
}

fn is_torrent_file(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// Fichiers déjà présents au démarrage (arrivés pendant que l'application ne tournait pas).
fn scan_existing(manager: &Arc<Manager>, folder: &WatchedFolder) {
    let Ok(entries) = std::fs::read_dir(&folder.path) else { return };
    for path in entries.flatten().map(|e| e.path()).filter(|p| is_torrent_file(p)) {
        let manager = manager.clone();
        let category = folder.category.clone();
        tauri::async_runtime::spawn(async move { add_and_move(&manager, &path, category.as_deref()).await });
    }
}

async fn add_and_move(manager: &Arc<Manager>, file: &Path, category: Option<&str>) {
    // le fichier peut encore être en cours d'écriture (téléchargement du navigateur) : petite marge
    tokio::time::sleep(Duration::from_millis(500)).await;
    if !file.is_file() {
        return;
    }
    // copie gardée par l'appli : le fichier d'origine est rangé dans « .traités » juste après,
    // alors que la recherche des infos du torrent le lit plus tard (et à chaque nouvel essai)
    let name = file.file_name().map_or_else(|| "surveille.torrent".into(), |n| n.to_string_lossy().into_owned());
    let copy = std::fs::metadata(file)
        .ok()
        .filter(|m| m.len() <= librqbit::MAX_TORRENT_FILE_BYTES as u64)
        .and_then(|_| std::fs::read(file).ok())
        .map(|bytes| manager.save_dropped_torrent(&name, &bytes));
    let link = match copy {
        Some(Ok(path)) => path.to_string_lossy().into_owned(),
        Some(Err(err)) => {
            tracing::warn!("dossier surveillé : {} : {err:#}", file.display());
            move_to_done(file);
            return;
        }
        None => {
            tracing::warn!("dossier surveillé : {} illisible ou trop gros", file.display());
            move_to_done(file);
            return;
        }
    };
    let options = AddOptions { category: category.map(str::to_owned), ..Default::default() };
    match manager.add_links(vec![link], options).await {
        Ok(report) if report.added > 0 => tracing::info!("dossier surveillé : {} ajouté", file.display()),
        Ok(report) => {
            tracing::warn!("dossier surveillé : {} non ajouté ({})", file.display(), report.errors.join(", "))
        }
        Err(err) => tracing::warn!("dossier surveillé : {} : {err:#}", file.display()),
    }
    move_to_done(file);
}

/// Range le fichier traité de côté, sans jamais l'effacer.
fn move_to_done(file: &Path) {
    let Some(parent) = file.parent() else { return };
    let done_dir = parent.join(DONE_DIR);
    if std::fs::create_dir_all(&done_dir).is_err() {
        return;
    }
    let Some(name) = file.file_name() else { return };
    let mut target = done_dir.join(name);
    let mut i = 1;
    while target.exists() {
        target = done_dir.join(format!("{i}-{}", name.to_string_lossy()));
        i += 1;
    }
    let _ = std::fs::rename(file, target);
}
