//! Choking sous contrainte (Torrent Cloud) : un seul partageur avec UNE seule place d'envoi,
//! face à plusieurs clients. Chacun ne peut finir qu'avec la place tournante du partageur et
//! la réciprocité entre clients : si le choker bloquait l'envoi, le test expirerait.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::time::timeout;

use crate::{
    AddTorrent, AddTorrentOptions, ListenerMode, Session, SessionOptions, create_torrent,
    listen::ListenerOptions,
    spawn_utils::BlockingSpawner,
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
};

const LEECHERS: u16 = 4;
const BASE_PORT: u16 = 15300;

async fn session(port: u16, upload_slots: usize, out: &std::path::Path) -> std::sync::Arc<Session> {
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
            upload_slots: Some(upload_slots),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_choking_single_slot_seeder() {
    setup_test_logging();
    let source = create_default_random_dir_with_torrents(4, 8_000_000, Some("rqbit_choking"));
    let torrent = create_torrent(
        source.path(),
        crate::CreateTorrentOptions {
            piece_length: Some(32 * 1024),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let all_peers: Vec<_> = (0..=LEECHERS)
        .map(|i| (Ipv4Addr::LOCALHOST, BASE_PORT + i).into())
        .collect();

    // partageur : une seule place
    let seeder = session(BASE_PORT, 1, source.path()).await;
    let seeder_handle = seeder
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.clone()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(source.path().to_str().unwrap().to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();

    // surveillance du partageur : jamais plus d'un pair intéressé débloqué à la fois, et au
    // moins un moment où plusieurs attendaient (sinon le test ne prouverait rien)
    let stop = Arc::new(AtomicBool::new(false));
    let watch = {
        let handle = seeder_handle.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let (mut max_unchoked, mut max_interested) = (0usize, 0usize);
            while !stop.load(Ordering::Relaxed) {
                if let Some(live) = handle.live() {
                    let (interested, unchoked) = live.choke_counts();
                    max_unchoked = max_unchoked.max(unchoked);
                    max_interested = max_interested.max(interested);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            (max_unchoked, max_interested)
        })
    };

    // clients : deux places chacun, tous connectés entre eux
    let mut leechers = Vec::new();
    for i in 1..=LEECHERS {
        let out = tempfile::TempDir::with_prefix("rqbit_choking_client").unwrap();
        let session = session(BASE_PORT + i, 2, out.path()).await;
        let handle = session
            .add_torrent(
                AddTorrent::TorrentFileBytes(bytes.clone()),
                Some(AddTorrentOptions {
                    initial_peers: Some(
                        all_peers
                            .iter()
                            .copied()
                            .filter(|p: &std::net::SocketAddr| p.port() != BASE_PORT + i)
                            .collect(),
                    ),
                    output_folder: Some(out.path().to_str().unwrap().to_owned()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_handle()
            .unwrap();
        leechers.push((out, session, handle));
    }

    let waits = leechers
        .iter()
        .map(|(_, _, handle)| handle.wait_until_completed());
    timeout(
        Duration::from_secs(120),
        futures::future::try_join_all(waits),
    )
    .await
    .expect("les clients n'ont pas fini : le choker bloque l'envoi")
    .unwrap();
    stop.store(true, Ordering::Relaxed);
    let (max_unchoked, max_interested) = watch.await.unwrap();
    assert!(
        max_interested > 1,
        "pas de concurrence chez le partageur ({max_interested} intéressé au plus) : test inutile"
    );
    assert!(
        max_unchoked <= 1,
        "le partageur a débloqué {max_unchoked} pairs pour 1 place"
    );
    tracing::info!(
        max_interested,
        max_unchoked,
        "choking vérifié chez le partageur"
    );

    // tout le monde a reçu les mêmes octets que la source
    for (out, _, handle) in &leechers {
        assert!(handle.stats().finished);
        for entry in std::fs::read_dir(source.path()).unwrap().flatten() {
            if entry.path().is_file() {
                let copy = out.path().join(entry.file_name());
                assert_eq!(
                    std::fs::read(entry.path()).unwrap(),
                    std::fs::read(copy).unwrap()
                );
            }
        }
    }
}

/// BEP 21 : un partageur partiel (un seul des fichiers choisi et téléchargé) se connecte à un
/// partageur complet. Les deux ont fini : la connexion est coupée grâce à `upload_only`.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_upload_only_partial_seed() {
    use std::sync::atomic::Ordering;

    use crate::torrent_state::live::UPLOAD_ONLY_DISCONNECTS;

    setup_test_logging();
    // fichiers multiples de la taille de pièce : aucune pièce à cheval sur deux fichiers
    let piece: u32 = 32 * 1024;
    let source =
        create_default_random_dir_with_torrents(4, piece as usize * 64, Some("rqbit_upload_only"));
    let torrent = create_torrent(
        source.path(),
        crate::CreateTorrentOptions {
            piece_length: Some(piece),
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    let bytes = torrent.as_bytes().unwrap();
    let before = UPLOAD_ONLY_DISCONNECTS.load(Ordering::Relaxed);

    let full = session(15320, 4, source.path()).await;
    full.add_torrent(
        AddTorrent::TorrentFileBytes(bytes.clone()),
        Some(AddTorrentOptions {
            overwrite: true,
            output_folder: Some(source.path().to_str().unwrap().to_owned()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();

    let partial_dir = tempfile::TempDir::with_prefix("rqbit_partial_seed").unwrap();
    std::fs::copy(
        source.path().join("0.data"),
        partial_dir.path().join("0.data"),
    )
    .unwrap();
    let partial = session(15321, 4, partial_dir.path()).await;
    let handle = partial
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.clone()),
            Some(AddTorrentOptions {
                overwrite: true,
                only_files: Some(vec![0]),
                output_folder: Some(partial_dir.path().to_str().unwrap().to_owned()),
                initial_peers: Some(vec![(Ipv4Addr::LOCALHOST, 15320).into()]),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    timeout(Duration::from_secs(30), handle.wait_until_completed())
        .await
        .unwrap()
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while UPLOAD_ONLY_DISCONNECTS.load(Ordering::Relaxed) == before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "aucune déconnexion BEP 21 entre deux partageurs"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
