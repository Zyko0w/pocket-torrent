//! Archive zip avant l'envoi en ligne : un téléchargement de plusieurs fichiers part en un seul
//! fichier, rangement des dossiers compris. Les hébergeurs acceptent mal les envois en rafale
//! (et certains refusent les exécutables), alors qu'un zip passe partout.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, bail};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Fichier d'un téléchargement : chemin relatif (séparateurs `/`), chemin sur le disque, taille.
pub type Entry = (String, PathBuf, u64);

/// Extensions que les hébergeurs refusent souvent seules : envoyées dans un zip.
const EXECUTABLES: &[&str] = &[
    "exe", "msi", "com", "scr", "bat", "cmd", "ps1", "vbs", "js", "jar", "dll", "sys", "apk", "lnk", "pif", "reg",
    "hta", "cpl", "msix", "appx",
];

/// Formats déjà compressés : rangés tels quels (les recompresser ne gagne rien et prend du temps).
const COMPRESSED: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "webm", "mp3", "m4a", "aac", "ogg", "opus", "flac", "jpg", "jpeg", "png",
    "gif", "webp", "zip", "rar", "7z", "gz", "xz", "bz2", "zst", "cab", "pdf", "epub", "docx", "xlsx", "pptx",
];

fn extension(name: &str) -> String {
    let name = name.rsplit('/').next().unwrap_or(name);
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Faut-il tout mettre dans un zip avant l'envoi ? Oui pour plusieurs fichiers (un seul lien,
/// dossiers gardés) et pour un exécutable seul ; non pour une vidéo, un pdf, un mp3…
pub fn needed(files: &[Entry]) -> bool {
    match files {
        [] => false,
        [(rel, _, _)] => EXECUTABLES.contains(&extension(rel).as_str()),
        _ => true,
    }
}

/// Nom du zip envoyé : celui du téléchargement, sans caractère interdit dans un nom de fichier.
pub fn file_name(name: &str) -> String {
    let clean: String =
        name.chars().map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).take(200).collect();
    let clean = clean.trim().trim_end_matches('.');
    format!("{}.zip", if clean.is_empty() { "fichiers" } else { clean })
}

/// Zip en cours d'écriture ou envoyé : effacé du disque dès qu'il n'est plus utile (fin de
/// l'envoi, erreur ou arrêt de la tâche).
pub struct TempArchive {
    pub path: PathBuf,
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("archive temporaire non effacée ({}) : {err}", self.path.display());
        }
    }
}

/// Écrit le zip des `files` dans `path` (écrasé s'il existe). `done` : octets lus jusqu'ici, pour
/// la progression. Fonction bloquante : à lancer hors du fil asynchrone.
pub fn write(path: &Path, files: &[Entry], done: &AtomicU64) -> anyhow::Result<u64> {
    let total: u64 = files.iter().map(|(_, _, size)| size).sum();
    if let Some(free) = crate::system::free_space(path.parent().unwrap_or(path)) {
        // un zip peut dépasser un peu la taille des fichiers (en-têtes, données incompressibles)
        if free < total + total / 100 + 16 * 1024 * 1024 {
            bail!("espace disque insuffisant pour préparer l'archive zip");
        }
    }
    let out = File::create(path).with_context(|| format!("archive impossible à créer : {}", path.display()))?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(1024 * 1024, out));
    let mut buffer = vec![0u8; 1024 * 1024];
    for (rel, source, _) in files {
        let method = if COMPRESSED.contains(&extension(rel).as_str()) {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        // large_file : fichiers de plus de 4 Go (Zip64)
        let options = SimpleFileOptions::default().compression_method(method).large_file(true);
        zip.start_file(rel.as_str(), options).with_context(|| format!("archive : « {rel} »"))?;
        let mut input = File::open(source).with_context(|| format!("fichier introuvable : {}", source.display()))?;
        loop {
            let read = input.read(&mut buffer).with_context(|| format!("lecture de « {rel} »"))?;
            if read == 0 {
                break;
            }
            zip.write_all(&buffer[..read]).context("écriture de l'archive (disque plein ?)")?;
            done.fetch_add(read as u64, Ordering::Relaxed);
        }
    }
    let mut out = zip.finish().context("archive impossible à terminer")?;
    out.flush().context("écriture de l'archive (disque plein ?)")?;
    drop(out);
    Ok(std::fs::metadata(path)?.len())
}

/// [`write`] dans une tâche à part ; `on_progress` reçoit régulièrement les octets déjà lus.
pub async fn write_async(path: PathBuf, files: Vec<Entry>, mut on_progress: impl FnMut(u64)) -> anyhow::Result<u64> {
    let done = Arc::new(AtomicU64::new(0));
    let counter = done.clone();
    let mut task = tokio::task::spawn_blocking(move || write(&path, &files, &counter));
    loop {
        tokio::select! {
            result = &mut task => return result.context("archive zip interrompue")?,
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => on_progress(done.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str) -> Entry {
        (rel.into(), PathBuf::from(rel), 1)
    }

    #[test]
    fn zip_only_when_needed() {
        assert!(!needed(&[]));
        assert!(!needed(&[entry("Film.mkv")]));
        assert!(!needed(&[entry("Livre.pdf")]));
        assert!(!needed(&[entry("Album/titre.mp3")]));
        assert!(needed(&[entry("MD5/QuickSFV.EXE")]));
        assert!(needed(&[entry("Saison/E01.mkv"), entry("Saison/E02.mkv")]));
    }

    #[test]
    fn archive_name_is_a_plain_file_name() {
        assert_eq!(file_name("MD5"), "MD5.zip");
        assert_eq!(file_name("a/b:c*?"), "a_b_c__.zip");
        assert_eq!(file_name("  ... "), "fichiers.zip");
    }

    #[test]
    fn archive_keeps_folders_and_contents() {
        let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(root.join("MD5/doc")).unwrap();
        let text = "somme de contrôle ".repeat(1000);
        std::fs::write(root.join("MD5/QuickSFV.EXE"), b"MZ\x90\x00binaire").unwrap();
        std::fs::write(root.join("MD5/doc/lisez-moi.txt"), &text).unwrap();
        let files: Vec<Entry> = ["MD5/QuickSFV.EXE", "MD5/doc/lisez-moi.txt"]
            .iter()
            .map(|rel| {
                let path = root.join(rel);
                let size = std::fs::metadata(&path).unwrap().len();
                (rel.to_string(), path, size)
            })
            .collect();
        let zip_path = root.join("envoi.zip");
        let done = AtomicU64::new(0);
        let size = write(&zip_path, &files, &done).unwrap();
        assert_eq!(done.load(Ordering::Relaxed), files.iter().map(|f| f.2).sum::<u64>());
        assert_eq!(size, std::fs::metadata(&zip_path).unwrap().len());

        let mut archive = zip::ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = archive.file_names().map(String::from).collect();
        names.sort();
        assert_eq!(names, ["MD5/QuickSFV.EXE", "MD5/doc/lisez-moi.txt"]);
        let mut read = String::new();
        archive.by_name("MD5/doc/lisez-moi.txt").unwrap().read_to_string(&mut read).unwrap();
        assert_eq!(read, text);

        // effacé dès que l'envoi n'en a plus besoin
        drop(TempArchive { path: zip_path.clone() });
        assert!(!zip_path.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
