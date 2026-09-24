//! Sources web (BEP 19, Torrent Cloud) : un torrent sans aucun pair, dont la clé `url-list`
//! pointe vers un petit serveur HTTP local qui gère les requêtes partielles (Range). Le client
//! doit tout télécharger depuis ce serveur, pièces à cheval sur deux fichiers comprises.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

use crate::{
    AddTorrent, AddTorrentOptions, Session, SessionOptions, create_torrent,
    spawn_utils::BlockingSpawner, tests::test_util::setup_test_logging,
};

/// Serveur HTTP minimal : GET avec « Range: bytes=a-b » sur les fichiers de `root`.
async fn serve(root: PathBuf, listener: tokio::net::TcpListener, ranges: Arc<AtomicUsize>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let root = root.clone();
        let ranges = ranges.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let mut len = 0;
            while !buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf[len..]).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => len += n,
                }
            }
            let request = String::from_utf8_lossy(&buf[..len]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let decoded = percent_decode(&path);
            let file = root.join(decoded.trim_start_matches('/'));
            let Ok(data) = std::fs::read(&file) else {
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                    .await;
                return;
            };
            let range = request.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("range: bytes=")
                    .map(str::to_owned)
            });
            let (status, body, extra) = match range.and_then(|r| {
                let (a, b) = r.trim().split_once('-')?;
                Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
            }) {
                Some((a, b)) if a <= b && b < data.len() => {
                    ranges.fetch_add(1, Ordering::Relaxed);
                    (
                        "206 Partial Content",
                        &data[a..=b],
                        format!("Content-Range: bytes {a}-{b}/{}\r\n", data.len()),
                    )
                }
                _ => ("200 OK", &data[..], String::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(body).await;
        });
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_webseed_only() {
    setup_test_logging();
    // dossier source avec des fichiers de tailles « bancales » (pièces à cheval sur deux fichiers)
    let source = tempfile::TempDir::with_prefix("rqbit webseed").unwrap();
    let sizes = [300_001usize, 77_777, 1_000_000];
    for (i, size) in sizes.iter().enumerate() {
        let data: Vec<u8> = (0..*size)
            .map(|n| u8::try_from((n * 31 + i * 7) % 251).unwrap())
            .collect();
        std::fs::write(source.path().join(format!("fichier {i}.bin")), data).unwrap();
    }
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

    // serveur sur le dossier parent : URL = base / nom du torrent / fichier
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let ranges = Arc::new(AtomicUsize::new(0));
    tokio::spawn(serve(
        source.path().parent().unwrap().to_owned(),
        listener,
        ranges.clone(),
    ));

    // .torrent + url-list (clé ajoutée en dernier : « url-list » vient après les autres clés)
    let mut bytes = torrent.as_bytes().unwrap().to_vec();
    assert_eq!(bytes.pop(), Some(b'e'));
    let url = format!("http://127.0.0.1:{port}/");
    bytes.extend_from_slice(format!("8:url-list{}:{url}e", url.len()).as_bytes());

    let out = tempfile::TempDir::with_prefix("rqbit_webseed_client").unwrap();
    let session = Session::new_with_opts(
        out.path().to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen: None,
            // le serveur de test est sur 127.0.0.1, refusé par défaut
            webseeds_allow_local_network: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let handle = session
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.into()),
            Some(AddTorrentOptions {
                output_folder: Some(out.path().to_str().unwrap().to_owned()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_handle()
        .unwrap();
    timeout(Duration::from_secs(60), handle.wait_until_completed())
        .await
        .expect("pas terminé depuis la source web en 60 s")
        .unwrap();

    for i in 0..sizes.len() {
        let name = format!("fichier {i}.bin");
        assert_eq!(
            std::fs::read(source.path().join(&name)).unwrap(),
            std::fs::read(out.path().join(&name)).unwrap(),
            "{name} différent"
        );
    }
    assert!(
        ranges.load(Ordering::Relaxed) > sizes.len(),
        "des requêtes partielles attendues"
    );
}
