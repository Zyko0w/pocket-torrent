//! Lance un programme externe à l'ajout ou à la fin d'un téléchargement (§5 du plan d'upgrade).
//! Variables substituées dans le modèle : `%N` (nom), `%F` (fichier ou dossier), `%I` (empreinte).

use std::path::Path;

/// Retire les caractères qui ont un sens spécial pour `cmd.exe` (`& | ^ % " < >`), pour
/// qu'un nom de fichier ou de torrent forgé par un tiers ne puisse pas injecter une
/// commande supplémentaire dans le modèle configuré par l'utilisateur.
fn shell_safe(text: &str) -> String {
    text.chars().filter(|c| !"&|^%\"<>\r\n".contains(*c) && !c.is_control()).collect()
}

/// Lance `template` (si non vide) sans bloquer ni attendre le résultat.
pub fn run(template: &str, name: &str, path: &Path, info_hash: &str) {
    let template = template.trim();
    if template.is_empty() {
        return;
    }
    let command = template
        .replace("%N", &shell_safe(name))
        .replace("%F", &shell_safe(&path.to_string_lossy()))
        .replace("%I", info_hash);
    tracing::info!("programme externe : {command}");
    if let Err(err) = spawn_cmd(&command) {
        tracing::warn!("programme externe impossible à lancer : {err:#}");
    }
}

/// `cmd /D /S /C "<commande>"`, transmis tel quel : `.arg()` échapperait les guillemets du
/// modèle à la manière de Rust (`\"`), que cmd.exe ne comprend pas. `/S` : cmd retire juste
/// les guillemets extérieurs ; `/D` : ignore les commandes AutoRun du registre.
#[cfg(windows)]
fn spawn_cmd(command: &str) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(crate::system::system32("cmd.exe")).raw_arg(format!("/D /S /C \"{command}\"")).spawn()
}

#[cfg(not(windows))]
fn spawn_cmd(command: &str) -> std::io::Result<std::process::Child> {
    std::process::Command::new("sh").arg("-c").arg(command).spawn()
}

#[cfg(test)]
mod tests {
    use super::shell_safe;

    #[test]
    fn injection_characters_are_removed() {
        assert_eq!(shell_safe("film & calc.exe | x ^ %PATH% \"q\" <a> \r\n"), "film  calc.exe  x  PATH q a ");
    }
}
