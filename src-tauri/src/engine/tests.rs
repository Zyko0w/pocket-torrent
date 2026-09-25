use super::*;

/// Essais de bout en bout dans un bac à sable : dossiers temporaires dédiés (jamais le vrai
/// dossier de téléchargement ni les données de l'appli) et moteur hors ligne (`Binding::Offline` :
/// ni DHT, ni trackers, ni écoute, connexions vers un proxy injoignable).
mod sandbox_tests {
    use super::*;

    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        async fn engine(&self) -> Engine {
            let settings = Settings { download_dir: self.root.join("telechargements"), ..Default::default() };
            let network = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
            Engine::start(&settings, &self.root.join("donnees"), Binding::Offline, &network).await.unwrap()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Crée un vrai .torrent (contenu factice) rangé à un chemin plein d'espaces, comme un
    /// fichier déposé ou choisi dans l'explorateur.
    async fn make_torrent(sandbox: &Sandbox, engine: &Engine) -> PathBuf {
        let content = sandbox.root.join("contenu source").join("Film de test 2026.mkv");
        std::fs::create_dir_all(content.parent().unwrap()).unwrap();
        std::fs::write(&content, vec![7u8; 300_000]).unwrap();
        let (created, handle) = engine
            .session()
            .create_and_serve_torrent(&content, Default::default())
            .await
            .expect("création du .torrent");
        // on ne garde que le fichier .torrent : le torrent créé est retiré du moteur
        engine.delete(&handle.info_hash().as_string(), false).await.unwrap();
        let target = sandbox.root.join("Mes torrents").join("Cocorico 2 TRUEFRENCH 4K (1).torrent");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, created.as_bytes().unwrap()).unwrap();
        target
    }

    #[tokio::test]
    async fn local_torrent_with_spaces_is_added_then_deleted_with_its_files() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let torrent = make_torrent(&sandbox, &engine).await;
        let link = torrent.to_string_lossy().into_owned();

        let preview = engine.preview(&link).await.expect("aperçu d'un .torrent local");
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.total_bytes, 300_000);

        let base = sandbox.root.join("telechargements");
        let hash = engine.add(&link, &base, None, false).await.expect("ajout d'un .torrent local");
        assert_eq!(hash, preview.info_hash);
        assert!(engine.torrents().iter().any(|t| t.info_hash == hash));

        // le .torrent d'origine n'est plus relu après l'ajout : il peut disparaître
        std::fs::remove_file(&torrent).unwrap();
        assert!(engine.files(&hash).is_ok());

        engine.delete(&hash, true).await.unwrap();
        assert!(engine.torrents().iter().all(|t| t.info_hash != hash));
        assert!(!base.join("Film de test 2026.mkv").exists(), "fichier commencé effacé");
        engine.stop().await;
    }

    /// Torrent à plusieurs fichiers, complet, dans `dir` ; renvoie son empreinte.
    async fn finished_pack(engine: &Engine, dir: &Path) -> String {
        let pack = dir.join("Série S01");
        std::fs::create_dir_all(pack.join("Sous-titres")).unwrap();
        std::fs::write(pack.join("e01.mkv"), vec![1u8; 300_000]).unwrap();
        std::fs::write(pack.join("Sous-titres").join("e01.srt"), vec![2u8; 5_000]).unwrap();
        let (_, handle) = engine.session().create_and_serve_torrent(&pack, Default::default()).await.unwrap();
        handle.info_hash().as_string()
    }

    async fn wait_finished(engine: &Engine, hash: &str) -> TorrentView {
        let started = Instant::now();
        loop {
            if let Some(view) =
                engine.torrents().into_iter().find(|t| t.info_hash == hash && t.state != EngineState::Initializing)
            {
                return view;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(20), "torrent jamais revenu");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Dossier « en cours » : le torrent terminé part dans son dossier final (sous-dossier du
    /// torrent gardé), y est complet, et le dossier « en cours » est vidé.
    #[tokio::test(flavor = "multi_thread")]
    async fn finished_torrent_moves_to_its_final_folder() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let incomplete = sandbox.root.join("en cours");
        let finale = sandbox.root.join("Séries");
        let hash = finished_pack(&engine, &incomplete).await;

        let folder = engine.relocate(&hash, &incomplete, &finale, true).await.unwrap();
        assert_eq!(folder, finale.join("Série S01"));
        assert_eq!(std::fs::read(folder.join("e01.mkv")).unwrap().len(), 300_000);
        assert!(folder.join("Sous-titres").join("e01.srt").exists());
        assert!(!incomplete.join("Série S01").exists(), "dossier du torrent retiré du dossier « en cours »");
        assert!(incomplete.exists(), "le dossier « en cours » reste");
        let view = wait_finished(&engine, &hash).await;
        assert_eq!(view.output_folder, folder);
        assert!(view.finished, "toutes les pièces retrouvées au nouvel emplacement");
        engine.stop().await;
    }

    /// Fichier du même nom déjà dans le dossier final : rien n'est déplacé ni écrasé, le torrent
    /// reste complet là où il était.
    #[tokio::test(flavor = "multi_thread")]
    async fn move_conflict_leaves_everything_in_place() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let incomplete = sandbox.root.join("en cours");
        let finale = sandbox.root.join("Séries");
        let hash = finished_pack(&engine, &incomplete).await;
        std::fs::create_dir_all(finale.join("Série S01")).unwrap();
        std::fs::write(finale.join("Série S01").join("e01.mkv"), b"fichier de l'utilisateur").unwrap();

        let err = engine.relocate(&hash, &incomplete, &finale, true).await.unwrap_err();
        assert!(format!("{err:#}").contains("existe déjà"), "{err:#}");
        assert_eq!(std::fs::read(finale.join("Série S01").join("e01.mkv")).unwrap(), b"fichier de l'utilisateur");
        assert!(!finale.join("Série S01").join("Sous-titres").exists(), "rien de déplacé");
        let view = wait_finished(&engine, &hash).await;
        assert_eq!(view.output_folder, incomplete.join("Série S01"));
        assert!(view.finished, "toujours complet à l'ancien emplacement");
        engine.stop().await;
    }

    /// Faux proxy SOCKS5 (127.0.0.1) : note les identifiants et chaque destination demandée,
    /// puis refuse la connexion (rien ne sort vers Internet).
    async fn fake_socks5() -> (u16, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut head = [0u8; 2];
                    socket.read_exact(&mut head).await?;
                    let mut methods = vec![0u8; head[1] as usize];
                    socket.read_exact(&mut methods).await?;
                    if methods.contains(&2) {
                        socket.write_all(&[5, 2]).await?;
                        let mut version_len = [0u8; 2];
                        socket.read_exact(&mut version_len).await?;
                        let mut user = vec![0u8; version_len[1] as usize];
                        socket.read_exact(&mut user).await?;
                        let mut pass_len = [0u8; 1];
                        socket.read_exact(&mut pass_len).await?;
                        let mut pass = vec![0u8; pass_len[0] as usize];
                        socket.read_exact(&mut pass).await?;
                        log.lock().push(format!(
                            "auth {}:{}",
                            String::from_utf8_lossy(&user),
                            String::from_utf8_lossy(&pass)
                        ));
                        socket.write_all(&[1, 0]).await?;
                    } else {
                        socket.write_all(&[5, 0]).await?;
                    }
                    let mut request = [0u8; 4];
                    socket.read_exact(&mut request).await?;
                    let host = match request[3] {
                        1 => {
                            let mut ip = [0u8; 4];
                            socket.read_exact(&mut ip).await?;
                            std::net::Ipv4Addr::from(ip).to_string()
                        }
                        3 => {
                            let mut len = [0u8; 1];
                            socket.read_exact(&mut len).await?;
                            let mut name = vec![0u8; len[0] as usize];
                            socket.read_exact(&mut name).await?;
                            String::from_utf8_lossy(&name).into_owned()
                        }
                        _ => {
                            let mut ip = [0u8; 16];
                            socket.read_exact(&mut ip).await?;
                            format!("[{}]", std::net::Ipv6Addr::from(ip))
                        }
                    };
                    let mut port = [0u8; 2];
                    socket.read_exact(&mut port).await?;
                    log.lock().push(format!("{host}:{}", u16::from_be_bytes(port)));
                    // connexion refusée
                    socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
                    Ok::<_, std::io::Error>(())
                });
            }
        });
        (port, seen)
    }

    /// Mode proxy SOCKS5 : pairs et trackers HTTP passent par le proxy (noms résolus par lui,
    /// identifiants transmis), rien d'UDP (ni DHT, ni tracker UDP), aucune écoute.
    #[tokio::test(flavor = "multi_thread")]
    async fn proxy_mode_sends_everything_through_the_proxy() {
        let sandbox = Sandbox::new();
        let (port, seen) = fake_socks5().await;
        let proxy = ProxyUrl::new("127.0.0.1", port, "moi", "p@ss:w%rd");
        assert!(
            !format!("{:?}", Binding::Proxy(proxy.clone())).contains("p@ss"),
            "mot de passe jamais dans le journal"
        );
        let network = Network::with_proxy(
            None,
            false,
            Some(proxy.url("socks5h")),
            crate::dns::Resolver::new(crate::dns::Mode::Blocked),
        );
        let settings = Settings { download_dir: sandbox.root.join("telechargements"), ..Default::default() };
        let engine = Arc::new(
            Engine::start(&settings, &sandbox.root.join("donnees"), Binding::Proxy(proxy), &network).await.unwrap(),
        );
        assert!(engine.session().get_dht().is_none(), "pas de DHT");
        assert!(engine.listen_port().is_none(), "pas d'écoute");

        let hash = "e8812d0e9b24a9a82a1376d50f2b786ee6ea797a";
        let magnet = format!(
            "magnet:?xt=urn:btih:{hash}&x.pe=203.0.113.7:6881&tr=http%3A%2F%2Ftracker.example%2Fannounce&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce"
        );
        let searching = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.preview(&magnet).await })
        };
        let wanted = ["203.0.113.7:6881", "tracker.example:80", "auth moi:p@ss:w%rd"];
        let started = Instant::now();
        while !wanted.iter().all(|w| seen.lock().iter().any(|s| s == w)) {
            assert!(started.elapsed() < std::time::Duration::from_secs(15), "vu par le proxy : {:?}", seen.lock());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let statuses = librqbit_tracker_comms::tracker_statuses(&hash.parse().unwrap());
        let udp = statuses.iter().find(|s| s.url.starts_with("udp://")).expect("tracker UDP listé");
        assert_eq!(udp.state, librqbit_tracker_comms::TrackerState::Skipped);
        assert!(udp.error.as_deref().is_some_and(|e| e.contains("proxy")), "{:?}", udp.error);
        searching.abort();
        engine.stop().await;
    }

    /// Faux serveur de liste de blocage (127.0.0.1) : compte les requêtes reçues.
    async fn fake_blocklist(body: &'static [u8]) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/liste.p2p",
            axum::routing::get(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move { body }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/liste.p2p"), hits)
    }

    fn with_blocklist(sandbox: &Sandbox, source: &str) -> Settings {
        let mut settings = Settings { download_dir: sandbox.root.join("telechargements"), ..Default::default() };
        settings.blocklist.enabled = true;
        settings.blocklist.source = source.to_owned();
        settings
    }

    /// VPN exigé mais coupé : la liste de blocage ne doit pas être téléchargée sans le VPN (elle
    /// révélerait l'adresse réelle) ; le moteur démarre quand même.
    #[tokio::test]
    async fn blocklist_is_never_fetched_outside_the_vpn() {
        let sandbox = Sandbox::new();
        let (url, hits) = fake_blocklist(b"Test:1.2.3.0-1.2.3.255\n").await;
        let offline = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
        let engine =
            Engine::start(&with_blocklist(&sandbox, &url), &sandbox.root.join("donnees"), Binding::Offline, &offline)
                .await
                .expect("le moteur démarre sans la liste");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "liste téléchargée hors du VPN");
        engine.stop().await;
    }

    /// Liste injoignable : le moteur démarre sans elle ; joignable : chargée, puis reprise de la
    /// copie locale si elle devient injoignable.
    #[tokio::test]
    async fn unreachable_blocklist_does_not_stop_the_engine() {
        let sandbox = Sandbox::new();
        let free = Network::new(None, false, crate::dns::Resolver::new(crate::dns::Mode::Free));
        let data = sandbox.root.join("donnees");
        let dead = with_blocklist(&sandbox, "http://127.0.0.1:9/liste.p2p");
        let engine = Engine::start(&dead, &data, Binding::Free, &free).await.expect("démarrage sans la liste");
        assert!(!engine.session().blocklist.has("1.2.3.4".parse().unwrap()));
        engine.stop().await;

        let (url, hits) = fake_blocklist(b"Test:1.2.3.0-1.2.3.255\n").await;
        let engine = Engine::start(&with_blocklist(&sandbox, &url), &data, Binding::Free, &free).await.unwrap();
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(engine.session().blocklist.has("1.2.3.4".parse().unwrap()));
        engine.stop().await;

        // même adresse, serveur tombé : la copie gardée sert encore
        let cached = Network::new(None, true, crate::dns::Resolver::new(crate::dns::Mode::Blocked));
        let engine = Engine::start(&with_blocklist(&sandbox, &url), &data, Binding::Offline, &cached).await.unwrap();
        assert!(engine.session().blocklist.has("1.2.3.4".parse().unwrap()));
        engine.stop().await;
    }

    /// Revérification d'un torrent à plusieurs fichiers dont un est abîmé et un autre effacé :
    /// seules les pièces touchées sont à retélécharger (pièces à cheval sur deux fichiers comprises).
    #[tokio::test(flavor = "multi_thread")]
    async fn recheck_finds_exactly_the_damaged_pieces() {
        const PIECE: u64 = 256 * 1024;
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let pack = sandbox.root.join("source").join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let sizes = [("a.bin", 4 * PIECE), ("b.bin", 4 * PIECE + 100), ("c.bin", 700 * 1024)];
        for (i, (name, len)) in sizes.iter().enumerate() {
            let bytes: Vec<u8> = (0..*len).map(|n| (n as usize * 31 + i * 7) as u8).collect();
            std::fs::write(pack.join(name), bytes).unwrap();
        }
        let options = librqbit::CreateTorrentOptions { piece_length: Some(PIECE as u32), ..Default::default() };
        let (_, handle) = engine.session().create_and_serve_torrent(&pack, options).await.unwrap();
        let hash = handle.info_hash().as_string();

        // un octet changé au début de b (pièce 4), c effacé (pièces 8 à 10, dont la fin de b)
        let mut b = std::fs::read(pack.join("b.bin")).unwrap();
        b[0] ^= 0xFF;
        std::fs::write(pack.join("b.bin"), b).unwrap();
        std::fs::remove_file(pack.join("c.bin")).unwrap();

        engine.recheck(&hash).await.unwrap();
        loop {
            if engine.torrents().iter().any(|t| t.info_hash == hash && t.state == EngineState::Live) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut files = engine.files(&hash).unwrap();
        files.sort_by_key(|f| f.index);
        let progress: Vec<u64> = files.iter().map(|f| f.progress_bytes).collect();
        assert_eq!(progress, vec![4 * PIECE, 3 * PIECE, 0]);
        engine.stop().await;
    }

    /// Mesure de la revérification (§8) : `cargo test --release --lib -- --ignored --nocapture
    /// recheck_speed`. Taille réglable par `BENCH_MIB` (1024 par défaut). Fichier pseudo-aléatoire
    /// dans le bac à sable ; relu depuis le cache de Windows, donc c'est surtout le calcul des
    /// empreintes qui est mesuré.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn recheck_speed() {
        use std::io::Write;
        let mib: usize = std::env::var("BENCH_MIB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let content = sandbox.root.join("source").join("gros fichier.bin");
        std::fs::create_dir_all(content.parent().unwrap()).unwrap();
        {
            let mut file = std::io::BufWriter::new(std::fs::File::create(&content).unwrap());
            let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
            let mut block = vec![0u8; 1024 * 1024];
            for _ in 0..mib {
                for word in block.as_chunks_mut::<8>().0 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *word = x.to_le_bytes();
                }
                file.write_all(&block).unwrap();
            }
        }
        let started = Instant::now();
        let (_, handle) = engine.session().create_and_serve_torrent(&content, Default::default()).await.unwrap();
        println!("création du .torrent ({mib} Mio) : {:?}", started.elapsed());
        let hash = handle.info_hash().as_string();
        let started = Instant::now();
        engine.recheck(&hash).await.unwrap();
        loop {
            let done =
                engine.torrents().iter().any(|t| t.info_hash == hash && t.state == EngineState::Live && t.finished);
            if done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let elapsed = started.elapsed();
        println!("revérification : {elapsed:?} ({:.0} Mio/s)", mib as f64 / elapsed.as_secs_f64());
        engine.stop().await;
    }

    #[tokio::test]
    async fn per_torrent_download_limit_reaches_the_running_engine() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let torrent = make_torrent(&sandbox, &engine).await;
        let base = sandbox.root.join("telechargements");
        let hash = engine.add(&torrent.to_string_lossy(), &base, None, false).await.unwrap();
        let live = loop {
            if let Some(live) = engine.handle(&hash).unwrap().live() {
                break live;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        engine.set_torrent_download_limit(&hash, Some(64 * 1024));
        assert_eq!(live.download_limit().map(|b| b.get()), Some(64 * 1024));
        engine.set_torrent_download_limit(&hash, None);
        assert_eq!(live.download_limit(), None);
        engine.stop().await;
    }

    #[tokio::test]
    async fn missing_torrent_file_says_so() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let missing = sandbox.root.join("Téléchargements").join("Cocorico.torrent");
        let err = engine.preview(&missing.to_string_lossy()).await.err().expect("erreur attendue");
        assert!(format!("{err:#}").contains("Fichier .torrent introuvable"), "{err:#}");
        let err = engine.preview("pas un lien").await.err().expect("erreur attendue");
        assert!(format!("{err:#}").contains("Lien non reconnu"), "{err:#}");
        engine.stop().await;
    }

    #[tokio::test]
    async fn magnet_without_any_peer_source_gives_a_readable_error() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let magnet = "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&tr=udp://tracker.opentrackr.org:1337/announce";
        let err = engine.preview(magnet).await.err().expect("erreur attendue");
        let text = format!("{err:#}");
        assert!(!text.contains("no known way"), "message brut du moteur : {text}");
        assert!(text.contains("VPN coupé"), "{text}");
        engine.stop().await;
    }

    #[tokio::test]
    async fn magnet_search_counts_the_peers_it_tries() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        // un seul pair, injoignable (moteur hors ligne) : compté, puis plus aucune source
        let magnet = "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&x.pe=127.0.0.1:9";
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let err = engine.preview_counting(magnet, Some(seen.clone())).await.err().expect("erreur attendue");
        assert_eq!(seen.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(format!("{err:#}").contains("Aucun pair n'a transmis"), "{err:#}");
        engine.stop().await;
    }

    #[test]
    fn engine_restarts_only_for_effective_changes() {
        let base = Settings::default();
        let same = |edit: &dyn Fn(&mut Settings)| {
            let mut next = base.clone();
            edit(&mut next);
            EngineConfig::of(&base) == EngineConfig::of(&next)
        };
        assert!(same(&|s| s.blocklist.source = "C:/listes/p2p.txt".into()), "liste désactivée");
        assert!(same(&|s| s.limits.down_kbps = 500), "limites appliquées sans redémarrage");
        assert!(!same(&|s| s.privacy.hide_announces = true));
        assert!(!same(&|s| s.torrent_port = 51413));

        let mut hardened = base.clone();
        hardened.privacy.hardened_mode = true;
        let mut both = hardened.clone();
        both.privacy.hide_announces = true;
        assert_eq!(EngineConfig::of(&hardened), EngineConfig::of(&both), "déjà inclus dans le mode renforcé");
    }

    /// Bencode minimal pour forger des .torrent hostiles.
    enum B<'a> {
        Int(i64),
        Str(&'a [u8]),
        List(Vec<B<'a>>),
        Dict(Vec<(&'a str, B<'a>)>),
    }

    fn bencode(value: &B, out: &mut Vec<u8>) {
        match value {
            B::Int(i) => out.extend(format!("i{i}e").as_bytes()),
            B::Str(s) => {
                out.extend(format!("{}:", s.len()).as_bytes());
                out.extend(*s);
            }
            B::List(items) => {
                out.push(b'l');
                items.iter().for_each(|item| bencode(item, out));
                out.push(b'e');
            }
            B::Dict(entries) => {
                out.push(b'd');
                let mut sorted: Vec<_> = entries.iter().collect();
                sorted.sort_by_key(|(k, _)| *k);
                for (key, item) in sorted {
                    bencode(&B::Str(key.as_bytes()), out);
                    bencode(item, out);
                }
                out.push(b'e');
            }
        }
    }

    /// .torrent à plusieurs fichiers dont les chemins sont `paths` (composants bruts).
    fn hostile_torrent(name: &[u8], paths: &[&[&[u8]]]) -> Vec<u8> {
        let files = paths
            .iter()
            .map(|path| {
                B::Dict(vec![("length", B::Int(10)), ("path", B::List(path.iter().map(|c| B::Str(c)).collect()))])
            })
            .collect();
        let pieces = vec![0u8; 20];
        let info = B::Dict(vec![
            ("files", B::List(files)),
            ("name", B::Str(name)),
            ("piece length", B::Int(16384)),
            ("pieces", B::Str(&pieces)),
        ]);
        let mut out = Vec::new();
        bencode(&B::Dict(vec![("info", info)]), &mut out);
        out
    }

    /// Tous les fichiers et dossiers sous `root`.
    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let path = entry.path();
            found.push(path.clone());
            if path.is_dir() {
                found.extend(walk(&path));
            }
        }
        found
    }

    #[tokio::test]
    async fn hostile_torrents_cannot_write_outside_the_download_folder() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let base = sandbox.root.join("telechargements");
        let before: Vec<PathBuf> = walk(&sandbox.root);

        // rejetés d'emblée : remontée de dossier, séparateur caché dans un nom
        for (label, bytes) in [
            ("remontée ..", hostile_torrent(b"film", &[&[b"..", b"..", b"evil.txt"]])),
            ("séparateur /", hostile_torrent(b"film", &[&[b"a/../../evil.txt"]])),
            ("nom de torrent ..", hostile_torrent(b"..", &[&[b"a.txt"], &[b"b.txt"]])),
            ("séparateur \\", hostile_torrent(b"film", &[&[b"..\\..\\evil.txt"]])),
        ] {
            let file = sandbox.root.join(format!("hostile-{}.torrent", label.replace(['/', '\\', '.', ' '], "_")));
            std::fs::write(&file, &bytes).unwrap();
            let result = engine.preview(&file.to_string_lossy()).await;
            assert!(result.is_err(), "{label} : accepté");
        }

        // acceptés mais neutralisés : lecteur, flux NTFS caché, noms réservés, points finaux,
        // et nom de torrent qui tente de sortir du dossier
        let tricky = hostile_torrent(
            b"film",
            &[
                &[b"C:", b"evil.txt"],
                &[b"D:evil.txt"],
                &[b"video.mkv:cache.exe"],
                &[b"CON"],
                &[b"sub", b"nul.txt"],
                &[b"COM1.log"],
                &[b"fin. . "],
            ],
        );
        let file = sandbox.root.join("hostile-tricky.torrent");
        std::fs::write(&file, &tricky).unwrap();
        match engine.add(&file.to_string_lossy(), &base, None, false).await {
            Ok(hash) => {
                // hors les .torrent piégés écrits par ce test
                let written: Vec<PathBuf> = walk(&sandbox.root)
                    .into_iter()
                    .filter(|p| !before.contains(p))
                    .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("hostile-")))
                    .collect();
                for path in &written {
                    assert!(
                        path.starts_with(&base) || path.starts_with(sandbox.root.join("donnees")) || path == &file,
                        "écrit hors du dossier de téléchargement : {}",
                        path.display()
                    );
                    let name = path.file_name().unwrap().to_string_lossy();
                    assert!(!name.contains(':'), "flux NTFS ou lecteur dans {}", path.display());
                    let stem = name.split('.').next().unwrap().to_uppercase();
                    assert!(!["CON", "NUL", "COM1"].contains(&stem.as_str()), "nom réservé : {}", path.display());
                }
                engine.delete(&hash, true).await.unwrap();
            }
            // refuser le torrent entier est aussi une défense valable
            Err(err) => eprintln!("torrent piégé refusé : {err:#}"),
        }
        assert!(!Path::new(r"C:\evil.txt").exists() && !Path::new(r"D:\evil.txt").exists());
        engine.stop().await;
    }

    #[test]
    fn paths_are_told_apart_from_links() {
        assert!(looks_like_path(r"C:\Users\x\Downloads\Cocorico.torrent"));
        assert!(looks_like_path(r"\\nas\partage\a.torrent"));
        assert!(looks_like_path("/home/x/a"));
        assert!(!looks_like_path("magnet:?xt=urn:btih:abc"));
        assert!(!looks_like_path("n'importe quoi"));
    }

    /// Actions du volet d'un torrent sur un torrent de 3 fichiers pas encore téléchargé :
    /// ordre de priorité, choix des fichiers, pause/reprise, lien magnet, empreinte inconnue.
    #[tokio::test(flavor = "multi_thread")]
    async fn torrent_actions_work_and_refuse_bad_input() {
        let sandbox = Sandbox::new();
        let engine = sandbox.engine().await;
        let pack = sandbox.root.join("source").join("Série & Co #1");
        std::fs::create_dir_all(&pack).unwrap();
        for (name, len) in [("a.mkv", 200_000), ("b.mkv", 100_000), ("c.srt", 3_000)] {
            std::fs::write(pack.join(name), vec![3u8; len]).unwrap();
        }
        let (created, handle) = engine.session().create_and_serve_torrent(&pack, Default::default()).await.unwrap();
        let hash = handle.info_hash().as_string();
        engine.delete(&hash, false).await.unwrap();
        let file = sandbox.root.join("pack.torrent");
        std::fs::write(&file, created.as_bytes().unwrap()).unwrap();
        let base = sandbox.root.join("telechargements");
        assert_eq!(engine.add(&file.to_string_lossy(), &base, None, false).await.unwrap(), hash);
        wait_finished(&engine, &hash).await;

        let paths = |engine: &Engine| -> Vec<String> {
            engine.files(&hash).unwrap().into_iter().map(|f| f.path.rsplit('/').next().unwrap().to_owned()).collect()
        };
        assert_eq!(paths(&engine), ["a.mkv", "b.mkv", "c.srt"]);

        // priorité : le dernier remonte d'un cran ; le premier ne peut pas monter plus haut
        let c = engine.files(&hash).unwrap().iter().find(|f| f.path.ends_with("c.srt")).unwrap().index;
        engine.move_file_priority(&hash, c, true).await.unwrap();
        assert_eq!(paths(&engine), ["a.mkv", "c.srt", "b.mkv"]);
        let first = engine.files(&hash).unwrap()[0].index;
        let unchanged = engine.move_file_priority(&hash, first, true).await.unwrap();
        assert_eq!(unchanged[0], first);
        assert!(engine.move_file_priority(&hash, 99, true).await.is_err(), "fichier inexistant");
        assert!(engine.restore_file_priorities(&hash, &[0, 0, 1]).is_err(), "ordre incohérent refusé");

        // choix des fichiers
        engine.set_files(&hash, vec![0]).await.unwrap();
        let included: Vec<bool> =
            engine.files(&hash).unwrap().iter().filter(|f| f.index == 0).map(|f| f.included).collect();
        assert_eq!(included, [true]);
        assert_eq!(engine.files(&hash).unwrap().iter().filter(|f| f.included).count(), 1);
        assert!(engine.set_files(&hash, vec![]).await.is_err(), "aucun fichier : refusé");
        let _ = engine.set_files(&hash, vec![999]).await; // hors limites : sans panique
        assert_eq!(engine.files(&hash).unwrap().len(), 3);

        // pause et reprise
        engine.pause(&hash).await.unwrap();
        engine.pause(&hash).await.unwrap();
        assert!(engine.torrents().iter().any(|t| t.info_hash == hash && t.state == EngineState::Paused));
        engine.resume(&hash).await.unwrap();
        assert!(engine.torrents().iter().any(|t| t.info_hash == hash && t.state != EngineState::Paused));

        // lien magnet : nom encodé (« & » et « # » ne coupent pas le lien)
        let magnet = engine.magnet(&hash).unwrap();
        assert!(magnet.starts_with(&format!("magnet:?xt=urn:btih:{hash}&dn=")), "{magnet}");
        let dn = magnet.split("&dn=").nth(1).unwrap().split('&').next().unwrap();
        assert!(!dn.contains('#') && !dn.contains(' '), "{magnet}");
        assert_eq!(percent_encoding::percent_decode_str(dn).decode_utf8().unwrap(), "Série & Co #1");

        // empreinte inconnue : erreurs lisibles, jamais de panique
        let unknown = "0".repeat(40);
        assert!(engine.files(&unknown).is_err());
        assert!(engine.pause(&unknown).await.is_err());
        assert!(engine.magnet("pas une empreinte").is_err());
        engine.stop().await;
    }
}

mod magnet_tests {
    #[test]
    fn v2_only_magnets_get_a_clear_error_and_hybrids_pass() {
        let v1 = "xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a";
        let v2 = "xt=urn:btmh:1220caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e";
        assert!(super::add_source(&format!("magnet:?{v1}&{v2}&dn=hybride")).is_ok());
        let err = super::add_source(&format!("magnet:?{v2}")).err().unwrap().to_string();
        assert!(err.contains("BitTorrent v2"), "{err}");
        let err = super::add_source("magnet:?xt=urn:btih:zz").err().unwrap().to_string();
        assert!(err.contains("invalide"), "{err}");
    }
}

mod selection_tests {
    #[test]
    fn selection_mask_ignores_out_of_range_indices() {
        let mask = super::selection_mask(Some(&[0, 2, 99]), 3).unwrap();
        assert_eq!(mask, vec![true, false, true]);
        assert!(super::is_selected(None, 5), "sans choix : tout est choisi");
        assert!(!super::is_selected(Some(&mask), 7));
    }
}
