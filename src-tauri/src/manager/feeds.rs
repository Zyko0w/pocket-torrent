//! Flux RSS : vérification périodique, filtres, ajout des nouveaux épisodes (voir rss.rs).
//!
//! À la première lecture d'un flux, rien n'est téléchargé : ses éléments sont seulement notés
//! comme vus, pour ne pas ajouter tout l'historique d'un coup. Ensuite, chaque nouvel élément qui
//! passe les filtres est ajouté une fois ; un épisode déjà ajouté (S02E05) n'est pas repris dans
//! une autre qualité. Le flux est lu par le même chemin réseau que les téléchargements (VPN).

use std::collections::VecDeque;

use super::*;
use crate::rss;
use crate::settings::RssFeed;

/// Taille maximale d'un flux lu (un flux de 300 éléments pèse quelques centaines de Kio).
const MAX_FEED_BYTES: usize = 5 * 1024 * 1024;
/// Éléments et épisodes retenus par flux (les plus anciens sont oubliés).
const MEMORY: usize = 3000;
const SEEN_FILE: &str = "rss-vus.json";

/// Éléments déjà vus d'un flux, gardés d'un lancement à l'autre.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct FeedSeen {
    initialized: bool,
    ids: VecDeque<String>,
    episodes: VecDeque<String>,
}

impl FeedSeen {
    fn remember(list: &mut VecDeque<String>, value: String) {
        if list.len() >= MEMORY {
            list.pop_front();
        }
        list.push_back(value);
    }
}

#[derive(Default)]
pub(super) struct Feeds {
    seen: Option<HashMap<String, FeedSeen>>,
    status: HashMap<String, FeedStatus>,
    last_run: Option<Instant>,
    busy: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedStatus {
    pub id: String,
    /// Dernière vérification, en ms depuis 1970.
    pub checked_at: Option<u64>,
    pub error: Option<String>,
    pub added: u32,
    /// Titres des derniers éléments ajoutés (5 au plus).
    pub last_added: Vec<String>,
    /// Première lecture faite : les éléments présents à ce moment ont été ignorés.
    pub ready: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedPreviewItem {
    pub title: String,
    pub matched: bool,
}

/// Ce que la vérification d'un flux doit faire de ses éléments (logique pure, testée à part).
pub(super) fn select_new(
    seen: &mut FeedSeen,
    items: &[rss::FeedItem],
    include: &str,
    exclude: &str,
    skip_repeats: bool,
) -> Vec<(rss::FeedItem, Option<String>)> {
    if !seen.initialized {
        seen.initialized = true;
        for item in items {
            FeedSeen::remember(&mut seen.ids, item.id.clone());
        }
        return Vec::new();
    }
    let mut chosen: Vec<(rss::FeedItem, Option<String>)> = Vec::new();
    // du plus ancien au plus récent : les épisodes arrivent dans l'ordre
    for item in items.iter().rev() {
        if seen.ids.contains(&item.id) {
            continue;
        }
        FeedSeen::remember(&mut seen.ids, item.id.clone());
        if !rss::matches(&item.title, include, exclude) {
            continue;
        }
        let episode = if skip_repeats { rss::episode_key(&item.title) } else { None };
        if let Some(key) = &episode
            && (seen.episodes.contains(key) || chosen.iter().any(|(_, e)| e.as_ref() == Some(key)))
        {
            continue;
        }
        chosen.push((item.clone(), episode));
    }
    chosen
}

impl Manager {
    fn seen_path(&self) -> PathBuf {
        self.data_dir.join(SEEN_FILE)
    }

    /// Appelé chaque seconde : lance la vérification quand l'intervalle est écoulé.
    pub(super) fn maintain_feeds(self: &Arc<Self>) {
        // appelé chaque seconde : juste les deux valeurs utiles, sans copier la liste des flux
        let (active, interval_minutes) = {
            let settings = self.settings.read();
            (settings.rss.feeds.iter().any(|f| f.enabled), settings.rss.interval_minutes)
        };
        if !active {
            return;
        }
        {
            let mut feeds = self.feeds.lock();
            let every = Duration::from_secs(u64::from(interval_minutes) * 60);
            if feeds.busy || feeds.last_run.is_some_and(|at| at.elapsed() < every) {
                return;
            }
            feeds.busy = true;
            feeds.last_run = Some(Instant::now());
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.check_feeds().await;
            manager.feeds.lock().busy = false;
        });
    }

    /// Bouton « Vérifier maintenant ».
    pub async fn check_feeds_now(self: &Arc<Self>) {
        {
            let mut feeds = self.feeds.lock();
            if feeds.busy {
                return;
            }
            feeds.busy = true;
            feeds.last_run = Some(Instant::now());
        }
        self.check_feeds().await;
        self.feeds.lock().busy = false;
    }

    async fn check_feeds(self: &Arc<Self>) {
        let settings = self.settings();
        let mut added_titles = Vec::new();
        for feed in settings.rss.feeds.iter().filter(|f| f.enabled) {
            let result = self.check_feed(feed, &settings).await;
            let mut feeds = self.feeds.lock();
            let status = feeds
                .status
                .entry(feed.id.clone())
                .or_insert_with(|| FeedStatus { id: feed.id.clone(), ..Default::default() });
            status.checked_at = Some(now_ms());
            status.ready = true;
            match result {
                Ok(titles) => {
                    status.error = None;
                    status.added += titles.len() as u32;
                    status.last_added.splice(0..0, titles.iter().cloned());
                    status.last_added.truncate(5);
                    added_titles.extend(titles);
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    tracing::warn!("flux RSS « {} » : {message}", feed.name);
                    status.error = Some(message);
                }
            }
        }
        self.save_seen();
        match added_titles.len() {
            0 => {}
            1 => self.notify(Notify::Done, "Flux RSS : nouvel épisode ajouté", &added_titles[0]),
            n => self.notify(Notify::Done, "Flux RSS", &format!("{n} nouveaux éléments ajoutés")),
        }
    }

    /// Lit un flux et ajoute ses nouveaux éléments. Renvoie les titres ajoutés.
    async fn check_feed(self: &Arc<Self>, feed: &RssFeed, settings: &Settings) -> anyhow::Result<Vec<String>> {
        let items = self.fetch_feed(&feed.url).await?;
        let chosen = {
            let mut feeds = self.feeds.lock();
            let seen = self.load_seen(&mut feeds).entry(feed.id.clone()).or_default();
            select_new(seen, &items, &feed.include, &feed.exclude, feed.skip_repeats)
        };
        let options = AddOptions { category: feed.category.clone(), ..Default::default() };
        let mut added = Vec::new();
        for (item, episode) in chosen {
            if !rss::link_allowed(&feed.url, &item.link) {
                tracing::warn!("flux RSS « {} » : lien vers le réseau local refusé pour « {} »", feed.name, item.title);
                continue;
            }
            let link = match self.feed_link(feed, &item.link, &item.title).await {
                Ok(link) => link,
                Err(err) => {
                    tracing::warn!("flux RSS « {} » : « {} » non ajouté : {err:#}", feed.name, item.title);
                    continue;
                }
            };
            match self.add_link(&link, &options, settings).await {
                Ok(()) => added.push(item.title.clone()),
                // déjà là (ajouté à la main, ou par un autre flux) : l'épisode compte comme récupéré
                Err(err) if format!("{err}").contains("déjà dans la liste") => {}
                Err(err) => {
                    tracing::warn!("flux RSS « {} » : « {} » non ajouté : {err:#}", feed.name, item.title);
                    continue;
                }
            }
            if let Some(key) = episode {
                let mut feeds = self.feeds.lock();
                let seen = self.load_seen(&mut feeds).entry(feed.id.clone()).or_default();
                FeedSeen::remember(&mut seen.episodes, key);
            }
        }
        if !added.is_empty() {
            self.save_now();
        }
        Ok(added)
    }

    /// Lien à ajouter pour un élément : le magnet tel quel ; un .torrent venu d'Internet est
    /// téléchargé ici par le client limité à Internet (adresse réellement jointe contrôlée,
    /// redirections comprises) puis ajouté comme un fichier déposé.
    async fn feed_link(&self, feed: &RssFeed, link: &str, title: &str) -> anyhow::Result<String> {
        if link.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("magnet:?")) || !rss::needs_public_only(&feed.url, link)
        {
            return Ok(link.to_owned());
        }
        let bytes = download_torrent(&self.network().public_client(), link).await?;
        Ok(self.save_dropped_torrent(title, &bytes)?.to_string_lossy().into_owned())
    }

    /// Télécharge et lit un flux par le client réseau des téléchargements (VPN compris).
    async fn fetch_feed(&self, url: &str) -> anyhow::Result<Vec<rss::FeedItem>> {
        fetch(&self.http(), url).await
    }

    /// Bouton « Tester » : éléments du flux et ceux qui passeraient les filtres, sans rien ajouter.
    pub async fn preview_feed(&self, url: &str, include: &str, exclude: &str) -> anyhow::Result<Vec<FeedPreviewItem>> {
        let items = self.fetch_feed(url.trim()).await?;
        Ok(items
            .into_iter()
            .take(50)
            .map(|item| FeedPreviewItem { matched: rss::matches(&item.title, include, exclude), title: item.title })
            .collect())
    }

    pub fn feed_status(&self) -> Vec<FeedStatus> {
        let feeds = self.feeds.lock();
        self.settings
            .read()
            .rss
            .feeds
            .iter()
            .map(|f| {
                feeds
                    .status
                    .get(&f.id)
                    .cloned()
                    .unwrap_or_else(|| FeedStatus { id: f.id.clone(), ..Default::default() })
            })
            .collect()
    }

    fn load_seen<'a>(&self, feeds: &'a mut Feeds) -> &'a mut HashMap<String, FeedSeen> {
        feeds.seen.get_or_insert_with(|| {
            std::fs::read(self.seen_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        })
    }

    fn save_seen(&self) {
        let json = {
            let feeds = self.feeds.lock();
            let Some(seen) = &feeds.seen else { return };
            // flux retirés des réglages : oubliés
            let kept: HashMap<&String, &FeedSeen> = {
                let settings = self.settings.read();
                seen.iter().filter(|(id, _)| settings.rss.feeds.iter().any(|f| &f.id == *id)).collect()
            };
            serde_json::to_vec(&kept)
        };
        match json {
            Ok(json) => {
                if let Err(err) = crate::store::write_atomic(&self.seen_path(), &json) {
                    tracing::warn!("mémoire des flux RSS non enregistrée : {err:#}");
                }
            }
            Err(err) => tracing::warn!("mémoire des flux RSS illisible : {err}"),
        }
    }
}

/// Télécharge et lit un flux : http(s) seulement, 30 s et 5 Mio au plus.
pub(super) async fn fetch(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<rss::FeedItem>> {
    let parsed = url::Url::parse(url).context("adresse du flux invalide")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("adresse du flux invalide (http ou https attendu)");
    }
    let mut response = client
        .get(parsed)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|err| anyhow!("flux injoignable : {}", err.without_url()))?;
    if !response.status().is_success() {
        bail!("le serveur du flux a répondu HTTP {}", response.status());
    }
    let mut body = Vec::new();
    while let Some(chunk) =
        response.chunk().await.map_err(|err| anyhow!("lecture du flux interrompue : {}", err.without_url()))?
    {
        if body.len() + chunk.len() > MAX_FEED_BYTES {
            bail!("flux trop gros (plus de 5 Mio)");
        }
        body.extend_from_slice(&chunk);
    }
    rss::parse_feed(&String::from_utf8_lossy(&body))
}

/// Télécharge un .torrent (100 Mio au plus, comme le moteur).
async fn download_torrent(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|err| anyhow!("fichier .torrent injoignable : {}", err.without_url()))?;
    if !response.status().is_success() {
        bail!("le serveur du .torrent a répondu HTTP {}", response.status());
    }
    let mut body = Vec::new();
    while let Some(chunk) =
        response.chunk().await.map_err(|err| anyhow!("lecture du .torrent interrompue : {}", err.without_url()))?
    {
        if body.len() + chunk.len() > librqbit::MAX_TORRENT_FILE_BYTES {
            bail!("fichier .torrent trop gros");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str) -> rss::FeedItem {
        rss::FeedItem { title: title.into(), link: format!("magnet:?xt=urn:btih:{id}"), id: id.into() }
    }

    #[test]
    fn first_read_downloads_nothing_then_only_new_matching_episodes() {
        let mut seen = FeedSeen::default();
        // flux tel qu'il est à l'abonnement (le plus récent en tête)
        let backlog = vec![item("2", "Série S01E02 1080p"), item("1", "Série S01E01 1080p")];
        assert!(select_new(&mut seen, &backlog, "série 1080p", "", true).is_empty(), "rien de l'historique");

        let mut feed =
            vec![item("5", "Série S01E03 1080p"), item("4", "Série S01E03 720p"), item("3", "Autre S05E01 1080p")];
        feed.extend(backlog);
        let chosen = select_new(&mut seen, &feed, "série 1080p", "", true);
        let titles: Vec<&str> = chosen.iter().map(|(i, _)| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Série S01E03 1080p"], "filtre et doublon de qualité");

        // relu plus tard : plus rien de nouveau
        assert!(select_new(&mut seen, &feed, "série 1080p", "", true).is_empty());
    }

    #[test]
    fn same_episode_in_two_qualities_is_taken_once() {
        let mut seen = FeedSeen { initialized: true, ..Default::default() };
        let feed = vec![item("b", "Série S02E05 720p"), item("a", "Série S02E05 1080p")];
        let chosen = select_new(&mut seen, &feed, "série", "", true);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].1.as_deref(), Some("série s02e05"));
        // doublons autorisés si l'option est coupée
        let mut seen = FeedSeen { initialized: true, ..Default::default() };
        assert_eq!(select_new(&mut seen, &feed, "série", "", false).len(), 2);
    }

    #[test]
    fn memory_is_bounded() {
        let mut list = VecDeque::new();
        for i in 0..(MEMORY + 10) {
            FeedSeen::remember(&mut list, i.to_string());
        }
        assert_eq!(list.len(), MEMORY);
        assert_eq!(list.front().unwrap(), "10");
    }

    /// Faux serveur de flux en bac à sable (127.0.0.1), jamais un vrai site.
    async fn fake_feed(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/rss",
            axum::routing::get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/rss")
    }

    #[tokio::test]
    async fn feed_is_fetched_and_hostile_answers_are_refused() {
        let client = reqwest::Client::new();
        let good = "<rss><channel><item><title>A S01E01</title><link>magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a</link></item></channel></rss>";
        let items = fetch(&client, &fake_feed(good.as_bytes().to_vec()).await).await.unwrap();
        assert_eq!(items.len(), 1);

        // réponse sans fin (6 Mio) : coupée à 5 Mio au lieu de remplir la mémoire
        let huge = fetch(&client, &fake_feed(vec![b' '; 6 * 1024 * 1024]).await).await.unwrap_err();
        assert!(format!("{huge}").contains("trop gros"), "{huge}");

        // adresses qui ne sont pas du web
        for url in ["file:///C:/Windows/win.ini", "ftp://site.example/rss", "javascript:alert(1)", "pas une adresse"] {
            assert!(fetch(&client, url).await.is_err(), "{url}");
        }
    }

    /// Résolveur de test : un nom public qui pointe vers ce PC (« DNS rebinding »).
    struct Rebinding;
    impl reqwest::dns::Resolve for Rebinding {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async {
                let addrs: reqwest::dns::Addrs =
                    Box::new(std::iter::once(std::net::SocketAddr::from(([127, 0, 0, 1], 0))));
                Ok(addrs)
            })
        }
    }

    #[tokio::test]
    async fn torrent_of_a_public_name_resolving_to_the_local_network_is_refused() {
        let local = fake_feed(b"d4:infod4:name1:xee".to_vec()).await;
        let port = url::Url::parse(&local).unwrap().port().unwrap();
        let rebound = format!("http://tracker.example:{port}/rss");
        // client ordinaire : la requête atteint bien le serveur local (le danger existe)
        let plain = reqwest::Client::builder().dns_resolver(Arc::new(Rebinding)).build().unwrap();
        assert!(download_torrent(&plain, &rebound).await.is_ok());
        // client limité à Internet (celui des flux) : refusée après résolution
        let public =
            librqbit::protect_webseed_client(reqwest::Client::builder(), Arc::new(Rebinding), false).build().unwrap();
        assert!(download_torrent(&public, &rebound).await.is_err());
        // (une adresse locale écrite telle quelle est refusée avant, par `rss::link_allowed`)
    }
}
