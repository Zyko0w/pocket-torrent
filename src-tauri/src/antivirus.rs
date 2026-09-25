//! Analyse antivirus optionnelle à la fin d'un téléchargement (§10.4 du plan d'upgrade), via
//! l'outil en ligne de commande de Microsoft Defender déjà présent sur Windows
//! (`MpCmdRun.exe`) — pas de nouvelle dépendance. Rapport seulement (`-DisableRemediation`) :
//! ne touche jamais au fichier, c'est à l'utilisateur de décider quoi en faire.
//!
//! Comportement vérifié manuellement sur une machine Windows 11 avec Microsoft Defender :
//! code de sortie 0 et « found no threats » si rien n'est trouvé ; code de sortie 2 et une
//! section « Threat : <nom> » par menace sinon. Le fichier scanné n'est pas touché.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum ScanResult {
    Clean,
    /// Nom de chaque menace détectée (ex. « Virus:DOS/EICAR_Test_File »).
    ThreatFound(Vec<String>),
    /// Outil introuvable ou erreur d'exécution : ne bloque jamais le téléchargement pour ça.
    Unavailable,
}

fn mpcmdrun_path() -> Option<PathBuf> {
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let stable = Path::new(&program_files).join("Windows Defender").join("MpCmdRun.exe");
        if stable.is_file() {
            return Some(stable);
        }
    }
    // sur les installations récentes, l'outil vit sous un dossier versionné
    let program_data = std::env::var_os("ProgramData")?;
    let platform_dir = Path::new(&program_data).join("Microsoft").join("Windows Defender").join("Platform");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&platform_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    let exe = versions.pop()?.join("MpCmdRun.exe");
    exe.is_file().then_some(exe)
}

pub fn available() -> bool {
    mpcmdrun_path().is_some()
}

/// Bouton « Tester » des réglages : analyse un petit fichier sans danger (dossier temporaire
/// dédié, effacé ensuite) et renvoie la durée. Bloquant, comme `scan`.
pub fn self_test() -> anyhow::Result<std::time::Duration> {
    let dir = std::env::temp_dir().join(format!("pocket-torrent-defender-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("test.txt");
    std::fs::write(&file, "Pocket Torrent : fichier de test sans danger")?;
    let started = std::time::Instant::now();
    let result = scan(&file);
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        ScanResult::Clean => Ok(started.elapsed()),
        ScanResult::ThreatFound(threats) => anyhow::bail!("résultat inattendu : {}", threats.join(", ")),
        ScanResult::Unavailable => anyhow::bail!(
            "Microsoft Defender ne répond pas : il est peut-être désactivé ou remplacé par un autre antivirus."
        ),
    }
}

/// Lance une analyse sur `path` (fichier ou dossier). Bloquant (appel de processus) : à lancer
/// hors du fil principal (`spawn_blocking`).
pub fn scan(path: &Path) -> ScanResult {
    // une analyse à la fois : 50 téléchargements terminés ensemble (flux RSS) ne lancent pas
    // 50 analyses en parallèle
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(exe) = mpcmdrun_path() else { return ScanResult::Unavailable };
    let Ok(output) = crate::system::hidden_command(exe)
        .args(["-Scan", "-ScanType", "3", "-DisableRemediation", "-File"])
        .arg(path)
        .output()
    else {
        return ScanResult::Unavailable;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let threats: Vec<String> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("Threat "))
        .filter_map(|line| line.split_once(':').map(|(_, value)| value.trim().to_owned()))
        .collect();
    if !threats.is_empty() {
        return ScanResult::ThreatFound(threats);
    }
    if output.status.success() { ScanResult::Clean } else { ScanResult::Unavailable }
}

#[cfg(test)]
mod tests {
    /// Sur un PC avec Microsoft Defender actif : `cargo test --lib -- --ignored defender`.
    /// (Ignoré par défaut : la machine de la CI n'a pas forcément Defender en service.)
    #[test]
    #[ignore]
    fn defender_answers_on_this_pc() {
        assert!(super::available(), "MpCmdRun.exe introuvable");
        let took = super::self_test().unwrap();
        assert!(took.as_secs() < 60, "{took:?}");
    }
}
