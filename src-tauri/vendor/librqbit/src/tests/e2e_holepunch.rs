//! Perçage de NAT (BEP 55, Pocket Torrent) : A et C, connectés tous deux à B, ne se connaissent
//! pas. A demande à B une mise en relation avec C : B prévient les deux, qui se connectent
//! l'un à l'autre. Tout sur 127.0.0.1 (pas de vrai routeur : c'est le protocole et les trois
//! rôles qui sont vérifiés, pas la traversée d'un vrai NAT).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::{
    AddTorrent, AddTorrentOptions, ListenerMode, Session, SessionOptions, create_torrent,
    listen::ListenerOptions,
    spawn_utils::BlockingSpawner,
    tests::test_util::{create_default_random_dir_with_torrents, setup_test_logging},
    torrent_state::live::{HOLEPUNCH_CONNECTS, HOLEPUNCH_RELAYED},
};

const BASE: u16 = 15460;

async fn session(port: u16, out: &std::path::Path) -> std::sync::Arc<Session> {
    Session::new_with_opts(
        out.to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                listen_addr: (Ipv4Addr::LOCALHOST, port).into(),
                // sur 127.0.0.1, le moteur n'annonce aucun port d'écoute (champ `p`) : forcé ici
                announce_port: Some(port),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_puts_two_peers_in_touch() {
    setup_test_logging();
    let source =
        create_default_random_dir_with_torrents(1, 4 * 1024 * 1024, Some("rqbit_holepunch"));
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

    // B : partageur et intermédiaire, bridé pour que A et C restent en cours de téléchargement
    let relay = Session::new_with_opts(
        source.path().to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                listen_addr: addr(BASE).into(),
                announce_port: Some(BASE),
                ..Default::default()
            }),
            ratelimits: crate::limits::LimitsConfig {
                upload_bps: std::num::NonZeroU32::new(64 * 1024),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    relay
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.clone()),
            Some(AddTorrentOptions {
                overwrite: true,
                output_folder: Some(source.path().to_str().unwrap().to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let mut clients = Vec::new();
    for port in [BASE + 1, BASE + 2] {
        let out = tempfile::TempDir::with_prefix("rqbit_holepunch_client").unwrap();
        let session = session(port, out.path()).await;
        let handle = session
            .add_torrent(
                AddTorrent::TorrentFileBytes(bytes.clone()),
                Some(AddTorrentOptions {
                    initial_peers: Some(vec![addr(BASE)]),
                    output_folder: Some(out.path().to_str().unwrap().to_owned()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_handle()
            .unwrap();
        clients.push((out, session, handle));
    }
    let (a, c) = (&clients[0].2, &clients[1].2);

    // les deux clients connectés à B (poignées de main échangées)
    let started = Instant::now();
    let live_a = loop {
        if let Some(live) = a.live()
            && live
                .peers
                .with_live(addr(BASE), |l| l.holepunch)
                .unwrap_or(false)
            && c.live().is_some_and(|c| {
                c.peers
                    .with_live(addr(BASE), |l| l.holepunch)
                    .unwrap_or(false)
            })
        {
            break live;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "clients jamais connectés à B"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        live_a.peers.with_live(addr(BASE + 2), |_| ()).is_none(),
        "A connaît déjà C : test inutile"
    );

    let (relayed, connects) = (
        HOLEPUNCH_RELAYED.load(Ordering::Relaxed),
        HOLEPUNCH_CONNECTS.load(Ordering::Relaxed),
    );
    live_a.holepunch_rendezvous(addr(BASE), addr(BASE + 2));

    // A et C connectés l'un à l'autre, par la mise en relation. Les deux « Connect » peuvent
    // arriver après la connexion elle-même (machine chargée) : on attend les trois.
    loop {
        let a_sees_c = live_a.peers.with_live(addr(BASE + 2), |_| ()).is_some();
        let c_sees_a = c.live().is_some_and(|live| {
            live.peers
                .states
                .iter()
                .any(|e| e.key().port() != BASE && e.value().get_live().is_some())
        });
        let was_relayed = HOLEPUNCH_RELAYED.load(Ordering::Relaxed) > relayed;
        let both_told = HOLEPUNCH_CONNECTS.load(Ordering::Relaxed) >= connects + 2;
        if (a_sees_c || c_sees_a) && was_relayed && both_told {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "A et C jamais mis en relation (connectés : {}, relayé : {was_relayed}, tous deux prévenus : {both_told})",
            a_sees_c || c_sees_a
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
