//! Envoi (Pocket Torrent) : des pairs qui n'ont besoin de rien (sources complètes connectées à
//! nous) ne doivent pas occuper les places d'envoi du choker au détriment de ceux qui veulent
//! nos pièces. Un pair entrant était compté « intéressé » d'office, et « plus intéressé »
//! n'était jamais pris en compte.

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use crate::{
    AddTorrent, AddTorrentOptions, ListenerMode, Session, SessionOptions, create_torrent,
    limits::LimitsConfig,
    listen::ListenerOptions,
    spawn_utils::BlockingSpawner,
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
};

const BASE: u16 = 15480;
const SIZE: usize = 512 * 1024;
const UPLOAD_BPS: u32 = 128 * 1024;

async fn session(
    port: u16,
    out: &std::path::Path,
    slots: Option<usize>,
    upload_bps: Option<u32>,
) -> std::sync::Arc<Session> {
    Session::new_with_opts(
        out.to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: (Ipv4Addr::LOCALHOST, port).into(),
                ..Default::default()
            }),
            upload_slots: slots,
            ratelimits: LimitsConfig {
                upload_bps: upload_bps.and_then(NonZeroU32::new),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// Temps pour que X télécharge 512 Kio depuis S (une place d'envoi, 128 Kio/s), quand
/// `idle_seeds` sources complètes sont connectées à S.
async fn download_time(base: u16, idle_seeds: u16) -> Duration {
    let source = create_default_random_dir_with_torrents(1, SIZE, Some("rqbit_interest"));
    let torrent = create_torrent(
        source.path(),
        crate::CreateTorrentOptions {
            piece_length: Some(16 * 1024),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let folder = source.path().to_str().unwrap().to_owned();
    let addr = |port: u16| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let add = |session: std::sync::Arc<Session>,
               peers: Option<Vec<SocketAddr>>,
               out: String,
               overwrite: bool| {
        let bytes = bytes.clone();
        async move {
            session
                .add_torrent(
                    AddTorrent::TorrentFileBytes(bytes),
                    Some(AddTorrentOptions {
                        overwrite,
                        initial_peers: peers,
                        output_folder: Some(out),
                        ..Default::default()
                    }),
                )
                .await
                .unwrap()
                .into_handle()
                .unwrap()
        }
    };

    let s = session(base, source.path(), Some(1), Some(UPLOAD_BPS)).await;
    add(s.clone(), None, folder.clone(), true).await;
    // sources complètes qui se connectent à S (entrantes pour lui) et n'ont besoin de rien
    let mut keep = Vec::new();
    for i in 1..=idle_seeds {
        let seed = session(base + i, source.path(), None, None).await;
        add(seed.clone(), Some(vec![addr(base)]), folder.clone(), true).await;
        keep.push(seed);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let out = tempfile::TempDir::with_prefix("rqbit_interest_client").unwrap();
    let x = session(base + 10, out.path(), None, None).await;
    let started = Instant::now();
    let handle = add(
        x.clone(),
        Some(vec![addr(base)]),
        out.path().to_str().unwrap().to_owned(),
        false,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(120), handle.wait_until_completed())
        .await
        .expect("jamais fini")
        .unwrap();
    started.elapsed()
}

/// Mesure : `--ignored --nocapture interest_measure`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn interest_measure() {
    setup_test_logging();
    println!("sans source inutile : {:?}", download_time(BASE, 0).await);
    println!(
        "avec 3 sources inutiles : {:?}",
        download_time(BASE + 20, 3).await
    );
}

/// Octets que Y a reçus de L en `window`, pendant que L télécharge lui-même depuis
/// `seeds` sources qui se connectent à lui (entrantes pour L). L a 2 places d'envoi ; Y ne
/// peut obtenir des pièces que de L.
async fn uploaded_while_downloading(base: u16, seeds: u16, window: Duration) -> u64 {
    let size = 2 * 1024 * 1024;
    let source = create_default_random_dir_with_torrents(1, size, Some("rqbit_interest_dl"));
    let torrent = create_torrent(
        source.path(),
        crate::CreateTorrentOptions {
            piece_length: Some(16 * 1024),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let addr = |port: u16| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let add = |session: std::sync::Arc<Session>,
               peers: Option<Vec<SocketAddr>>,
               out: String,
               overwrite: bool| {
        let bytes = bytes.clone();
        async move {
            session
                .add_torrent(
                    AddTorrent::TorrentFileBytes(bytes),
                    Some(AddTorrentOptions {
                        overwrite,
                        initial_peers: peers,
                        output_folder: Some(out),
                        ..Default::default()
                    }),
                )
                .await
                .unwrap()
                .into_handle()
                .unwrap()
        }
    };
    // L : client sous test, 2 places d'envoi
    let l_out = tempfile::TempDir::with_prefix("rqbit_interest_l").unwrap();
    let l = session(base, l_out.path(), Some(2), None).await;
    add(
        l.clone(),
        None,
        l_out.path().to_str().unwrap().to_owned(),
        false,
    )
    .await;
    // sources : se connectent à L, bridées pour que L reste longtemps en téléchargement
    let mut keep = Vec::new();
    for i in 1..=seeds {
        let seed = session(base + i, source.path(), None, Some(48 * 1024)).await;
        add(
            seed.clone(),
            Some(vec![addr(base)]),
            source.path().to_str().unwrap().to_owned(),
            true,
        )
        .await;
        keep.push(seed);
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    // Y : ne connaît que L
    let y_out = tempfile::TempDir::with_prefix("rqbit_interest_y").unwrap();
    let y = session(base + 10, y_out.path(), None, None).await;
    let y_handle = add(
        y.clone(),
        Some(vec![addr(base)]),
        y_out.path().to_str().unwrap().to_owned(),
        false,
    )
    .await;
    tokio::time::sleep(window).await;
    y_handle.stats().progress_bytes
}

/// Mesure : `--ignored --nocapture interest_while_downloading_measure`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn interest_while_downloading_measure() {
    setup_test_logging();
    for round in 0..10u16 {
        let got =
            uploaded_while_downloading(BASE + 40 + round * 20, 3, Duration::from_secs(6)).await;
        println!("essai {round} : Y a reçu {} Kio de L", got / 1024);
    }
}
