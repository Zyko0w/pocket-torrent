//! Création de la session librqbit selon les réglages et la liaison réseau (libre, VPN, coupé).

use super::*;

pub fn limits_config(limits: &SpeedLimits) -> LimitsConfig {
    LimitsConfig {
        download_bps: NonZeroU32::new(limits.down_kbps.saturating_mul(1024)),
        upload_bps: NonZeroU32::new(limits.up_kbps.saturating_mul(1024)),
    }
}

/// Par où le moteur a le droit de communiquer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Sans VPN : réseau normal.
    Free,
    /// Tout passe par cette carte réseau (VPN), rien ailleurs.
    Vpn { adapter: String, ipv6: bool },
    /// VPN exigé mais coupé : aucune communication possible.
    Offline,
    /// Tout par ce proxy SOCKS5 (TCP seulement : ni DHT, ni UDP, ni connexion entrante).
    Proxy(ProxyUrl),
}

/// Adresse d'un proxy SOCKS5, identifiants compris : jamais écrite telle quelle dans le journal.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyUrl {
    host: String,
    port: u16,
    credentials: Option<(String, String)>,
}

impl ProxyUrl {
    pub fn new(host: &str, port: u16, user: &str, password: &str) -> Self {
        let credentials = (!user.is_empty() || !password.is_empty()).then(|| (user.to_owned(), password.to_owned()));
        Self { host: host.to_owned(), port, credentials }
    }

    /// `socks5h` : noms résolus par le proxy (aucune requête DNS ne sort d'ici) ; `socks5` pour les
    /// pairs, toujours désignés par adresse IP.
    pub fn url(&self, scheme: &str) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let credentials = self.credentials.as_ref().map_or_else(String::new, |(user, password)| {
            format!(
                "{}:{}@",
                utf8_percent_encode(user, NON_ALPHANUMERIC),
                utf8_percent_encode(password, NON_ALPHANUMERIC)
            )
        });
        format!("{scheme}://{credentials}{host}:{}", self.port)
    }
}

impl std::fmt::Debug for ProxyUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "socks5://{}{}:{}", if self.credentials.is_some() { "***@" } else { "" }, self.host, self.port)
    }
}

/// TCP seul par défaut ; µTP en plus si activé dans les réglages (voir `Settings::utp_enabled`).
pub(super) fn listener_mode(settings: &Settings) -> ListenerMode {
    if settings.utp_enabled { ListenerMode::TcpAndUtp } else { ListenerMode::TcpOnly }
}

pub(super) fn encryption_mode(settings: &Settings) -> EncryptionMode {
    match settings.privacy.encryption {
        EncryptionLevel::Off => EncryptionMode::Disabled,
        EncryptionLevel::Prefer => EncryptionMode::Prefer,
        EncryptionLevel::Require => EncryptionMode::Require,
    }
}

pub(super) async fn create_session(
    settings: &Settings,
    data_dir: &Path,
    binding: &Binding,
    network: &Network,
) -> anyhow::Result<Arc<Session>> {
    std::fs::create_dir_all(&settings.download_dir).ok();
    let hide_announces = settings.privacy.hide_announces || settings.privacy.hardened_mode;
    // annonces discrètes : seuls les trackers HTTPS (les annonces HTTP et UDP sont lisibles en clair)
    librqbit_tracker_comms::set_https_trackers_only(hide_announces);

    let mut options = match binding {
        Binding::Free => free_options(settings, data_dir),
        Binding::Vpn { adapter, ipv6 } => SessionOptions {
            bind_device_name: Some(adapter.clone()),
            // sans IPv6 dans le tunnel, rien ne doit tenter l'IPv6 hors du tunnel
            ipv4_only: !ipv6,
            // la découverte locale et l'UPnP parlent au réseau local, pas au VPN
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: listener_mode(settings),
                listen_addr: SocketAddr::from((Ipv6Addr::UNSPECIFIED, settings.torrent_port)),
                enable_upnp_port_forwarding: false,
                ..Default::default()
            }),
            ..free_options(settings, data_dir)
        },
        Binding::Proxy(proxy) => SessionOptions {
            // ni DHT ni µTP (UDP), ni écoute (le proxy n'apporte pas de connexions entrantes), ni
            // découverte locale ou UPnP (réseau local, hors proxy)
            dht: None,
            listen: None,
            disable_local_service_discovery: true,
            disable_udp_trackers: true,
            connect: Some(ConnectionOptions {
                proxy_url: Some(proxy.url("socks5")),
                enable_tcp: false,
                peer_opts: None,
                encryption: encryption_mode(settings),
            }),
            ..free_options(settings, data_dir)
        },
        Binding::Offline => SessionOptions {
            dht: None,
            listen: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            // proxy injoignable : toute tentative de connexion échoue au lieu de sortir sans VPN
            connect: Some(ConnectionOptions {
                proxy_url: Some("socks5://127.0.0.1:9".into()),
                enable_tcp: false,
                peer_opts: None,
                encryption: EncryptionMode::Require,
            }),
            ..free_options(settings, data_dir)
        },
    };
    if hide_announces {
        // la DHT et la découverte locale échangent des messages BitTorrent non chiffrés
        options.dht = None;
        options.disable_local_service_discovery = true;
        // une source web en http:// serait lisible en clair, comme un tracker HTTP
        options.webseeds_https_only = true;
    }
    if settings.privacy.hardened_mode {
        if let Some(listen) = options.listen.as_mut() {
            // l'UPnP parle au routeur en clair et annonce un port ouvert vers l'extérieur
            listen.enable_upnp_port_forwarding = false;
        }
        // fait passer ce PC pour un client courant plutôt que Pocket Torrent
        options.peer_id = Some(generate_azereus_style(*b"UT", (3, 5, 5, 0)));
        options.client_name_and_version = Some("uTorrent/3.5.5".to_owned());
        if let Some(connect) = options.connect.as_mut() {
            connect.peer_opts.get_or_insert_with(Default::default).disable_yourip = true;
        }
    }
    options.blocklist_url = local_blocklist(settings, data_dir, network).await;
    options.webseed_reqwest_client = Some(network.webseed_client(options.webseeds_https_only));
    options.reqwest_client = Some(network.http.clone());
    Session::new_with_opts(settings.download_dir.clone(), options)
        .await
        .context("impossible de démarrer le moteur torrent")
}

/// Ce dont le moteur dépend dans les réglages, en valeurs effectives : il n'est redémarré
/// (téléchargements coupés quelques secondes) que si ceci change. Ainsi, cocher « mode discret »
/// quand la confidentialité renforcée l'inclut déjà, ou modifier la source d'une liste de
/// blocage désactivée, ne redémarre rien. Même règle côté interface : `src/engineConfig.ts`.
#[derive(Debug, PartialEq, Eq)]
pub struct EngineConfig {
    port: u16,
    utp: bool,
    peer_limit: u32,
    upload_slots: u32,
    encryption: EncryptionLevel,
    discreet: bool,
    hardened: bool,
    blocklist: Option<String>,
}

impl EngineConfig {
    pub fn of(settings: &Settings) -> Self {
        Self {
            port: settings.torrent_port,
            utp: settings.utp_enabled,
            peer_limit: settings.max_peers_per_torrent,
            upload_slots: settings.upload_slots,
            encryption: settings.privacy.encryption,
            discreet: settings.privacy.hide_announces || settings.privacy.hardened_mode,
            hardened: settings.privacy.hardened_mode,
            blocklist: blocklist_url(settings),
        }
    }
}

/// Chemin local ou URL http(s)/file pour `SessionOptions::blocklist_url` (§9.7). Efficacité
/// faible en pratique (voir l'avertissement affiché dans les réglages), gardée à la demande
/// du plan d'upgrade.
pub(super) fn blocklist_url(settings: &Settings) -> Option<String> {
    if !settings.blocklist.enabled {
        return None;
    }
    let source = settings.blocklist.source.trim();
    if source.is_empty() {
        return None;
    }
    if source.starts_with("http://") || source.starts_with("https://") || source.starts_with("file://") {
        return Some(source.to_owned());
    }
    url::Url::from_file_path(source).ok().map(|u| u.to_string())
}

/// Copie locale d'une liste de blocage téléchargée, et l'adresse dont elle vient.
const BLOCKLIST_FILE: &str = "liste-de-blocage";
const BLOCKLIST_SOURCE_FILE: &str = "liste-de-blocage.source";
/// Taille maximale d'une liste téléchargée (les listes P2P courantes, compressées, font
/// quelques Mio ; décompressées, quelques dizaines).
const MAX_BLOCKLIST_BYTES: usize = 128 * 1024 * 1024;

/// Liste de blocage pour le moteur, toujours sous forme de fichier local. Une liste en ligne est
/// téléchargée ici, par le client réseau de l'appli (VPN, DNS chiffré ; VPN coupé : rien ne sort),
/// et gardée : si elle est injoignable, la dernière copie de la même adresse sert, sinon le moteur
/// démarre sans liste. Avant, le moteur la téléchargeait lui-même avec un client par défaut (hors
/// du VPN, même VPN coupé) et refusait de démarrer si elle était injoignable.
pub(super) async fn local_blocklist(settings: &Settings, data_dir: &Path, network: &Network) -> Option<String> {
    let source = blocklist_url(settings)?;
    if !(source.starts_with("http://") || source.starts_with("https://")) {
        // fichier local déplacé ou effacé : moteur démarré sans liste plutôt que pas du tout
        let exists = url::Url::parse(&source).ok().and_then(|u| u.to_file_path().ok()).is_some_and(|p| p.is_file());
        if !exists {
            tracing::warn!("liste de blocage introuvable ({source}), moteur démarré sans elle");
            return None;
        }
        return Some(source);
    }
    let cache = data_dir.join(BLOCKLIST_FILE);
    let marker = data_dir.join(BLOCKLIST_SOURCE_FILE);
    match download_blocklist(&network.http, &source).await {
        Ok(bytes) => {
            let saved = crate::store::write_atomic(&cache, &bytes)
                .and_then(|()| crate::store::write_atomic(&marker, source.as_bytes()));
            if let Err(err) = saved {
                tracing::warn!("liste de blocage non enregistrée : {err:#}");
                return None;
            }
        }
        Err(err) => {
            let same_source = std::fs::read_to_string(&marker).is_ok_and(|s| s == source);
            if !(same_source && cache.is_file()) {
                tracing::warn!("liste de blocage injoignable, moteur démarré sans elle : {err:#}");
                return None;
            }
            tracing::warn!("liste de blocage injoignable, dernière copie utilisée : {err:#}");
        }
    }
    url::Url::from_file_path(&cache).ok().map(|u| u.to_string())
}

async fn download_blocklist(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|err| anyhow!("{}", err.without_url()))?;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| anyhow!("{}", err.without_url()))? {
        if body.len() + chunk.len() > MAX_BLOCKLIST_BYTES {
            bail!("liste trop grosse (plus de 128 Mio)");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn free_options(settings: &Settings, data_dir: &Path) -> SessionOptions {
    SessionOptions {
        // voir `local_blocklist`, appelée par `create_session`
        blocklist_url: None,
        peer_limit: (settings.max_peers_per_torrent > 0).then_some(settings.max_peers_per_torrent as usize),
        upload_slots: (settings.upload_slots > 0).then_some(settings.upload_slots as usize),
        upnp_description: Some("Pocket Torrent".into()),
        // reprise automatique des téléchargements au prochain lancement
        persistence: Some(SessionPersistenceConfig::Json { folder: Some(data_dir.join("session")) }),
        fastresume: true,
        dht: Some(DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                config_filename: Some(data_dir.join("dht.json")),
                dump_interval: None,
            }),
            ..Default::default()
        }),
        listen: Some(ListenerOptions {
            // µTP désactivé par défaut : le module µTP de librqbit reste jeune. Réglable (§9.3).
            mode: listener_mode(settings),
            listen_addr: SocketAddr::from((Ipv6Addr::UNSPECIFIED, settings.torrent_port)),
            enable_upnp_port_forwarding: true,
            ..Default::default()
        }),
        connect: Some(ConnectionOptions { encryption: encryption_mode(settings), ..Default::default() }),
        trackers: EXTRA_TRACKERS.iter().filter_map(|t| t.parse().ok()).collect(),
        ratelimits: limits_config(&settings.limits),
        ..Default::default()
    }
}

#[cfg(test)]
mod proxy_url_tests {
    use super::ProxyUrl;

    #[test]
    fn proxy_urls_are_built_safely() {
        let proxy = ProxyUrl::new("2001:db8::1", 1080, "moi@site", "p@ss:w/rd%");
        assert_eq!(proxy.url("socks5h"), "socks5h://moi%40site:p%40ss%3Aw%2Frd%25@[2001:db8::1]:1080");
        assert_eq!(ProxyUrl::new("10.0.0.1", 1080, "", "").url("socks5"), "socks5://10.0.0.1:1080");
        // journal : jamais le mot de passe ni l'identifiant
        assert_eq!(format!("{proxy:?}"), "socks5://***@2001:db8::1:1080");
    }
}
