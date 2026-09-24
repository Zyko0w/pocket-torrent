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

/// Envoie une commande au service et attend sa réponse.
pub fn request(request: &Request) -> anyhow::Result<Response> {
    let pipe =
        std::fs::OpenOptions::new().read(true).write(true).open(PIPE_NAME).map_err(|err| match err.raw_os_error() {
            Some(2) => anyhow!("le service du tunnel ne tourne pas"),
            Some(5) => anyhow!("accès refusé au service du tunnel (installé par un autre compte ?)"),
            _ => anyhow!("service du tunnel injoignable : {err}"),
        })?;
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
