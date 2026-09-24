//! Détection de fichiers dangereux dans un torrent : extensions exécutables et pièges
//! classiques (double extension façon « film.mkv.exe », caractère RTLO trompeur).
//! Voir §10.4 du plan d'upgrade : un torrent peut se faire passer pour une vidéo alors
//! qu'il contient un exécutable.

/// Caractère d'inversion de texte : peut faire afficher « exe.mkv » comme « mkv.exe ».
const RTLO: char = '\u{202e}';

/// Extensions de médias courantes, utilisées pour repérer une « fausse vidéo »
/// (ex. « film.mkv.exe » : le nom se termine par une extension dangereuse cachée
/// derrière une extension de média qui rassure au premier coup d'œil).
const MEDIA_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "m4v", "mp3", "flac", "wav", "pdf", "jpg", "jpeg", "png", "gif", "srt", "sub",
    "epub", "docx", "iso", "zip", "rar",
];

/// Raison de se méfier de ce fichier, ou `None` s'il ne présente rien d'inhabituel.
/// `path` : chemin relatif dans le torrent (séparateurs `/`). `excluded` : extensions
/// signalées, avec le point (ex. « .exe »), voir `Settings::security`.
pub fn warn_reason(path: &str, excluded: &[String]) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.contains(RTLO) {
        return Some("nom de fichier trompeur (caractère d'inversion de texte)".into());
    }
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| format!(".{e}"))?;
    if !excluded.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
        return None;
    }
    let parts: Vec<&str> = lower.split('.').collect();
    if parts.len() >= 3 && MEDIA_EXTENSIONS.contains(&parts[parts.len() - 2]) {
        return Some(format!("fausse vidéo : {ext} caché derrière .{}", parts[parts.len() - 2]));
    }
    Some(format!("extension {ext} potentiellement dangereuse"))
}

#[cfg(test)]
mod tests {
    use super::warn_reason;

    fn excluded() -> Vec<String> {
        vec![".exe".into(), ".scr".into(), ".lnk".into()]
    }

    #[test]
    fn safe_file_is_not_flagged() {
        assert_eq!(warn_reason("Film/movie.mkv", &excluded()), None);
    }

    #[test]
    fn dangerous_extension_is_flagged() {
        assert!(warn_reason("setup.exe", &excluded()).is_some());
    }

    #[test]
    fn fake_video_is_flagged_distinctly() {
        let reason = warn_reason("film.mkv.exe", &excluded()).unwrap();
        assert!(reason.contains("fausse vidéo"), "{reason}");
    }

    #[test]
    fn rtlo_is_flagged() {
        assert!(warn_reason("film\u{202e}exe.mkv", &excluded()).is_some());
    }

    #[test]
    fn extension_not_excluded_is_ignored() {
        assert_eq!(warn_reason("readme.txt", &excluded()), None);
    }
}
