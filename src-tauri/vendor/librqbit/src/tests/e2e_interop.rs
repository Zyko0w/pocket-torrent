//! Interopérabilité avec un autre client BitTorrent qui partage un torrent en local (Torrent
//! Cloud : vérification de l'extension Fast BEP 6, du choking et du chiffrement MSE face à
//! qBittorrent). Ignoré par défaut, lancé à la main :
//!
//! INTEROP_PEER=127.0.0.1:58960 INTEROP_TORRENT=chemin.torrent INTEROP_SOURCE=dossier_des_données \
//!   cargo test ... --lib test_interop_download -- --ignored --nocapture

use std::time::Duration;

use tokio::time::timeout;

use crate::{
    AddTorrent, AddTorrentOptions, ConnectionOptions, Session, SessionOptions, mse::EncryptionMode,
    tests::test_util::setup_test_logging,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_interop_download() {
    setup_test_logging();
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("variable {k} manquante"));
    let peer: std::net::SocketAddr = env("INTEROP_PEER").parse().unwrap();
    let torrent = std::fs::read(env("INTEROP_TORRENT")).unwrap();
    let source = std::path::PathBuf::from(env("INTEROP_SOURCE"));

    let out = tempfile::TempDir::with_prefix("rqbit_interop").unwrap();
    let session = Session::new_with_opts(
        out.path().to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: None,
            connect: Some(ConnectionOptions {
                encryption: EncryptionMode::Prefer,
                ..Default::default()
            }),
            upload_slots: Some(4),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let handle = session
        .add_torrent(
            AddTorrent::TorrentFileBytes(torrent.into()),
            Some(AddTorrentOptions {
                initial_peers: Some(vec![peer]),
                output_folder: Some(out.path().to_str().unwrap().to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();

    timeout(Duration::from_secs(120), handle.wait_until_completed())
        .await
        .expect("téléchargement depuis l'autre client pas terminé en 2 min")
        .unwrap();

    let mut compared = 0;
    for entry in std::fs::read_dir(&source).unwrap().flatten() {
        let copy = out.path().join(entry.file_name());
        assert_eq!(
            std::fs::read(entry.path()).unwrap(),
            std::fs::read(&copy).unwrap(),
            "{copy:?} différent"
        );
        compared += 1;
    }
    assert!(compared > 0);
    tracing::info!(compared, "interop : fichiers identiques");
}

/// Sens inverse : on partage sur INTEROP_LISTEN (ex. 127.0.0.1:15400) et on attend que l'autre
/// client ait tout reçu (ignoré par défaut, lancé à la main comme `test_interop_download`).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_interop_seed() {
    setup_test_logging();
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("variable {k} manquante"));
    let listen: std::net::SocketAddr = env("INTEROP_LISTEN").parse().unwrap();
    let torrent = std::fs::read(env("INTEROP_TORRENT")).unwrap();
    let source = std::path::PathBuf::from(env("INTEROP_SOURCE"));

    let session = Session::new_with_opts(
        source.clone(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: Some(crate::listen::ListenerOptions {
                mode: crate::ListenerMode::TcpOnly,
                listen_addr: listen,
                ..Default::default()
            }),
            connect: Some(ConnectionOptions {
                encryption: EncryptionMode::Prefer,
                ..Default::default()
            }),
            upload_slots: Some(4),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let handle = session
        .add_torrent(
            AddTorrent::TorrentFileBytes(torrent.into()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(source.to_str().unwrap().to_owned()),
                // torrent privé sans tracker : c'est nous qui allons vers l'autre client
                initial_peers: std::env::var("INTEROP_PEER")
                    .ok()
                    .map(|p| vec![p.parse().unwrap()]),
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
    let total = handle.stats().total_bytes;
    tracing::info!(total, "interop : partage prêt");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let uploaded = handle.stats().uploaded_bytes;
        if uploaded >= total {
            tracing::info!(uploaded, total, "interop : tout a été envoyé");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "l'autre client n'a reçu que {uploaded}/{total} octets en 5 min"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // laisse le temps au dernier bloc d'arriver avant de fermer
    tokio::time::sleep(Duration::from_secs(3)).await;
}
