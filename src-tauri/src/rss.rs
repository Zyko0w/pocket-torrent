//! Flux RSS / Atom de torrents (séries) : lecture du flux, filtres, reconnaissance des épisodes.
//! Fonctions pures, sans réseau : le téléchargement du flux et l'ajout sont dans `manager::feeds`.
//!
//! Compatible avec les flux courants (RSS 2.0, Atom) et ceux de Jackett / Prowlarr (Torznab :
//! lien magnet dans `<torznab:attr name="magneturl">`). Le lecteur XML ne charge rien d'extérieur
//! (pas de DTD ni d'entité externe) : seules les entités standard (`&amp;`…) sont traduites.

use quick_xml::Reader;
use quick_xml::events::Event;

/// Nombre maximal d'éléments lus dans un flux (les plus récents sont en tête).
pub const MAX_ITEMS: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedItem {
    pub title: String,
    /// Lien magnet ou adresse http(s) d'un .torrent.
    pub link: String,
    /// Identifiant stable de l'élément (guid, id Atom, sinon le lien).
    pub id: String,
}

#[derive(Default)]
struct Draft {
    title: String,
    link: String,
    guid: String,
    enclosure: String,
    magnet: String,
}

impl Draft {
    /// Lien à télécharger : magnet d'abord, puis pièce jointe .torrent, puis lien de l'élément s'il
    /// désigne un magnet ou un .torrent (souvent, c'est la page web du torrent : jamais téléchargée).
    fn finish(self) -> Option<FeedItem> {
        let magnet = |l: &str| l.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("magnet:?"));
        let web = |l: &str| {
            let lower = l.to_ascii_lowercase();
            lower.starts_with("https://") || lower.starts_with("http://")
        };
        let torrent_file =
            |l: &str| web(l) && url::Url::parse(l).is_ok_and(|u| u.path().to_ascii_lowercase().ends_with(".torrent"));
        let (magnet_link, enclosure, link) = (self.magnet.trim(), self.enclosure.trim(), self.link.trim());
        let link = [
            (magnet(magnet_link)).then_some(magnet_link),
            (magnet(enclosure) || web(enclosure)).then_some(enclosure),
            (magnet(link) || torrent_file(link)).then_some(link),
        ]
        .into_iter()
        .flatten()
        .find(|l| l.len() <= 8000)?
        .to_owned();
        let title: String = self.title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        if title.is_empty() {
            return None;
        }
        let id = if self.guid.trim().is_empty() { link.clone() } else { self.guid.trim().chars().take(500).collect() };
        Some(FeedItem { title, link, id })
    }
}

/// Lit les éléments d'un flux RSS ou Atom. Erreur si ce n'est pas du XML lisible.
pub fn parse_feed(xml: &str) -> anyhow::Result<Vec<FeedItem>> {
    let mut reader = Reader::from_str(xml);
    let mut items = Vec::new();
    let mut draft: Option<Draft> = None;
    let mut field: Option<&'static str> = None;
    let mut text = String::new();

    loop {
        let event = reader.read_event().map_err(|e| anyhow::anyhow!("flux illisible (XML invalide) : {e}"))?;
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                let name = local.as_ref();
                match name {
                    "item" | "entry" => draft = Some(Draft::default()),
                    _ if draft.is_some() => {
                        let d = draft.as_mut().unwrap();
                        let attr = |key: &str| {
                            e.attributes()
                                .flatten()
                                .find(|a| a.key.local_name().as_ref() == key)
                                .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                                .map(|v| v.into_owned())
                        };
                        match name {
                            "title" => field = Some("title"),
                            "guid" | "id" => field = Some("guid"),
                            "link" => {
                                // Atom : <link href="…"/> ; RSS : <link>…</link>
                                if let Some(href) = attr("href") {
                                    let rel_ok = attr("rel").is_none_or(|r| r == "alternate" || r == "enclosure");
                                    let torrent = attr("type").is_some_and(|t| t == "application/x-bittorrent");
                                    if torrent || (rel_ok && d.link.is_empty()) {
                                        d.link = href;
                                    }
                                } else {
                                    field = Some("link");
                                }
                            }
                            "enclosure" => {
                                let torrent = attr("type").is_none_or(|t| t == "application/x-bittorrent");
                                if let (true, Some(url)) = (torrent, attr("url")) {
                                    d.enclosure = url;
                                }
                            }
                            // Torznab (Jackett, Prowlarr) : <torznab:attr name="magneturl" value="magnet:…"/>
                            "attr" if attr("name").as_deref() == Some("magneturl") => {
                                if let Some(value) = attr("value") {
                                    d.magnet = value;
                                }
                            }
                            // certains flux publics : <torrent:magnetURI>magnet:…</torrent:magnetURI>
                            "magnetURI" => field = Some("magnet"),
                            _ => {}
                        }
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                if field.is_some() {
                    text.push_str(&e.into_inner());
                }
            }
            Event::CData(e) => {
                if field.is_some() {
                    text.push_str(&e.into_inner());
                }
            }
            Event::GeneralRef(e) => {
                if field.is_some() {
                    let raw = format!("&{};", e.into_inner());
                    text.push_str(&quick_xml::escape::unescape(&raw).map(|c| c.into_owned()).unwrap_or_default());
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                let name = local.as_ref();
                if matches!(name, "item" | "entry") {
                    if let Some(item) = draft.take().and_then(Draft::finish) {
                        items.push(item);
                        if items.len() >= MAX_ITEMS {
                            break;
                        }
                    }
                } else if let (Some(f), Some(d)) = (field, draft.as_mut()) {
                    let value = std::mem::take(&mut text);
                    match f {
                        "title" => d.title = value,
                        "guid" => d.guid = value,
                        "link" => d.link = value,
                        "magnet" => d.magnet = value,
                        _ => {}
                    }
                    field = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(items)
}

/// Lien d'un élément que l'appli peut suivre. Un flux hostile pourrait viser le réseau local
/// (« http://192.168.1.1/… » : la box, un NAS) : un lien web vers une adresse locale n'est suivi
/// que s'il est sur le même serveur que le flux lui-même (cas de Jackett / Prowlarr sur ce PC).
/// Les liens magnet ne déclenchent aucune requête web : toujours acceptés.
pub fn link_allowed(feed_url: &str, link: &str) -> bool {
    if link.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("magnet:?")) {
        return true;
    }
    let Ok(target) = url::Url::parse(link) else { return false };
    if !matches!(target.scheme(), "http" | "https") {
        return false;
    }
    let same_server = url::Url::parse(feed_url).is_ok_and(|feed| {
        feed.host_str() == target.host_str() && feed.port_or_known_default() == target.port_or_known_default()
    });
    same_server || !is_local_host(&target)
}

/// Lien web d'un élément à télécharger par le client limité à Internet (`Network::public_client`) :
/// tous, sauf ceux du même serveur qu'un flux lui-même local (Jackett / Prowlarr sur ce PC).
/// L'adresse écrite ne suffit pas : un nom public peut se résoudre vers la box (« DNS
/// rebinding ») ou rediriger vers elle ; le client limité contrôle l'adresse réellement jointe.
pub fn needs_public_only(feed_url: &str, link: &str) -> bool {
    let (Ok(feed), Ok(target)) = (url::Url::parse(feed_url), url::Url::parse(link)) else { return true };
    let same_server =
        feed.host_str() == target.host_str() && feed.port_or_known_default() == target.port_or_known_default();
    !(same_server && is_local_host(&feed))
}

/// Adresse de ce PC ou du réseau local (écrite telle quelle dans le lien).
pub(crate) fn is_local_host(url: &url::Url) -> bool {
    use std::net::IpAddr;
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost"
                || name.ends_with(".localhost")
                || name.ends_with(".local")
                || name.ends_with(".lan")
                || name.ends_with(".home.arpa")
        }
        Some(url::Host::Ipv4(ip)) => {
            let ip = IpAddr::V4(ip);
            ip.is_loopback()
                || ip.is_unspecified()
                || matches!(ip, IpAddr::V4(v4) if v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_private() || v4.is_loopback() || v4.is_link_local())
        }
        None => true,
    }
}

/// Mots d'un filtre (séparés par des espaces ou des virgules), en minuscules.
fn words(filter: &str) -> Vec<String> {
    filter.split([' ', ',']).map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect()
}

/// Filtre d'un flux : mots (tous ceux de `include`, aucun de `exclude`) ou expressions
/// régulières (`include` doit trouver quelque chose, `exclude` rien), sans tenir compte des
/// majuscules. Préparé une fois par vérification du flux.
pub enum Filter {
    Words { include: String, exclude: String },
    Regex { include: Option<regex::Regex>, exclude: Option<regex::Regex> },
}

/// Taille maximale d'une expression compilée : le moteur `regex` garantit un temps linéaire,
/// cette limite borne en plus la mémoire (une expression comme `(a{1000}){1000}` est refusée).
const MAX_REGEX_BYTES: usize = 256 * 1024;

impl Filter {
    pub fn new(include: &str, exclude: &str, regex: bool) -> Result<Self, String> {
        if !regex {
            return Ok(Filter::Words { include: include.to_owned(), exclude: exclude.to_owned() });
        }
        let compile = |pattern: &str, field: &str| -> Result<Option<regex::Regex>, String> {
            if pattern.trim().is_empty() {
                return Ok(None);
            }
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(MAX_REGEX_BYTES)
                .dfa_size_limit(MAX_REGEX_BYTES)
                .build()
                .map(Some)
                .map_err(|err| match err {
                    regex::Error::CompiledTooBig(_) => format!("expression « {field} » trop complexe"),
                    _ => format!("expression « {field} » invalide"),
                })
        };
        Ok(Filter::Regex {
            include: compile(include, "doit contenir")?,
            exclude: compile(exclude, "ne doit pas contenir")?,
        })
    }

    pub fn matches(&self, title: &str) -> bool {
        match self {
            Filter::Words { include, exclude } => matches(title, include, exclude),
            Filter::Regex { include, exclude } => {
                include.as_ref().is_none_or(|r| r.is_match(title))
                    && !exclude.as_ref().is_some_and(|r| r.is_match(title))
            }
        }
    }
}

/// Le titre contient-il tous les mots de `include` et aucun de `exclude` ? (sans tenir compte
/// des majuscules ; les points et tirets des titres comptent comme des espaces)
pub fn matches(title: &str, include: &str, exclude: &str) -> bool {
    let haystack = format!(" {} ", title.to_lowercase().replace(['.', '_', '-', '[', ']', '(', ')'], " "));
    let has = |word: &str| haystack.contains(word);
    words(include).iter().all(|w| has(w)) && !words(exclude).iter().any(|w| has(w))
}

/// Épisode reconnu dans un titre : « nom de la série + s01e02 » (aussi « 1x02 »), pour ne pas
/// télécharger deux fois le même épisode publié en plusieurs qualités. `None` si pas d'épisode.
pub fn episode_key(title: &str) -> Option<String> {
    let normalized: String = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        let parsed = parse_sxxexx(token).or_else(|| parse_nxnn(token));
        if let Some((season, episode)) = parsed {
            let show = tokens[..i].join(" ");
            if show.is_empty() {
                return None;
            }
            return Some(format!("{show} s{season:02}e{episode:02}"));
        }
    }
    None
}

fn parse_sxxexx(token: &str) -> Option<(u32, u32)> {
    let rest = token.strip_prefix('s')?;
    let (season, episode) = rest.split_once('e')?;
    let episode: String = episode.chars().take_while(char::is_ascii_digit).collect();
    if season.is_empty()
        || season.len() > 3
        || !season.chars().all(|c| c.is_ascii_digit())
        || episode.is_empty()
        || episode.len() > 4
    {
        return None;
    }
    Some((season.parse().ok()?, episode.parse().ok()?))
}

fn parse_nxnn(token: &str) -> Option<(u32, u32)> {
    let (season, episode) = token.split_once('x')?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit());
    (digits(season) && digits(episode) && episode.len() >= 2)
        .then(|| Some((season.parse().ok()?, episode.parse().ok()?)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
<channel><title>Séries</title>
  <item>
    <title>Ma Série S02E05 1080p WEB &amp; VOSTFR</title>
    <link>https://tracker.example/details/1</link>
    <guid>abc-1</guid>
    <enclosure url="https://tracker.example/dl/1.torrent" type="application/x-bittorrent" length="1"/>
  </item>
  <item>
    <title><![CDATA[Ma Série S02E05 720p]]></title>
    <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&amp;dn=x"/>
    <guid>abc-2</guid>
  </item>
  <item><title>Sans lien</title></item>
  <item><title>Lien piégé</title><link>file:///C:/Windows/win.ini</link></item>
</channel></rss>"#;

    #[test]
    fn rss_items_prefer_magnets_then_torrent_files() {
        let items = parse_feed(RSS).unwrap();
        assert_eq!(items.len(), 2, "les éléments sans lien utilisable sont ignorés : {items:?}");
        // lien de l'élément vers une page web : jamais pris pour un téléchargement
        assert!(parse_feed("<rss><channel><item><title>A S01E01</title><link>https://site.example/page</link></item></channel></rss>").unwrap().is_empty());
        assert_eq!(items[0].title, "Ma Série S02E05 1080p WEB & VOSTFR");
        assert_eq!(items[0].link, "https://tracker.example/dl/1.torrent");
        assert_eq!(items[0].id, "abc-1");
        assert_eq!(items[1].title, "Ma Série S02E05 720p");
        assert_eq!(items[1].link, "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a&dn=x");
    }

    #[test]
    fn atom_entries_are_read() {
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>Autre S01E01</title><id>urn:1</id>
            <link rel="alternate" href="https://site.example/1"/><link rel="enclosure" type="application/x-bittorrent" href="https://site.example/1.torrent"/></entry></feed>"#;
        let items = parse_feed(atom).unwrap();
        assert_eq!(
            items,
            vec![FeedItem {
                title: "Autre S01E01".into(),
                link: "https://site.example/1.torrent".into(),
                id: "urn:1".into()
            }]
        );
    }

    #[test]
    fn hostile_feeds_are_harmless() {
        // entité externe (XXE) : jamais chargée ; entité inconnue : ignorée
        let xxe = r#"<?xml version="1.0"?><!DOCTYPE r [<!ENTITY x SYSTEM "file:///C:/Windows/win.ini">]>
            <rss><channel><item><title>A &x; B S01E01</title><link>magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a</link></item></channel></rss>"#;
        let items = parse_feed(xxe).unwrap();
        assert_eq!(items.len(), 1);
        assert!(!items[0].title.contains("fonts") && !items[0].title.contains("[extensions]"), "{}", items[0].title);
        assert!(parse_feed("<rss><channel><item><title>x</title>").is_ok() || parse_feed("<rss><item>").is_err());
        assert!(parse_feed("pas du xml <<<").is_err() || parse_feed("pas du xml <<<").unwrap().is_empty());
        // nombre d'éléments plafonné
        let many = format!(
            "<rss><channel>{}</channel></rss>",
            "<item><title>t</title><link>https://a.example/x.torrent</link></item>".repeat(1000)
        );
        assert_eq!(parse_feed(&many).unwrap().len(), MAX_ITEMS);
    }

    #[test]
    fn hostile_links_to_the_local_network_are_refused() {
        let public_feed = "https://tracker.example/rss";
        for link in [
            "http://192.168.1.1/cgi-bin/reboot.torrent",
            "http://10.0.0.5/x.torrent",
            "http://127.0.0.1:9117/dl/x.torrent",
            "http://localhost/x.torrent",
            "http://nas.local/x.torrent",
            "http://[::1]/x.torrent",
            "http://[fe80::1]/x.torrent",
            "http://[::ffff:192.168.0.1]/x.torrent",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/x.torrent",
            "file:///C:/Windows/win.ini",
        ] {
            assert!(!link_allowed(public_feed, link), "{link}");
        }
        assert!(link_allowed(public_feed, "https://tracker.example/dl/1.torrent"));
        assert!(link_allowed(public_feed, "https://autre-site.example/1.torrent"));
        assert!(link_allowed(public_feed, "magnet:?xt=urn:btih:e8812d0e9b24a9a82a1376d50f2b786ee6ea797a"));
        // Jackett sur ce PC : ses propres liens restent utilisables, pas ceux vers un autre port local
        let jackett = "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/api?apikey=x";
        assert!(link_allowed(jackett, "http://127.0.0.1:9117/dl/indexer/?jackett_apikey=x&path=y"));
        assert!(!link_allowed(jackett, "http://127.0.0.1:8080/admin"));
    }

    #[test]
    fn only_links_of_a_local_feed_skip_the_internet_only_client() {
        // flux public : ses liens, même sur son propre serveur, passent par le client limité
        // (le nom public pourrait se résoudre vers la box)
        assert!(needs_public_only("https://tracker.example/rss", "https://tracker.example/dl/1.torrent"));
        assert!(needs_public_only("https://tracker.example/rss", "https://autre.example/1.torrent"));
        // Jackett sur ce PC : ses propres liens seulement
        let jackett = "http://127.0.0.1:9117/api?apikey=x";
        assert!(!needs_public_only(jackett, "http://127.0.0.1:9117/dl/x.torrent"));
        assert!(needs_public_only(jackett, "https://tracker.example/1.torrent"));
        assert!(needs_public_only("pas une adresse", "https://tracker.example/1.torrent"));
    }

    #[test]
    fn filters_need_every_word_and_no_excluded_one() {
        let title = "Ma.Série.S02E05.1080p.WEB-DL.VOSTFR";
        assert!(matches(title, "série 1080p", ""));
        assert!(matches(title, "SÉRIE, vostfr", "720p"));
        assert!(!matches(title, "série 2160p", ""));
        assert!(!matches(title, "série", "web"));
        assert!(matches(title, "", ""), "sans filtre : tout");
    }

    #[test]
    fn regex_filters_need_a_match_and_no_excluded_one() {
        let filter = Filter::new(r"s0[12]e\d+.*(1080p|2160p)", r"\bhdr\b|\bcam\b", true).unwrap();
        assert!(filter.matches("Ma.Serie.S01E03.1080p.WEB"));
        assert!(filter.matches("MA SERIE s02e10 2160P"), "sans tenir compte des majuscules");
        assert!(!filter.matches("Ma.Serie.S03E01.1080p"), "saison 3");
        assert!(!filter.matches("Ma.Serie.S01E03.2160p.HDR"), "exclu");
        assert!(!filter.matches("Ma.Serie.S01E03.720p"));
        // champs vides : tout passe
        assert!(Filter::new("", " ", true).unwrap().matches("n'importe quoi"));
        // mode mots inchangé : les caractères spéciaux y sont des lettres comme les autres
        assert!(Filter::new("s01e03", "", false).unwrap().matches("Ma.Serie.S01E03"));
    }

    #[test]
    fn hostile_regexes_are_refused_not_slow() {
        assert!(Filter::new("(série", "", true).is_err(), "parenthèse ouverte");
        let err = Filter::new("(a{1000}){1000}", "", true).err().unwrap();
        assert!(err.contains("trop complexe"), "{err}");
        // motif classique de retour arrière catastrophique : temps linéaire avec ce moteur
        let filter = Filter::new("(a+)+$", "", true).unwrap();
        let started = std::time::Instant::now();
        assert!(!filter.matches(&format!("{}!", "a".repeat(100_000))));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn episodes_are_recognised_whatever_the_quality() {
        let a = episode_key("Ma.Série.S02E05.1080p.WEB").unwrap();
        let b = episode_key("Ma Série - S02E05 - 720p").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "ma série s02e05");
        assert_eq!(episode_key("Ma Série 2x05 HDTV").unwrap(), "ma série s02e05");
        assert_ne!(episode_key("Ma Série S02E06").unwrap(), a);
        assert_eq!(episode_key("Film 2026 1080p"), None);
        assert_eq!(episode_key("S01E01 sans nom"), None);
        assert_eq!(episode_key("1920x1080 Démo"), None, "une résolution n'est pas un épisode");
    }
}
