//! Analyse antivirus optionnelle à la fin d'un téléchargement (§10.4 du plan d'upgrade), via
//! l'outil en ligne de commande de Microsoft Defender déjà présent sur Windows
//! (`MpCmdRun.exe`) — pas de nouvelle dépendance. Rapport seulement (`-DisableRemediation`) :
//! ne touche jamais au fichier, c'est à l'utilisateur de décider quoi en faire.
//!
//! Comportement vérifié manuellement sur une machine Windows 11 avec Microsoft Defender :
//! code de sortie 0 et « found no threats » si rien n'est trouvé ; code de sortie 2 et une
//! section « Threat : <nom> » par menace sinon. Le fichier scanné n'est pas touché.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Lance une analyse sur `path` (fichier ou dossier). Bloquant (appel de processus) : à lancer
/// hors du fil principal (`spawn_blocking`).
pub fn scan(path: &Path) -> ScanResult {
    let Some(exe) = mpcmdrun_path() else { return ScanResult::Unavailable };
    let Ok(output) =
        Command::new(exe).args(["-Scan", "-ScanType", "3", "-DisableRemediation", "-File"]).arg(path).output()
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
