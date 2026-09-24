//! `pocket-torrent-tunnel.exe`
//!
//! - `service --user <SID>` : lancé par Windows, gère le tunnel ;
//! - `install --user <SID>` / `uninstall` : lancés avec l'autorisation administrateur par l'application.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod net;
#[cfg(windows)]
mod nt;
#[cfg(windows)]
mod service;

#[cfg(not(windows))]
fn main() {
    eprintln!("disponible seulement sous Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("");
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();

    let result = match command {
        "service" => service::run_dispatcher(),
        "install" => {
            option("--user").ok_or_else(|| anyhow::anyhow!("--user manquant")).and_then(|sid| service::install(&sid))
        }
        "uninstall" => service::uninstall(),
        _ => Err(anyhow::anyhow!("usage : pocket-torrent-tunnel.exe service|install|uninstall")),
    };

    if let Err(err) = result {
        let message = format!("{err:#}");
        service::log(&format!("erreur ({command}) : {message}"));
        if let Some(path) = option("--result") {
            service::write_result(&path, &message);
        }
        std::process::exit(1);
    }
}
