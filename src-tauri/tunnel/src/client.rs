//! Côté application : état du service, installation (une autorisation administrateur) et commandes.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use windows_service::service::ServiceAccess;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::protocol::{PIPE_NAME, PROTOCOL_VERSION, Request, Response, SERVICE_NAME};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Running,
    /// Installé mais d'une autre version : à réinstaller.
    Outdated,
}

pub fn service_state() -> ServiceState {
    let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) else {
        return ServiceState::NotInstalled;
    };
    let Ok(service) = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) else {
        return ServiceState::NotInstalled;
    };
    match service.query_status() {
        Ok(status) if status.current_state == windows_service::service::ServiceState::Running => {
            match request(&Request::Version) {
                Ok(Response { version: Some(v), .. }) if v == PROTOCOL_VERSION => ServiceState::Running,
                Ok(_) => ServiceState::Outdated,
                // démarré mais pas encore prêt à répondre
                Err(_) => ServiceState::Stopped,
            }
        }
        _ => ServiceState::Stopped,
    }
}

/// Processus du service du tunnel d'après le gestionnaire de services, s'il tourne.
fn service_pid() -> Option<u32> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).ok()?;
    let status = service.query_status().ok()?;
    (status.current_state == windows_service::service::ServiceState::Running).then_some(status.process_id).flatten()
}

/// Envoie une commande au service et attend sa réponse.
pub fn request(request: &Request) -> anyhow::Result<Response> {
    let pid = service_pid().ok_or_else(|| anyhow!("le service du tunnel ne tourne pas"))?;
    request_on(PIPE_NAME, pid, request)
}

/// Envoie `request` sur le canal `name`, à condition qu'il soit tenu par le processus `expected_pid`.
///
/// N'importe quel programme peut créer un canal nommé à ce nom quand le service est arrêté (ou
/// avant lui) : sans ces vérifications, il recevrait la configuration WireGuard, clé privée
/// comprise, et pourrait agir avec les droits de l'utilisateur. D'où :
/// - le serveur du canal doit être le processus du service (identifiant donné par Windows) ;
/// - le serveur ne peut qu'identifier le client, pas emprunter son identité (`SECURITY_IDENTIFICATION`).
fn request_on(name: &str, expected_pid: u32, request: &Request) -> anyhow::Result<Response> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
        .open(name)
        .map_err(|err| match err.raw_os_error() {
            Some(2) => anyhow!("le service du tunnel ne tourne pas"),
            Some(5) => anyhow!("accès refusé au service du tunnel (installé par un autre compte ?)"),
            _ => anyhow!("service du tunnel injoignable : {err}"),
        })?;
    let mut server_pid = 0u32;
    // SAFETY: poignée de canal valide pendant l'appel, sortie écrite dans une variable locale.
    let known = unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server_pid) } != 0;
    if !known || server_pid != expected_pid {
        bail!("canal du tunnel tenu par un autre programme que le service : rien n'a été envoyé");
    }
    let mut writer = &pipe;
    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    let mut reader = BufReader::new(&pipe);
    let mut answer = String::new();
    reader.read_line(&mut answer).context("pas de réponse du service du tunnel")?;
    let response: Response = serde_json::from_str(&answer).context("réponse illisible du service du tunnel")?;
    Ok(response)
}

/// Comme `request`, mais transforme une réponse d'échec en erreur.
pub fn call(req: &Request) -> anyhow::Result<Response> {
    let response = request(req)?;
    if !response.ok {
        bail!(response.error.unwrap_or_else(|| "le service du tunnel a refusé la commande".into()));
    }
    Ok(response)
}

/// Installe (ou met à jour) le service. Windows affiche une demande d'autorisation administrateur.
pub fn install(tunnel_exe: &Path) -> anyhow::Result<()> {
    let sid = current_user_sid()?;
    run_elevated(tunnel_exe, &format!("install --user {sid}"))
}

pub fn uninstall(tunnel_exe: &Path) -> anyhow::Result<()> {
    run_elevated(tunnel_exe, "uninstall")
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_elevated(exe: &Path, arguments: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    if !exe.exists() {
        bail!("programme du tunnel introuvable : {}", exe.display());
    }
    let result_file = std::env::temp_dir().join(format!("pocket-torrent-tunnel-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&result_file);
    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params = wide(&format!("{arguments} --result \"{}\"", result_file.display()));

    // SAFETY: structure initialisée à zéro puis remplie ; chaînes terminées par un zéro, vivantes pendant l'appel.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(1223) {
                bail!("autorisation administrateur refusée");
            }
            bail!("lancement impossible : {err}");
        }
        if info.hProcess.is_null() {
            bail!("lancement impossible");
        }
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            bail!("attente du programme du tunnel impossible");
        }
        let message = std::fs::read_to_string(&result_file).unwrap_or_default();
        let _ = std::fs::remove_file(&result_file);
        if code != 0 {
            bail!(if message.trim().is_empty() { format!("échec (code {code})") } else { message.trim().to_owned() });
        }
    }
    Ok(())
}

/// Identifiant (SID) du compte Windows qui lance l'application.
pub fn current_user_sid() -> anyhow::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: appels Win32 classiques ; le tampon est dimensionné par un premier appel.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!("jeton du processus illisible : {}", std::io::Error::last_os_error());
        }
        let mut size = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u8; size as usize];
        let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size);
        CloseHandle(token);
        if ok == 0 {
            bail!("compte Windows illisible : {}", std::io::Error::last_os_error());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            bail!("SID illisible : {}", std::io::Error::last_os_error());
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(sid)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::mpsc;

    use super::request_on;
    use crate::protocol::{PROTOCOL_VERSION, Request};

    /// Faux service : canal nommé créé par ce processus (comme le ferait un programme
    /// malveillant), qui renvoie tout ce qu'il reçoit par `received`.
    fn fake_server(name: &str) -> mpsc::Receiver<Vec<u8>> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: nom terminé par un zéro ; sécurité par défaut.
        let handle = unsafe {
            CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, std::ptr::null())
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let handle = handle as usize;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let handle = handle as windows_sys::Win32::Foundation::HANDLE;
            // SAFETY: poignée créée ci-dessus, reprise par le `File`.
            unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            let pipe = unsafe { std::fs::File::from_raw_handle(handle) };
            let mut line = String::new();
            let _ = BufReader::new(&pipe).take(65_536).read_line(&mut line);
            let _ = (&pipe).write_all(format!("{{\"ok\":true,\"version\":{PROTOCOL_VERSION}}}\n").as_bytes());
            let _ = tx.send(line.into_bytes());
        });
        rx
    }

    /// Canal propre à chaque test (bac à sable) : jamais celui du vrai service.
    fn pipe_name() -> String {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\pocket-torrent-sandbox-{}-{n}", std::process::id())
    }

    #[test]
    fn fake_pipe_server_never_receives_the_wireguard_key() {
        let name = pipe_name();
        let received = fake_server(&name);
        let config = "[Interface]\nPrivateKey = SECRET\n".to_owned();
        // le vrai service aurait un autre processus : le client refuse avant d'écrire
        let err = request_on(&name, std::process::id().wrapping_add(1), &Request::Up { config }).unwrap_err();
        assert!(format!("{err}").contains("autre programme"), "{err}");
        let sent = received.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(sent.is_empty(), "le faux service a reçu : {}", String::from_utf8_lossy(&sent));
    }

    #[test]
    fn genuine_server_process_is_accepted() {
        let name = pipe_name();
        let received = fake_server(&name);
        let response = request_on(&name, std::process::id(), &Request::Version).unwrap();
        assert_eq!(response.version, Some(PROTOCOL_VERSION));
        assert!(!received.recv_timeout(std::time::Duration::from_secs(5)).unwrap().is_empty());
    }
}
