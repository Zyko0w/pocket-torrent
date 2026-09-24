//! Instantané de performance (§2.2 du plan d'upgrade) : à relever avant et après une
//! modification, pour prouver un gain ou repérer une régression sur ce PC. Ne remplace pas
//! les mesures d'un vrai essaim (débit à 10 Gbit/s, 10 000 torrents…), qui se font à la main
//! avec des clients de référence — voir la méthode décrite en §2.2.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::engine::Engine;

static STARTED_AT: OnceLock<Instant> = OnceLock::new();

/// À appeler une fois, au lancement de l'application.
pub fn mark_start() {
    let _ = STARTED_AT.set(Instant::now());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub uptime_secs: u64,
    pub cpu_percent: f32,
    pub memory_mb: u64,
    pub torrents: usize,
    pub peers_live: u32,
    pub down_speed: u64,
    pub up_speed: u64,
    pub listen_port: Option<u16>,
    pub disk_free_mb: Option<u64>,
}

/// Deux mesures espacées de 200 ms : `sysinfo` a besoin d'un écart pour calculer un
/// pourcentage de CPU, une seule lecture instantanée renverrait toujours 0.
pub async fn snapshot(engine: &Engine, download_dir: &Path) -> Snapshot {
    let (cpu_percent, memory_mb) = match sysinfo::get_current_pid() {
        Ok(pid) => {
            let mut system = System::new();
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::everything());
            tokio::time::sleep(Duration::from_millis(200)).await;
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::everything());
            system.process(pid).map(|p| (p.cpu_usage(), p.memory() / (1024 * 1024))).unwrap_or_default()
        }
        Err(_) => (0.0, 0),
    };

    let torrents = engine.torrents();
    Snapshot {
        uptime_secs: STARTED_AT.get().map_or(0, |s| s.elapsed().as_secs()),
        cpu_percent,
        memory_mb,
        torrents: torrents.len(),
        peers_live: torrents.iter().map(|t| t.peers_live).sum(),
        down_speed: torrents.iter().map(|t| t.down_speed).sum(),
        up_speed: torrents.iter().map(|t| t.up_speed).sum(),
        listen_port: engine.listen_port(),
        disk_free_mb: crate::system::free_space(download_dir).map(|b| b / (1024 * 1024)),
    }
}
