//! Intégration Windows : espace disque, programmes lancés, démarrage avec Windows,
//! association des liens magnet et ouverture de l'Explorateur.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Argument passé au lancement automatique : l'application démarre cachée dans la zone de notification.
pub const HIDDEN_ARG: &str = "--cache";

// ------------------------------------------------------------ espace disque

/// Octets libres pour l'utilisateur sur le disque qui contient `dir` (ou son premier parent existant).
pub fn free_space(dir: &Path) -> Option<u64> {
    let mut target = dir.to_path_buf();
    while !target.exists() {
        target = target.parent()?.to_path_buf();
    }
    imp::free_space(&target)
}

// -------------------------------------------------------------- programmes

pub struct ProcessWatcher {
    system: sysinfo::System,
}

impl ProcessWatcher {
    pub fn new() -> Self {
        Self { system: sysinfo::System::new() }
    }

    /// Noms des programmes lancés, en minuscules (ex. « cs2.exe »).
    pub fn running(&mut self) -> HashSet<String> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        // seulement la liste des processus : pas de mesure CPU/mémoire, pour rester léger pendant les jeux
        self.system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        self.system.processes().values().map(|p| p.name().to_string_lossy().to_lowercase()).collect()
    }
}

pub fn normalize_exe(name: &str) -> String {
    let name = name.trim().to_lowercase();
    if name.ends_with(".exe") { name } else { format!("{name}.exe") }
}

// ------------------------------------------------------------ arrêt du PC

/// Chemin complet d'un outil de Windows : lancé par son seul nom, il serait d'abord cherché
/// dans le dossier de l'application (modifiable par l'utilisateur), avant System32.
pub fn system32(exe: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join(exe)
}

/// Outil en ligne de commande lancé sans fenêtre : l'application n'a pas de console
/// (`windows_subsystem = "windows"`), Windows en ouvrirait donc une à chaque lancement.
pub fn hidden_command(exe: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn run_shutdown(args: &[&str]) -> anyhow::Result<()> {
    let status = hidden_command(system32("shutdown.exe")).args(args).status().context("shutdown.exe introuvable")?;
    anyhow::ensure!(status.success(), "shutdown.exe a échoué ({status})");
    Ok(())
}

/// Programme l'arrêt de Windows dans `delay_secs` secondes (Windows affiche son avertissement).
pub fn schedule_shutdown(delay_secs: u64) -> anyhow::Result<()> {
    run_shutdown(&["/s", "/t", &delay_secs.to_string(), "/c", "Pocket Torrent : téléchargements terminés."])
}

/// Annule un arrêt programmé.
pub fn cancel_shutdown() -> anyhow::Result<()> {
    run_shutdown(&["/a"]).context("aucun arrêt programmé à annuler")
}

// ----------------------------------------------------------- Explorateur

/// Ouvre un dossier, ou l'Explorateur avec le fichier sélectionné.
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(explorer_argument(path, path.is_file()));
    }
    #[cfg(not(windows))]
    command.arg(path);
    command.spawn().context("impossible d'ouvrir l'Explorateur")?;
    Ok(())
}

/// Argument d'explorer.exe, chemin toujours entre guillemets (un nom de fichier Windows ne peut
/// pas en contenir) : Explorer découpe sa ligne de commande aux virgules, et un nom venu d'un
/// torrent (« a,b.exe ») ne doit pas pouvoir être lu comme autre chose qu'un chemin.
#[cfg_attr(not(windows), allow(dead_code))]
fn explorer_argument(path: &Path, select: bool) -> String {
    let quoted = format!("\"{}\"", path.display().to_string().replace('"', ""));
    if select { format!("/select,{quoted}") } else { quoted }
}

/// Un seul accès au presse-papiers à la fois dans tout le programme. Les commandes `async` de
/// Tauri tournent sur n'importe quel thread tokio : deux lectures simultanées (retour sur la
/// fenêtre, copie d'un lien) faisaient libérer par un thread le bloc mémoire que l'autre était
/// en train de lire → corruption du tas (0xc0000374), plantage de l'application en 0.2.1.
/// Reproduit par `clipboard_stress::concurrent_reads_do_not_corrupt_memory`.
static CLIPBOARD: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Copie du texte dans le presse-papiers Windows.
pub fn copy_text(text: &str) -> anyhow::Result<()> {
    let _guard = CLIPBOARD.lock();
    imp::copy_text(text)
}

/// Texte du presse-papiers Windows (au plus `max_chars` caractères), `None` s'il n'y a pas de texte.
pub fn read_clipboard_text(max_chars: usize) -> Option<String> {
    let _guard = CLIPBOARD.lock();
    imp::read_clipboard_text(max_chars)
}

fn current_exe() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("chemin du programme introuvable")
}

pub use imp::{
    link_handler_enabled, on_session_end, open_with_default, prevent_sleep, refresh_registration, set_link_handler,
    set_start_with_windows, start_with_windows_enabled,
};

// ------------------------------------------------- réparation des associations

/// Type de fichier (« ProgID ») des .torrent ouverts par Pocket Torrent.
const TORRENT_PROG_ID: &str = "PocketTorrent.Torrent";
/// Celui des versions « Torrent Cloud » (effacé depuis) : l'extension qui y pointe encore est libre.
const OLD_TORRENT_PROG_ID: &str = "TorrentCloud.Torrent";

/// Associations trouvées au démarrage, quand l'option « liens magnet et fichiers .torrent » est
/// active (commande magnet à nous).
#[cfg_attr(not(windows), allow(dead_code))]
struct RegistrationState {
    /// Commande magnet sur l'exe actuel (pas un ancien dossier d'installation).
    magnet_ok: bool,
    /// Nom, icône et déclaration dans « Applications par défaut ».
    caps_ok: bool,
    /// Commande d'ouverture du type de fichier sur l'exe actuel.
    prog_id_ok: bool,
    /// Programme choisi pour l'extension `.torrent`.
    torrent_ext: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Repairs {
    magnet: bool,
    capabilities: bool,
    prog_id: bool,
    torrent_ext: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Repairs {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
impl RegistrationState {
    /// Ce qu'il faut remettre. L'extension `.torrent` n'est reprise que libre ou laissée par
    /// l'ancien nom : si l'utilisateur a choisi un autre programme, il est respecté.
    fn repairs(&self) -> Repairs {
        let ext_free = match self.torrent_ext.as_deref() {
            None => true,
            Some(prog_id) => prog_id.eq_ignore_ascii_case(OLD_TORRENT_PROG_ID),
        };
        Repairs {
            magnet: !self.magnet_ok,
            capabilities: !self.caps_ok,
            prog_id: !self.prog_id_ok,
            torrent_ext: ext_free,
        }
    }
}

#[cfg(test)]
mod registration_tests {
    use super::{RegistrationState, Repairs};

    fn state(torrent_ext: Option<&str>) -> RegistrationState {
        RegistrationState { magnet_ok: true, caps_ok: true, prog_id_ok: true, torrent_ext: torrent_ext.map(Into::into) }
    }

    #[test]
    fn complete_registration_needs_nothing() {
        assert!(state(Some("PocketTorrent.Torrent")).repairs().is_empty());
    }

    /// Cas trouvé le 26/09 : magnet en place, plus rien pour les .torrent (ancien « Torrent Cloud »).
    #[test]
    fn magnet_alone_gets_the_torrent_files_back() {
        let broken = RegistrationState { caps_ok: false, prog_id_ok: false, ..state(None) };
        assert_eq!(broken.repairs(), Repairs { magnet: false, capabilities: true, prog_id: true, torrent_ext: true });
        assert!(state(Some("TorrentCloud.Torrent")).repairs().torrent_ext, "ancien nom : repris");
    }

    #[test]
    fn another_program_chosen_for_torrent_files_is_respected() {
        assert!(!state(Some("qBittorrent.File.Torrent")).repairs().torrent_ext);
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use anyhow::Context;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    use super::{HIDDEN_ARG, RegistrationState, current_exe};

    /// Empêche (ou autorise de nouveau) la mise en veille pendant un transfert actif.
    /// `ES_CONTINUOUS` fait tenir ce réglage jusqu'au prochain appel : pas besoin de le
    /// répéter en boucle, seulement à chaque changement d'état (voir `Manager::update_sleep_prevention`).
    pub fn prevent_sleep(active: bool) {
        use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
        let flags = if active { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
        // SAFETY : appel Win32 documenté, sans pointeur.
        unsafe {
            SetThreadExecutionState(flags);
        }
    }

    /// Windows s'arrête, redémarre ou ferme la session (arrêt programmé par « éteindre à la fin »
    /// compris) : `on_end` est appelé avant que Windows ne tue le processus. Passe par la fenêtre
    /// principale (même cachée, elle reçoit `WM_ENDSESSION` comme toute fenêtre de premier niveau).
    /// Windows n'attend qu'environ 5 s : `on_end` doit rester en dessous.
    pub fn on_session_end(hwnd: *mut std::ffi::c_void, on_end: Box<dyn Fn() + Send + Sync>) -> anyhow::Result<()> {
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
        use windows_sys::Win32::UI::WindowsAndMessaging::WM_ENDSESSION;

        type Callback = Box<dyn Fn() + Send + Sync>;

        unsafe extern "system" fn subclass(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            _id: usize,
            data: usize,
        ) -> LRESULT {
            // wparam = 0 : l'arrêt a été annulé (par un autre programme ou l'utilisateur)
            if msg == WM_ENDSESSION && wparam != 0 {
                // SAFETY : `data` est le `Box<Callback>` fuité ci-dessous, jamais libéré.
                let on_end = unsafe { &*(data as *const Callback) };
                on_end();
            }
            // SAFETY : transmet le message à la procédure d'origine (celle de Tauri).
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }

        // vit aussi longtemps que la fenêtre, c'est-à-dire tout le programme
        let data = Box::into_raw(Box::new(on_end)) as usize;
        // SAFETY : fenêtre valide, créée par ce thread (appelé depuis `setup`, thread principal).
        let ok = unsafe { SetWindowSubclass(hwnd as HWND, Some(subclass), 1, data) };
        anyhow::ensure!(ok != 0, "SetWindowSubclass a échoué");
        Ok(())
    }

    /// Ouvre un fichier ou une adresse avec le programme associé : appel direct à
    /// `ShellExecuteW`, sans passer par `cmd.exe` (aucune ré-interprétation du chemin).
    pub fn open_with_default(path: &Path) -> anyhow::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;

        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let wide_verb: Vec<u16> = "open\0".encode_utf16().collect();
        // SAFETY : chaînes terminées par un zéro ; SW_SHOWNORMAL = 1 ; pas de fenêtre parente.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                wide_verb.as_ptr(),
                wide_path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        } as isize;
        if result <= 32 {
            anyhow::bail!("impossible d'ouvrir le fichier (code {result})");
        }
        Ok(())
    }

    pub fn free_space(dir: &Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut available = 0u64;
        // SAFETY: chemin terminé par un zéro, pointeurs de sortie valides ou nuls.
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(available)
    }

    pub fn copy_text(text: &str) -> anyhow::Result<()> {
        use windows_sys::Win32::Foundation::GlobalFree;
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

        const CF_UNICODETEXT: u32 = 13;
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: séquence classique du presse-papiers Win32 ; la mémoire appartient au système
        // une fois SetClipboardData réussi, sinon elle est libérée ici.
        unsafe {
            let mut opened = false;
            for _ in 0..10 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if !opened {
                anyhow::bail!("presse-papiers occupé, réessaie");
            }
            let result = (|| {
                EmptyClipboard();
                let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
                if memory.is_null() {
                    anyhow::bail!("mémoire insuffisante pour copier");
                }
                let target = GlobalLock(memory).cast::<u16>();
                if target.is_null() {
                    GlobalFree(memory);
                    anyhow::bail!("mémoire insuffisante pour copier");
                }
                std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                GlobalUnlock(memory);
                if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
                    GlobalFree(memory);
                    anyhow::bail!("copie refusée par Windows");
                }
                Ok(())
            })();
            CloseClipboard();
            result
        }
    }

    pub fn read_clipboard_text(max_chars: usize) -> Option<String> {
        use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
        use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

        const CF_UNICODETEXT: u32 = 13;
        // SAFETY: presse-papiers ouvert puis refermé ; lecture bornée par la taille du bloc mémoire
        // (et par `max_chars`), jamais au-delà même sans zéro final.
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }
            let text = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT);
                if handle.is_null() {
                    return None;
                }
                let data = GlobalLock(handle).cast::<u16>();
                if data.is_null() {
                    return None;
                }
                let units = (GlobalSize(handle) / 2).min(max_chars);
                let slice = std::slice::from_raw_parts(data, units);
                let len = slice.iter().position(|&c| c == 0).unwrap_or(units);
                let text = String::from_utf16_lossy(&slice[..len]);
                GlobalUnlock(handle);
                Some(text)
            })();
            CloseClipboard();
            text
        }
    }

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "Pocket Torrent";

    const MAGNET_KEY: &str = r"Software\Classes\magnet";
    const MAGNET_COMMAND_KEY: &str = r"Software\Classes\magnet\shell\open\command";
    const MAGNET_ICON_KEY: &str = r"Software\Classes\magnet\DefaultIcon";

    const TORRENT_EXT_KEY: &str = r"Software\Classes\.torrent";
    const TORRENT_OPENWITH_KEY: &str = r"Software\Classes\.torrent\OpenWithProgids";
    /// Identifiant de type de fichier (« ProgID ») pour les .torrent : Windows a besoin d'une
    /// clé séparée de l'extension elle-même pour y accrocher l'icône et la commande d'ouverture.
    const PROG_ID: &str = super::TORRENT_PROG_ID;
    const PROG_ID_KEY: &str = r"Software\Classes\PocketTorrent.Torrent";
    const PROG_ID_COMMAND_KEY: &str = r"Software\Classes\PocketTorrent.Torrent\shell\open\command";
    const PROG_ID_ICON_KEY: &str = r"Software\Classes\PocketTorrent.Torrent\DefaultIcon";

    /// « Capacités » façon Windows (nom, description, icône) : sans ça, Pocket Torrent apparaît
    /// sans nom ni icône dans les listes « Ouvrir avec » et « Applications par défaut ».
    const APP_KEY: &str = r"Software\PocketTorrent";
    const CAPABILITIES_KEY: &str = r"Software\PocketTorrent\Capabilities";
    const CAPABILITIES_FILE_ASSOC_KEY: &str = r"Software\PocketTorrent\Capabilities\FileAssociations";
    const CAPABILITIES_URL_ASSOC_KEY: &str = r"Software\PocketTorrent\Capabilities\URLAssociations";
    const REGISTERED_APPS_KEY: &str = r"Software\RegisteredApplications";
    const REGISTERED_APPS_VALUE: &str = "Pocket Torrent";
    const APP_NAME: &str = "Pocket Torrent";

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    fn our_command(exe: &Path, arg: &str) -> String {
        format!("\"{}\" {arg}", exe.display())
    }

    fn our_icon(exe: &Path) -> String {
        format!("{},0", exe.display())
    }

    /// Sans ça, l'Explorateur et les navigateurs gardent l'ancienne association en cache
    /// jusqu'à la prochaine ouverture de session.
    fn notify_associations_changed() {
        use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        // SAFETY: pas de pointeur passé (SHCNF_IDLIST sans item) ; appel Win32 documenté sans effet de bord dangereux.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    }

    pub fn start_with_windows_enabled() -> bool {
        let Ok(exe) = current_exe() else { return false };
        hkcu()
            .open_subkey_with_flags(RUN_KEY, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(RUN_VALUE))
            .is_ok_and(|value| value == our_command(&exe, HIDDEN_ARG))
    }

    pub fn set_start_with_windows(enabled: bool) -> anyhow::Result<()> {
        let (key, _) = hkcu().create_subkey(RUN_KEY).context("clé de démarrage inaccessible")?;
        if enabled {
            key.set_value(RUN_VALUE, &our_command(&current_exe()?, HIDDEN_ARG))
                .context("impossible d'activer le démarrage avec Windows")?;
        } else if key.get_value::<String, _>(RUN_VALUE).is_ok() {
            key.delete_value(RUN_VALUE).context("impossible de désactiver le démarrage avec Windows")?;
        }
        Ok(())
    }

    fn magnet_command() -> Option<String> {
        hkcu()
            .open_subkey_with_flags(MAGNET_COMMAND_KEY, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(""))
            .ok()
            .filter(|value| !value.is_empty())
    }

    fn torrent_ext_progid() -> Option<String> {
        hkcu()
            .open_subkey_with_flags(TORRENT_EXT_KEY, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(""))
            .ok()
            .filter(|value| !value.is_empty())
    }

    /// Vrai si Pocket Torrent est déjà le programme choisi pour les liens magnet.
    pub fn link_handler_enabled() -> bool {
        let Ok(exe) = current_exe() else { return false };
        magnet_command().is_some_and(|command| command == our_command(&exe, "\"%1\""))
    }

    /// Enregistre Pocket Torrent comme application « capable » de Windows : nom, description et
    /// icône visibles dans « Ouvrir avec » et « Applications par défaut », comme le fait
    /// l'installeur de qBittorrent. Sans cette clé, Windows ne connaît que le chemin de l'exe.
    fn register_capabilities(exe: &Path) -> anyhow::Result<()> {
        let (caps, _) = hkcu().create_subkey(CAPABILITIES_KEY).context("clé des capacités inaccessible")?;
        caps.set_value("ApplicationName", &APP_NAME)?;
        caps.set_value("ApplicationDescription", &"Torrents et liens directs")?;
        caps.set_value("ApplicationIcon", &our_icon(exe))?;

        let (file_assoc, _) = hkcu().create_subkey(CAPABILITIES_FILE_ASSOC_KEY)?;
        file_assoc.set_value(".torrent", &PROG_ID)?;

        let (url_assoc, _) = hkcu().create_subkey(CAPABILITIES_URL_ASSOC_KEY)?;
        url_assoc.set_value("magnet", &"magnet")?;

        let (registered, _) =
            hkcu().create_subkey(REGISTERED_APPS_KEY).context("clé des applications enregistrées inaccessible")?;
        registered.set_value(REGISTERED_APPS_VALUE, &CAPABILITIES_KEY)?;
        Ok(())
    }

    fn unregister_capabilities() {
        if let Ok(registered) = hkcu().open_subkey_with_flags(REGISTERED_APPS_KEY, KEY_SET_VALUE) {
            let _ = registered.delete_value(REGISTERED_APPS_VALUE);
        }
        let _ = hkcu().delete_subkey_all(APP_KEY);
    }

    /// Après une mise à jour qui change le dossier d'installation : le démarrage avec Windows et
    /// les liens magnet / .torrent, s'ils étaient activés, sont remis sur l'exe actuel. Rien n'est
    /// activé qui ne l'était pas. Pas en version de développement : l'exe de développement
    /// prendrait la place de l'application installée.
    pub fn refresh_registration() -> anyhow::Result<()> {
        if cfg!(debug_assertions) {
            return Ok(());
        }
        let exe = current_exe()?;
        let exe_name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        // commande posée par une version précédente : même exe, dossier éventuellement différent
        let is_ours = |command: &str| command.to_lowercase().contains(&format!("\\{exe_name}\""));

        if let Ok(run) = hkcu().open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_SET_VALUE) {
            let value = |name: &str| run.get_value::<String, _>(name).ok().filter(|c| is_ours(c));
            let enabled = value(RUN_VALUE).is_some();
            let wanted = our_command(&exe, HIDDEN_ARG);
            if enabled && value(RUN_VALUE).as_deref() != Some(wanted.as_str()) {
                run.set_value(RUN_VALUE, &wanted)?;
                tracing::info!("démarrage avec Windows remis sur {}", exe.display());
            }
        }

        // option « Ouvrir les liens magnet et fichiers .torrent » active : la commande magnet est à
        // nous. Tout le reste est alors vérifié, pas seulement le magnet : une ancienne version
        // (« Torrent Cloud ») a pu laisser le magnet en place mais plus rien pour les .torrent.
        let ours = our_command(&exe, "\"%1\"");
        let Some(magnet) = magnet_command().filter(|c| is_ours(c)) else { return Ok(()) };
        let state = RegistrationState {
            magnet_ok: magnet == ours,
            caps_ok: hkcu().open_subkey_with_flags(CAPABILITIES_KEY, KEY_READ).is_ok_and(|caps| {
                caps.get_value::<String, _>("ApplicationName").ok().as_deref() == Some(APP_NAME)
                    && caps.get_value::<String, _>("ApplicationIcon").ok() == Some(our_icon(&exe))
            }) && hkcu()
                .open_subkey_with_flags(REGISTERED_APPS_KEY, KEY_READ)
                .and_then(|k| k.get_value::<String, _>(REGISTERED_APPS_VALUE))
                .is_ok_and(|v| v == CAPABILITIES_KEY),
            prog_id_ok: hkcu()
                .open_subkey_with_flags(PROG_ID_COMMAND_KEY, KEY_READ)
                .and_then(|k| k.get_value::<String, _>(""))
                .is_ok_and(|c| c == ours),
            torrent_ext: torrent_ext_progid(),
        };
        let plan = state.repairs();
        if plan.is_empty() {
            return Ok(());
        }
        if plan.magnet {
            let (command, _) = hkcu().create_subkey(MAGNET_COMMAND_KEY)?;
            command.set_value("", &ours)?;
            let (icon, _) = hkcu().create_subkey(MAGNET_ICON_KEY)?;
            icon.set_value("", &our_icon(&exe))?;
        }
        if plan.capabilities {
            register_capabilities(&exe)?;
        }
        if plan.prog_id {
            write_prog_id(&exe)?;
        }
        if plan.torrent_ext {
            let (ext, _) = hkcu().create_subkey(TORRENT_EXT_KEY)?;
            ext.set_value("", &PROG_ID)?;
        }
        // toujours proposé dans « Ouvrir avec », même si un autre programme est choisi par défaut
        let (open_with, _) = hkcu().create_subkey(TORRENT_OPENWITH_KEY)?;
        open_with.set_value(PROG_ID, &"")?;
        notify_associations_changed();
        tracing::info!(?plan, "liens magnet et fichiers .torrent remis sur {}", exe.display());
        Ok(())
    }

    fn write_prog_id(exe: &Path) -> anyhow::Result<()> {
        let (prog, _) = hkcu().create_subkey(PROG_ID_KEY).context("clé du type de fichier torrent inaccessible")?;
        prog.set_value("", &"Fichier Torrent")?;
        let (icon, _) = hkcu().create_subkey(PROG_ID_ICON_KEY)?;
        icon.set_value("", &our_icon(exe))?;
        let (command, _) = hkcu().create_subkey(PROG_ID_COMMAND_KEY)?;
        command.set_value("", &our_command(exe, "\"%1\""))?;
        Ok(())
    }

    /// Associe (ou non) les liens magnet et les fichiers .torrent à Pocket Torrent pour l'utilisateur.
    /// `previous` : (ancienne commande magnet, ancien programme .torrent), à conserver pour
    /// restaurer l'un ou l'autre si on désactive. Renvoie la même paire mise à jour.
    pub fn set_link_handler(
        enabled: bool,
        previous: (Option<String>, Option<String>),
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        let exe = current_exe()?;
        let (previous_magnet, previous_torrent) = previous;
        let ours_command = our_command(&exe, "\"%1\"");

        let existing_magnet = magnet_command();
        let magnet_is_ours = existing_magnet.as_deref() == Some(ours_command.as_str());
        let existing_torrent = torrent_ext_progid();
        let torrent_is_ours = existing_torrent.as_deref() == Some(PROG_ID);

        if enabled {
            register_capabilities(&exe)?;
            write_prog_id(&exe)?;

            let next_magnet = if magnet_is_ours {
                previous_magnet
            } else {
                let (magnet, _) = hkcu().create_subkey(MAGNET_KEY).context("clé magnet inaccessible")?;
                magnet.set_value("", &"URL:Magnet Link")?;
                magnet.set_value("URL Protocol", &"")?;
                let (icon, _) = hkcu().create_subkey(MAGNET_ICON_KEY)?;
                icon.set_value("", &our_icon(&exe))?;
                let (command, _) = hkcu().create_subkey(MAGNET_COMMAND_KEY)?;
                command.set_value("", &ours_command).context("impossible d'associer les liens magnet")?;
                existing_magnet
            };

            let next_torrent = if torrent_is_ours {
                previous_torrent
            } else {
                let (ext, _) = hkcu().create_subkey(TORRENT_EXT_KEY).context("clé .torrent inaccessible")?;
                ext.set_value("", &PROG_ID).context("impossible d'associer les fichiers .torrent")?;
                // en plus du programme par défaut, Pocket Torrent reste proposé dans « Ouvrir avec »
                let (open_with, _) = hkcu().create_subkey(TORRENT_OPENWITH_KEY)?;
                open_with.set_value(PROG_ID, &"")?;
                existing_torrent
            };

            notify_associations_changed();
            return Ok((next_magnet, next_torrent));
        }

        // désactivation : on retire nos associations, en remettant ce qu'il y avait avant s'il y en avait un
        if magnet_is_ours {
            match &previous_magnet {
                Some(old) => {
                    let command = hkcu().open_subkey_with_flags(MAGNET_COMMAND_KEY, KEY_SET_VALUE)?;
                    command.set_value("", old).context("impossible de remettre l'ancien programme magnet")?;
                }
                None => {
                    hkcu().delete_subkey_all(MAGNET_KEY).context("impossible de retirer l'association magnet")?;
                }
            }
        }
        if torrent_is_ours {
            match &previous_torrent {
                Some(old) => {
                    let ext = hkcu().open_subkey_with_flags(TORRENT_EXT_KEY, KEY_SET_VALUE)?;
                    ext.set_value("", old).context("impossible de remettre l'ancien programme .torrent")?;
                }
                None => {
                    hkcu()
                        .delete_subkey_all(TORRENT_EXT_KEY)
                        .context("impossible de retirer l'association .torrent")?;
                }
            }
            if let Ok(open_with) = hkcu().open_subkey_with_flags(TORRENT_OPENWITH_KEY, KEY_SET_VALUE) {
                let _ = open_with.delete_value(PROG_ID);
            }
        }
        let _ = hkcu().delete_subkey_all(PROG_ID_KEY);
        unregister_capabilities();
        notify_associations_changed();
        Ok((None, None))
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn free_space(_dir: &Path) -> Option<u64> {
        None
    }
    pub fn prevent_sleep(_active: bool) {}
    pub fn refresh_registration() -> anyhow::Result<()> {
        Ok(())
    }
    pub fn on_session_end(_hwnd: *mut std::ffi::c_void, _on_end: Box<dyn Fn() + Send + Sync>) -> anyhow::Result<()> {
        Ok(())
    }
    pub fn open_with_default(_path: &Path) -> anyhow::Result<()> {
        anyhow::bail!("disponible seulement sous Windows")
    }
    pub fn copy_text(_text: &str) -> anyhow::Result<()> {
        anyhow::bail!("disponible seulement sous Windows")
    }
    pub fn read_clipboard_text(_max_chars: usize) -> Option<String> {
        None
    }
    pub fn start_with_windows_enabled() -> bool {
        false
    }
    pub fn set_start_with_windows(_enabled: bool) -> anyhow::Result<()> {
        anyhow::bail!("disponible seulement sous Windows")
    }
    pub fn link_handler_enabled() -> bool {
        false
    }
    pub fn set_link_handler(
        _enabled: bool,
        previous: (Option<String>, Option<String>),
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        Ok(previous)
    }
}

#[cfg(test)]
mod clipboard_stress {
    /// Lecture seule du presse-papiers, répétée : jamais d'écriture (le contenu de l'utilisateur
    /// n'est pas touché). Sert à détecter une corruption mémoire dans `read_clipboard_text`.
    #[test]
    #[ignore = "lit le vrai presse-papiers : à lancer à la main"]
    fn repeated_reads_do_not_corrupt_memory() {
        for _ in 0..2000 {
            let text = super::read_clipboard_text(8192);
            std::hint::black_box(text);
            let big: Vec<String> = (0..50).map(|i| "x".repeat(i * 10)).collect();
            std::hint::black_box(big);
        }
    }

    /// Même chose depuis plusieurs threads à la fois, comme le font les commandes `async`
    /// de Tauri (un thread tokio différent à chaque retour sur la fenêtre).
    #[test]
    #[ignore = "lit le vrai presse-papiers : à lancer à la main"]
    fn concurrent_reads_do_not_corrupt_memory() {
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..3000 {
                        std::hint::black_box(super::read_clipboard_text(8192));
                        let big: Vec<String> = (0..50).map(|i| "x".repeat(i * 10)).collect();
                        std::hint::black_box(big);
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod explorer_tests {
    use std::path::Path;

    #[test]
    fn explorer_always_gets_one_quoted_path() {
        assert_eq!(
            super::explorer_argument(Path::new(r"C:\Téléchargements\a,b ,c.exe"), true),
            r#"/select,"C:\Téléchargements\a,b ,c.exe""#
        );
        assert_eq!(super::explorer_argument(Path::new(r"C:\dl\x"), false), r#""C:\dl\x""#);
        // jamais de guillemet qui fermerait l'argument (impossible dans un vrai nom Windows)
        assert_eq!(super::explorer_argument(Path::new("a\" /x"), false), r#""a /x""#);
    }
}
