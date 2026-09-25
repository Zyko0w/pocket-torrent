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
