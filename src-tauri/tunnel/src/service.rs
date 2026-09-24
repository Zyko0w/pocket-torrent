//! Service Windows du tunnel : installation, canal nommé et cycle de vie de la carte WireGuard.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use sha2::{Digest, Sha256};
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
    ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

use pocket_torrent_tunnel::config::{self, Summary};
use pocket_torrent_tunnel::protocol::{
    ADAPTER_NAME, PIPE_NAME, PROTOCOL_VERSION, Request, Response, SERVICE_DISPLAY_NAME, SERVICE_NAME, TunnelStatus,
};

use crate::{net, nt};

/// Empreinte SHA-256 de wireguard.dll (WireGuard-NT 1.1, amd64, signée par WireGuard LLC).
const WIREGUARD_DLL_SHA256: &str = "b1b85e072c45d81358be29d94c599dc76652f912be8c0f0a41e2d5d89a6461d3";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// ------------------------------------------------------------------ journal

/// Journal dans le dossier d'installation (Program Files, modifiable seulement par les administrateurs) :
/// un dossier de ProgramData pourrait être créé à l'avance par n'importe quel compte et redirigé ailleurs.
pub fn log(message: &str) {
    let Ok(dir) = install_dir() else { return };
    if !dir.is_dir() {
        return;
    }
    let path = dir.join("tunnel.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("tunnel.log.old"));
    }
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[{seconds}] {message}");
    }
}

/// Message d'erreur pour l'application (fichier temporaire qu'elle a désigné).
pub fn write_result(path: &str, message: &str) {
    let path = Path::new(path);
    // seulement un fichier « pocket-torrent-tunnel-*.txt » du dossier temporaire : ce processus a les droits administrateur
    let name_ok = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("pocket-torrent-tunnel-") && n.ends_with(".txt"));
    let dir_ok = path.parent().is_some_and(|p| p == std::env::temp_dir());
    if name_ok && dir_ok {
        // fichier neuf uniquement (l'application l'efface avant de lancer ce programme) : un
        // lien placé là d'avance par un autre programme ne ferait pas écrire ailleurs avec les
        // droits administrateur
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
            let _ = file.write_all(message.as_bytes());
        }
    }
}

// ------------------------------------------------------------------ dossiers

enum KnownFolder {
    ProgramFiles,
}

/// Dossier système lu auprès de Windows (pas dans les variables d'environnement, modifiables par l'utilisateur).
fn known_folder(folder: KnownFolder) -> anyhow::Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

    let id = match folder {
        KnownFolder::ProgramFiles => &FOLDERID_ProgramFiles,
    };
    let mut path: *mut u16 = std::ptr::null_mut();
    // SAFETY: la chaîne renvoyée est libérée par CoTaskMemFree.
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut path);
        if hr < 0 || path.is_null() {
            bail!("dossier système introuvable (0x{hr:08x})");
        }
        let len = (0..).take_while(|&i| *path.add(i) != 0).count();
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path.cast());
        Ok(PathBuf::from(text))
    }
}

fn install_dir() -> anyhow::Result<PathBuf> {
    Ok(known_folder(KnownFolder::ProgramFiles)?.join("Pocket Torrent").join("Tunnel"))
}

/// Dossier du service installé par les versions nommées « Torrent Cloud » (jusqu'à la 0.1.1) :
/// retiré à la réinstallation et à la désinstallation.
fn legacy_install_dir() -> anyhow::Result<PathBuf> {
    Ok(known_folder(KnownFolder::ProgramFiles)?.join("Torrent Cloud").join("Tunnel"))
}

fn valid_sid(sid: &str) -> bool {
    sid.starts_with("S-1-") && sid.len() < 200 && sid[2..].chars().all(|c| c.is_ascii_digit() || c == '-')
}

// ------------------------------------------------------------- installation

fn stop_and_delete(manager: &ServiceManager) -> anyhow::Result<()> {
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let Ok(service) = manager.open_service(SERVICE_NAME, access) else {
        return Ok(());
    };
    if service.query_status()?.current_state != ServiceState::Stopped {
        let _ = service.stop();
        for _ in 0..50 {
            if service.query_status()?.current_state == ServiceState::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    service.delete().context("suppression de l'ancien service impossible")?;
    drop(service);
    // le processus du service peut garder son exécutable ouvert un court instant
    std::thread::sleep(Duration::from_millis(800));
    Ok(())
}

fn copy_with_retry(from: &Path, to: &Path) -> anyhow::Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(err) => last = Some(err),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(anyhow!("copie de {} impossible : {}", from.display(), last.map(|e| e.to_string()).unwrap_or_default()))
}

pub fn install(user_sid: &str) -> anyhow::Result<()> {
    if !valid_sid(user_sid) {
        bail!("identifiant de compte invalide");
    }
    let source_exe = std::env::current_exe()?;
    let source_dll = source_exe.with_file_name("wireguard.dll");
    let dll = std::fs::read(&source_dll)
        .with_context(|| format!("wireguard.dll introuvable à côté de {}", source_exe.display()))?;
    let digest = format!("{:x}", Sha256::digest(&dll));
    if digest != WIREGUARD_DLL_SHA256 {
        bail!("wireguard.dll ne correspond pas au pilote officiel attendu : installation annulée");
    }

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("gestionnaire de services inaccessible (droits administrateur ?)")?;
    stop_and_delete(&manager)?;
    // ancien dossier (versions « Torrent Cloud ») : le service vient d'être arrêté et supprimé
    if let Ok(legacy) = legacy_install_dir()
        && legacy.exists()
    {
        let _ = std::fs::remove_dir_all(&legacy);
        if let Some(parent) = legacy.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    let dir = install_dir()?;
    std::fs::create_dir_all(&dir)?;
    let exe = dir.join("pocket-torrent-tunnel.exe");
    copy_with_retry(&source_exe, &exe)?;
    std::fs::write(dir.join("wireguard.dll"), &dll)?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service"), OsString::from("--user"), OsString::from(user_sid)],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .context("création du service impossible")?;
    let _ = service.set_description("Tunnel WireGuard réservé aux téléchargements de Pocket Torrent.");
    service.start::<&str>(&[]).context("démarrage du service impossible")?;
    log("service installé et démarré");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("gestionnaire de services inaccessible (droits administrateur ?)")?;
    stop_and_delete(&manager)?;
    for dir in [install_dir(), legacy_install_dir()].into_iter().flatten() {
        for _ in 0..20 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if let Some(parent) = dir.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ service

define_windows_service!(ffi_service_main, service_main);

pub fn run_dispatcher() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).context("lancement hors du gestionnaire de services")
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(err) = run_service() {
        log(&format!("arrêt sur erreur : {err:#}"));
    }
}

fn run_service() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sid = args
        .iter()
        .position(|a| a == "--user")
        .and_then(|i| args.get(i + 1))
        .filter(|sid| valid_sid(sid))
        .cloned()
        .context("compte autorisé manquant")?;

    let stopping = Arc::new(AtomicBool::new(false));
    let handler_stopping = stopping.clone();
    let status_handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            handler_stopping.store(true, Ordering::SeqCst);
            // débloque l'attente d'un client sur le canal
            let _ = std::fs::OpenOptions::new().read(true).write(true).open(PIPE_NAME);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    let status = |state: ServiceState| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_service_status(status(ServiceState::Running))?;
    log("service démarré");

    let tunnel = Arc::new(Mutex::new(TunnelManager::default()));
    let result = serve(&sid, &stopping, &tunnel);
    tunnel.lock().unwrap_or_else(|e| e.into_inner()).down();
    status_handle.set_service_status(status(ServiceState::Stopped))?;
    log("service arrêté");
    result
}

/// Instances simultanées du canal : un programme qui ne répond pas n'en bloque qu'une.
const PIPE_INSTANCES: u32 = 4;

/// Canal nommé, accessible seulement à SYSTEM, aux administrateurs et au compte installateur.
/// Chaque client est servi dans son propre fil ; une instance reste toujours à l'écoute.
fn serve(sid: &str, stopping: &AtomicBool, tunnel: &Arc<Mutex<TunnelManager>>) -> anyhow::Result<()> {
    // compte installateur : lecture et écriture, sans le droit de créer sa propre instance du canal
    // (0x4, FILE_CREATE_PIPE_INSTANCE) : aucun programme ne peut se faire passer pour le service
    let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;{sid})"));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: SDDL terminé par un zéro ; descripteur libéré par LocalFree en fin de fonction.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        bail!("droits du canal invalides : {}", std::io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = wide(PIPE_NAME);
    let mut first = true;
    let result = loop {
        if stopping.load(Ordering::SeqCst) {
            break Ok(());
        }
        let flags = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: nom et attributs valides pendant l'appel.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                flags,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_INSTANCES,
                65536,
                65536,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let err = std::io::Error::last_os_error();
            if !first && err.raw_os_error() == Some(231) {
                // ERROR_PIPE_BUSY : toutes les instances sont occupées
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            break Err(anyhow!("création du canal impossible : {err}"));
        }
        first = false;

        // SAFETY: poignée de canal valide, attente synchrone.
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
            || std::io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32);
        // SAFETY: la poignée appartient désormais à ce `File` (fermée à sa destruction).
        let pipe = unsafe { std::fs::File::from_raw_handle(handle) };
        if stopping.load(Ordering::SeqCst) {
            break Ok(());
        }
        if !connected {
            continue;
        }
        let tunnel = tunnel.clone();
        std::thread::spawn(move || {
            if let Err(err) = handle_client(&pipe, &tunnel) {
                log(&format!("client : {err:#}"));
            }
            use std::os::windows::io::AsRawHandle;
            // SAFETY: poignée de canal valide, fermée ensuite par `pipe`.
            unsafe { DisconnectNamedPipe(pipe.as_raw_handle()) };
        });
    };
    // SAFETY: descripteur alloué par ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe { LocalFree(descriptor.cast()) };
    result
}

fn handle_client(pipe: &std::fs::File, tunnel: &Mutex<TunnelManager>) -> anyhow::Result<()> {
    let mut reader = BufReader::new(pipe);
    let mut line = String::new();
    // une requête tient largement dans 64 Ko (un .conf fait moins de 2 Ko)
    let read = reader.by_ref().take(65_536).read_line(&mut line)?;
    if read == 0 {
        return Ok(());
    }
    // la requête est lue en entier avant de prendre le verrou : un client lent ne bloque personne
    let mut tunnel = tunnel.lock().unwrap_or_else(|e| e.into_inner());
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(Request::Version) => Response { ok: true, version: Some(PROTOCOL_VERSION), ..Default::default() },
        Ok(Request::Up { config }) => match tunnel.up(&config) {
            Ok(()) => Response { ok: true, status: Some(tunnel.status()), ..Default::default() },
            Err(err) => {
                log(&format!("montée du tunnel impossible : {err:#}"));
                tunnel.down();
                Response::error(format!("{err:#}"))
            }
        },
        Ok(Request::Down) => {
            tunnel.down();
            Response::ok()
        }
        Ok(Request::Status) => Response { ok: true, status: Some(tunnel.status()), ..Default::default() },
        Err(_) => Response::error("commande inconnue"),
    };
    drop(tunnel);
    let mut writer = pipe;
    let mut answer = serde_json::to_string(&response)?;
    answer.push('\n');
    writer.write_all(answer.as_bytes())?;
    writer.flush()?;
    Ok(())
}

// ------------------------------------------------------------------ tunnel

#[derive(Default)]
struct TunnelManager {
    driver: Option<Arc<nt::Driver>>,
    active: Option<ActiveTunnel>,
}

struct ActiveTunnel {
    adapter: nt::Adapter,
    summary: Summary,
    /// Empreinte du .conf utilisé : une demande identique ne remonte pas le tunnel.
    config_digest: [u8; 32],
}

impl TunnelManager {
    fn driver(&mut self) -> anyhow::Result<Arc<nt::Driver>> {
        if let Some(driver) = &self.driver {
            return Ok(driver.clone());
        }
        let dll = std::env::current_exe()?.with_file_name("wireguard.dll");
        let driver = Arc::new(nt::Driver::load(&dll)?);
        self.driver = Some(driver.clone());
        Ok(driver)
    }

    fn up(&mut self, text: &str) -> anyhow::Result<()> {
        let config = config::parse(text)?;
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if self.active.as_ref().is_some_and(|a| a.config_digest == digest) {
            return Ok(()); // déjà monté avec cette configuration
        }
        self.down();

        let endpoints = config
            .peers
            .iter()
            .map(|peer| resolve(peer.endpoint.as_deref().unwrap_or_default()))
            .collect::<anyhow::Result<Vec<SocketAddr>>>()?;

        let adapter = nt::Adapter::create(self.driver()?, ADAPTER_NAME)?;
        adapter.configure(&config, &endpoints)?;
        adapter.set_up(true)?;
        let luid = adapter.luid();

        net::set_interface(luid, AF_INET, config.mtu)?;
        net::set_interface(luid, AF_INET6, config.mtu)?;
        for (ip, prefix) in &config.addresses {
            net::add_address(luid, *ip, *prefix)?;
        }
        // routes par défaut uniquement : des routes plus précises détourneraient le trafic normal du PC
        if config.addresses.iter().any(|(ip, _)| ip.is_ipv4()) {
            net::add_default_route(luid, false)?;
        }
        if config.addresses.iter().any(|(ip, _)| ip.is_ipv6()) {
            net::add_default_route(luid, true)?;
        }
        log(&format!("tunnel monté vers {:?}", config.summary().endpoint));
        self.active = Some(ActiveTunnel { adapter, summary: config.summary(), config_digest: digest });
        Ok(())
    }

    fn down(&mut self) {
        if let Some(active) = self.active.take() {
            let _ = active.adapter.set_up(false);
            drop(active);
            log("tunnel arrêté");
        }
    }

    fn status(&self) -> TunnelStatus {
        let Some(active) = &self.active else {
            return TunnelStatus::default();
        };
        let (rx_bytes, tx_bytes, handshake) = active.adapter.statistics().unwrap_or((0, 0, 0));
        // 100 ns depuis 1601 → secondes Unix
        let handshake_age_secs = (handshake > 0).then(|| {
            let unix = (handshake / 10_000_000).saturating_sub(11_644_473_600);
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            now.saturating_sub(unix)
        });
        TunnelStatus {
            up: true,
            addresses: active.summary.addresses.clone(),
            endpoint: active.summary.endpoint.clone(),
            handshake_age_secs,
            rx_bytes,
            tx_bytes,
        }
    }
}

/// « hôte:port » → adresse, IPv4 de préférence.
fn resolve(endpoint: &str) -> anyhow::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> =
        endpoint.to_socket_addrs().with_context(|| format!("serveur « {endpoint} » introuvable"))?.collect();
    addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or(addresses.first())
        .copied()
        .with_context(|| format!("serveur « {endpoint} » sans adresse"))
}
