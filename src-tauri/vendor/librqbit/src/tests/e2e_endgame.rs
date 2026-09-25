//! Fin de téléchargement (Pocket Torrent) : une source rapide et une source très lente. Si la
//! source lente garde une des dernières pièces, le téléchargement ne doit pas l'attendre (le
//! « bloqué à 99 % ») : la source rapide doit la reprendre.

use std::net::Ipv4Addr;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use crate::{
    AddTorrent, AddTorrentOptions, ListenerMode, Session, SessionOptions, create_torrent,
    limits::LimitsConfig,
    listen::ListenerOptions,
    spawn_utils::BlockingSpawner,
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
};

const PIECE: u32 = 64 * 1024;
/// Source lente : 16 Kio/s, soit 4 s par pièce.
const SLOW_BPS: u32 = 16 * 1024;

async fn session(
    port: u16,
    out: &std::path::Path,
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

/// Temps pour télécharger 2 Mio depuis une source rapide et une source à 16 Kio/s.
async fn download_time(base_port: u16) -> Duration {
    let source = create_default_random_dir_with_torrents(1, 2 * 1024 * 1024, Some("rqbit_endgame"));
    let torrent = create_torrent(
        source.path(),
        crate::CreateTorrentOptions {
            piece_length: Some(PIECE),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let seed = |session: std::sync::Arc<Session>| {
        let bytes = bytes.clone();
        let folder = source.path().to_str().unwrap().to_owned();
        async move {
            session
                .add_torrent(
                    AddTorrent::TorrentFileBytes(bytes),
                    Some(AddTorrentOptions {
                        overwrite: true,
                        output_folder: Some(folder),
                        ..Default::default()
                    }),
                )
                .await
                .unwrap();
            session
        }
    };
    let _fast = seed(session(base_port, source.path(), None).await).await;
    let _slow = seed(session(base_port + 1, source.path(), Some(SLOW_BPS)).await).await;

    let out = tempfile::TempDir::with_prefix("rqbit_endgame_client").unwrap();
    let client = session(base_port + 2, out.path(), None).await;
    let started = Instant::now();
    let handle = client
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.clone()),
            Some(AddTorrentOptions {
                initial_peers: Some(vec![
                    (Ipv4Addr::LOCALHOST, base_port).into(),
                    (Ipv4Addr::LOCALHOST, base_port + 1).into(),
                ]),
                output_folder: Some(out.path().to_str().unwrap().to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(120), handle.wait_until_completed())
        .await
        .expect("téléchargement jamais fini")
        .unwrap();
    started.elapsed()
}

/// Mesure : `cargo test … --lib -- --ignored --nocapture endgame_measure`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn endgame_measure() {
    setup_test_logging();
    let base: u16 = std::env::var("ENDGAME_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(15400);
    for round in 0..5u16 {
        let elapsed = download_time(base + round * 3).await;
        println!("essai {round} : {elapsed:?}");
    }
}

/// Régression : avant la fin de partie, le client attendait toujours la source lente (5 s pour
/// 2 Mio, mesuré) ; depuis, environ 0,3 s. Marge large pour les machines lentes.
#[tokio::test(flavor = "multi_thread")]
async fn slow_source_does_not_hold_up_the_end() {
    setup_test_logging();
    let elapsed = download_time(15430).await;
    assert!(
        elapsed < Duration::from_millis(2500),
        "fin de téléchargement trop lente : {elapsed:?}"
    );
}
