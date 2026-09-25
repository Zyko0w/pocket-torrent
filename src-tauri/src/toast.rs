//! Notifications Windows (« toasts »).
//!
//! Une appli de bureau classique ne peut afficher de notification que si son identifiant
//! (AppUserModelID, ici l'identifiant Tauri `app.pocket.torrent`) est déclaré à Windows. Sinon
//! Windows ignore la notification sans aucune erreur. `register` le déclare dans le registre de
//! l'utilisateur (nom et icône affichés), à chaque démarrage ; le désinstallateur l'efface
//! (`windows/hooks.nsh`). `show` renvoie une erreur lisible quand Windows ne l'affichera pas
//! (notifications coupées pour tout le PC ou pour l'appli), au lieu de l'avaler.

use std::path::Path;

/// Clé de la déclaration, sous `HKCU\Software\Classes`.
#[cfg(windows)]
fn key_path(app_id: &str) -> String {
    format!(r"Software\Classes\AppUserModelId\{app_id}")
}

/// Identifiant utilisé : en développement (exe non installé), celui de PowerShell, toujours
/// déclaré, pour ne rien écrire dans le registre.
#[cfg(windows)]
fn effective_id(app_id: &str) -> &str {
    if cfg!(debug_assertions) { tauri_winrt_notification::Toast::POWERSHELL_APP_ID } else { app_id }
}

/// Déclare l'identifiant à Windows : nom « Pocket Torrent » et icône (copiée dans `data_dir`,
/// Windows attend un fichier image).
#[cfg(windows)]
pub fn register(app_id: &str, data_dir: &Path) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    const ICON: &[u8] = include_bytes!("../icons/128x128.png");
    let icon = data_dir.join("notification.png");
    if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(data_dir)?;
        crate::store::write_atomic(&icon, ICON)?;
    }
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(key_path(app_id))?;
    key.set_value("DisplayName", &"Pocket Torrent")?;
    key.set_value("IconUri", &icon.to_string_lossy().into_owned())?;
    Ok(())
}

#[cfg(not(windows))]
pub fn register(_app_id: &str, _data_dir: &Path) -> anyhow::Result<()> {
    Ok(())
}

/// Texte venu d'un torrent (nom choisi par un inconnu) : le XML est échappé par la
/// bibliothèque, mais un caractère de contrôle rendrait la notification invalide (perdue) et un
/// caractère d'inversion ferait lire « film\u{202e}4pm.exe » comme « filmexe.mp4 ».
fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() || librqbit_core::torrent_metainfo::is_bidi_control(c) { ' ' } else { c })
        .take(300)
        .collect();
    if text.chars().count() > 300 {
        out.push('…');
    }
    out
}

/// Affiche une notification. Bloquant (appels COM) : à lancer hors du fil principal.
#[cfg(windows)]
pub fn show(app_id: &str, title: &str, body: &str) -> anyhow::Result<()> {
    use windows::UI::Notifications::{NotificationSetting, ToastNotificationManager};
    let id = effective_id(app_id);
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&windows::core::HSTRING::from(id))?;
    match notifier.Setting()? {
        NotificationSetting::Enabled => {}
        NotificationSetting::DisabledForUser => anyhow::bail!(
            "les notifications sont coupées dans Windows (Paramètres › Système › Notifications) : aucune appli ne peut en afficher"
        ),
        NotificationSetting::DisabledForApplication => anyhow::bail!(
            "les notifications de Pocket Torrent sont coupées dans Windows (Paramètres › Système › Notifications › Pocket Torrent)"
        ),
        NotificationSetting::DisabledByGroupPolicy => {
            anyhow::bail!("les notifications sont bloquées par une stratégie de l'organisation sur ce PC")
        }
        other => anyhow::bail!("Windows refuse les notifications (réglage {})", other.0),
    }
    tauri_winrt_notification::Toast::new(id)
        .title(&clean(title))
        .text1(&clean(body))
        .show()
        .map_err(|err| anyhow::anyhow!("notification refusée par Windows : {err}"))
}

#[cfg(not(windows))]
pub fn show(_app_id: &str, _title: &str, _body: &str) -> anyhow::Result<()> {
    anyhow::bail!("notifications disponibles seulement sous Windows")
}

#[cfg(all(test, windows))]
mod tests {
    /// Sans toucher au registre : clé lue par Windows, et en développement l'identifiant de
    /// PowerShell (déjà déclaré) au lieu de celui de l'appli.
    #[test]
    fn declaration_key_and_development_id() {
        assert_eq!(super::key_path("app.pocket.torrent"), r"Software\Classes\AppUserModelId\app.pocket.torrent");
        assert_eq!(super::effective_id("app.pocket.torrent"), tauri_winrt_notification::Toast::POWERSHELL_APP_ID);
    }

    #[test]
    fn torrent_names_cannot_break_or_disguise_a_notification() {
        assert_eq!(super::clean("film\u{202e}4pm.exe"), "film 4pm.exe");
        assert_eq!(super::clean("a\u{1}b\nc"), "a b c");
        assert_eq!(super::clean("Film <b>&</b>"), "Film <b>&</b>", "le XML est échappé par la bibliothèque");
        let long = super::clean(&"x".repeat(1000));
        assert_eq!(long.chars().count(), 301);
    }
}

#[cfg(all(test, windows))]
mod manual_tests {
    /// Sur ce PC : `cargo test --lib -- --ignored toast_reports`. Affiche une vraie notification
    /// (identifiant de PowerShell) si Windows les autorise, sinon doit dire pourquoi.
    #[test]
    #[ignore]
    fn toast_reports_what_windows_does() {
        match super::show("app.pocket.torrent", "Pocket Torrent", "Test de notification") {
            Ok(()) => println!("RÉSULTAT : notification affichée"),
            Err(err) => println!("RÉSULTAT : {err:#}"),
        }
    }
}
