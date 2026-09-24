//! Sources web (BEP 19, « GetRight »), ajoutées pour Torrent Cloud : le .torrent peut donner des
//! adresses HTTP où les mêmes fichiers sont hébergés (clé `url-list` : distributions Linux,
//! archive.org…). Chaque source se comporte comme un pair qui aurait toutes les pièces :
//! elle réserve une pièce dans le même sélecteur que les pairs, la télécharge par requêtes
//! HTTP de plages d'octets (une par fichier touché), puis passe par exactement le même circuit
//! d'écriture et de vérification d'empreinte (`write_chunk_and_verify`).
//!
//! Le client HTTP est dédié aux sources web (`SessionOptions::webseed_reqwest_client`, construit
//! avec `protect_webseed_client`) : mêmes VPN, DNS chiffré et certificats que la session, mais
//! les adresses privées sont retirées après la résolution DNS et chaque redirection est
//! contrôlée. Les limites de vitesse s'appliquent. En cas d'échec, la pièce est rendue à la file
//! et la source attend de plus en plus longtemps avant de réessayer.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, bail};
use buffers::ByteBuf;
use librqbit_core::lengths::ValidPieceIndex;
use peer_binary_protocol::Piece;
use tracing::{debug, trace, warn};
use url::Url;

use super::{TorrentStateLive, write_chunk_and_verify};
use crate::piece_tracker::{AcquireRequest, AcquireResult};
use crate::torrent_state::live::peer::stats::atomic::PeerCountersAtomic;

/// Téléchargements simultanés par source web.
const WORKERS_PER_SEED: u16 = 2;
/// Au plus ce nombre de sources web par torrent.
const MAX_SEEDS: usize = 8;
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// Une pièce est gardée en mémoire le temps de la télécharger : au-delà, pas de source web
/// (un torrent piégé annonçant des pièces de plusieurs Go ferait sinon saturer la mémoire).
const MAX_WEBSEED_PIECE: u32 = 32 * 1024 * 1024;

/// Au plus ce nombre de redirections HTTP par requête (chacune contrôlée comme l'adresse d'origine).
const MAX_REDIRECTS: usize = 3;

/// Adresse joignable sur Internet : ni boucle locale, ni réseau privé, ni adresse automatique,
/// ni réseau de l'opérateur (CGNAT), ni multicast.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                // 100.64.0.0/10 (réseau de l'opérateur, CGNAT)
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 64))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xFE00) == 0xFC00 // adresses locales uniques fc00::/7
                || (first & 0xFFC0) == 0xFE80) // lien local fe80::/10
        }
    }
}

/// Adresse de source web acceptable : http(s), et jamais le réseau local (boucle locale,
/// réseaux privés, adresses automatiques, `localhost`) sauf autorisation explicite. Même
/// protection que libtorrent contre les torrents qui visent la box ou le réseau local.
/// Ne voit que l'adresse écrite : un nom qui se résout vers le réseau local est arrêté par
/// `PublicOnlyResolver`.
pub(crate) fn is_allowed_webseed(url: &Url, https_only: bool, allow_local: bool) -> bool {
    let scheme_ok = match url.scheme() {
        "https" => true,
        "http" => !https_only,
        _ => false,
    };
    if !scheme_ok {
        return false;
    }
    if allow_local {
        return true;
    }
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d != "localhost"
                && !d.ends_with(".localhost")
                && !d.ends_with(".local")
                && !d.ends_with(".lan")
        }
        Some(url::Host::Ipv4(ip)) => is_public_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_public_ip(IpAddr::V6(ip)),
        None => false,
    }
}

/// Résolveur DNS qui retire les adresses non publiques de la réponse (protection contre le
/// « DNS rebinding » : un nom public qui pointe vers la box ou le réseau local). reqwest se
/// connecte uniquement aux adresses renvoyées ici : le contrôle porte sur l'adresse réellement
/// utilisée, pas sur une résolution faite à part.
pub struct PublicOnlyResolver(pub Arc<dyn reqwest::dns::Resolve>);

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let inner = self.0.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let public: Vec<SocketAddr> = inner
                .resolve(name)
                .await?
                .filter(|a| is_public_ip(a.ip()))
                .collect();
            if public.is_empty() {
                return Err(format!("{host} resolves to a local network address").into());
            }
            let addrs: reqwest::dns::Addrs = Box::new(public.into_iter());
            Ok(addrs)
        })
    }
}

/// Redirections des sources web : au plus `MAX_REDIRECTS`, chacune soumise aux mêmes règles
/// que l'adresse d'origine (pas de réseau local, pas de HTTP en clair si `https_only`).
fn webseed_redirect_policy(https_only: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if !is_allowed_webseed(attempt.url(), https_only, false) {
            attempt.error("redirect to a refused address")
        } else {
            attempt.follow()
        }
    })
}

/// Complète le constructeur du client HTTP des sources web : résolveur filtrant (au-dessus de
/// `resolver`, typiquement le DNS chiffré de l'application) et redirections contrôlées. À
/// appeler en dernier : remplace un éventuel `dns_resolver` déjà posé.
pub fn protect_webseed_client(
    builder: reqwest::ClientBuilder,
    resolver: Arc<dyn reqwest::dns::Resolve>,
    https_only: bool,
) -> reqwest::ClientBuilder {
    builder
        .dns_resolver(Arc::new(PublicOnlyResolver(resolver)))
        .redirect(webseed_redirect_policy(https_only))
}

/// Encode un nom de fichier pour un segment d'URL (RFC 3986 : seuls les caractères « non
/// réservés » et les sous-délimiteurs restent tels quels).
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Adresses `url-list` d'un fichier .torrent (une chaîne ou une liste de chaînes).
pub(crate) fn url_list(torrent_bytes: &[u8]) -> Vec<Url> {
    use bencode::BencodeValue;
    let Ok(BencodeValue::Dict(dict)) = bencode::dyn_from_bytes::<ByteBuf>(torrent_bytes) else {
        return Vec::new();
    };
    let values = match dict.get(&ByteBuf(b"url-list")) {
        Some(BencodeValue::Bytes(one)) => vec![one],
        Some(BencodeValue::List(many)) => many
            .iter()
            .filter_map(|v| {
                if let BencodeValue::Bytes(b) = v {
                    Some(b)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|b| std::str::from_utf8(b.as_ref()).ok())
        .filter_map(|s| Url::parse(s.trim()).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .collect()
}

/// URL d'un fichier du torrent sur la source web (BEP 19). `name` : nom du torrent ; `path` :
/// composants du chemin du fichier (vide pour un torrent à un seul fichier).
pub(crate) fn file_url(
    base: &Url,
    name: &str,
    path: &[String],
    single_file: bool,
) -> anyhow::Result<Url> {
    let base = base.as_str();
    let mut url = base.to_owned();
    if single_file {
        // un seul fichier : l'URL le désigne directement, sauf si elle finit par « / »
        if base.ends_with('/') {
            url.push_str(&encode_segment(name));
        }
    } else {
        if !base.ends_with('/') {
            url.push('/');
        }
        url.push_str(&encode_segment(name));
        for part in path {
            url.push('/');
            url.push_str(&encode_segment(part));
        }
    }
    Url::parse(&url).context("bad web seed file URL")
}

/// Morceau de pièce à lire dans un fichier : (index du fichier, début dans le fichier,
/// longueur, position dans la pièce). `files` : (début dans le torrent, longueur) de chaque
/// fichier, dans l'ordre du torrent ; le premier fichier concerné est trouvé par dichotomie.
pub(crate) fn piece_slices(
    files: &[(u64, u64)],
    piece_offset: u64,
    piece_len: u64,
) -> Vec<(usize, u64, u64, u64)> {
    let piece_end = piece_offset + piece_len;
    let first = files.partition_point(|&(start, len)| start + len <= piece_offset);
    files[first..]
        .iter()
        .enumerate()
        .take_while(|(_, (start, _))| *start < piece_end)
        .filter_map(|(i, &(start, len))| {
            let from = start.max(piece_offset);
            let to = (start + len).min(piece_end);
            (from < to).then_some((first + i, from - start, to - from, from - piece_offset))
        })
        .collect()
}

/// Ce qu'il faut savoir des fichiers pour construire les requêtes, calculé une seule fois
/// par torrent (et non à chaque pièce : jusqu'à 100 000 fichiers).
pub(crate) struct WebSeedLayout {
    name: String,
    single_file: bool,
    /// (début dans le torrent, longueur) de chaque fichier.
    spans: Vec<(u64, u64)>,
    /// Chemin d'origine (non nettoyé pour Windows) de chaque fichier.
    paths: Vec<Vec<String>>,
    padding: Vec<bool>,
}

impl TorrentStateLive {
    /// Lance les sources web du torrent, s'il en a et qu'il reste à télécharger.
    pub(crate) fn spawn_webseeds(self: &Arc<Self>) {
        let Some(session) = self.shared.session.upgrade() else {
            return;
        };
        let all = url_list(&self.metadata.torrent_bytes);
        let seeds: Vec<Url> = all
            .iter()
            .filter(|u| {
                is_allowed_webseed(
                    u,
                    session.webseeds_https_only,
                    session.webseeds_allow_local_network,
                )
            })
            .take(MAX_SEEDS)
            .cloned()
            .collect();
        if seeds.len() < all.len() {
            debug!(
                ignored = all.len() - seeds.len(),
                "web seeds refused (local network, or plain HTTP in discreet mode)"
            );
        }
        if seeds.is_empty() || self.is_finished() {
            return;
        }
        if session.webseed_client().is_none() {
            warn!("no protected HTTP client for web seeds, web seeds disabled");
            return;
        }
        if self.lengths.default_piece_length() > MAX_WEBSEED_PIECE {
            warn!(
                piece_length = self.lengths.default_piece_length(),
                "pieces too large, web seeds disabled"
            );
            return;
        }
        debug!(count = seeds.len(), "starting web seeds");
        let info = &self.metadata.info;
        let layout = Arc::new(WebSeedLayout {
            name: info
                .info()
                .name
                .as_ref()
                .map(|n| String::from_utf8_lossy(n.as_ref()).into_owned())
                .unwrap_or_default(),
            single_file: info.info().files.is_none(),
            spans: self
                .metadata
                .file_infos
                .iter()
                .map(|f| (f.offset_in_torrent, f.len))
                .collect(),
            paths: info
                .iter_file_details()
                .map(|d| d.filename.to_vec())
                .collect(),
            padding: self
                .metadata
                .file_infos
                .iter()
                .map(|f| f.attrs.padding)
                .collect(),
        });
        // adresse fictive propre à chaque tâche (1, 2, 3…) : le sélecteur de pièces suit qui a quoi
        let mut next_port = 1u16;
        for url in seeds {
            for worker in 0..WORKERS_PER_SEED {
                let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, next_port));
                next_port += 1;
                self.spawn(
                    tracing::debug_span!(parent: self.shared.span.clone(), "webseed", url = %url, worker),
                    format!("[{}]webseed", self.shared.id),
                    self.clone().task_webseed(url.clone(), addr, layout.clone()),
                );
            }
        }
    }

    async fn task_webseed(
        self: Arc<Self>,
        base: Url,
        addr: SocketAddr,
        layout: Arc<WebSeedLayout>,
    ) -> crate::Result<()> {
        let counters = Arc::new(PeerCountersAtomic::default());
        let mut backoff = Duration::from_secs(10);
        loop {
            if self.is_finished() {
                return Ok(());
            }
            let new_pieces = self.new_pieces_notify.notified();
            let Some(piece) = self.reserve_piece_for_webseed(addr)? else {
                // rien à prendre pour l'instant (tout est réservé par les pairs, par exemple)
                let _ = tokio::time::timeout(Duration::from_secs(5), new_pieces).await;
                continue;
            };
            match self
                .download_piece_from_web(&base, addr, &counters, &layout, piece)
                .await
            {
                Ok(()) => backoff = Duration::from_secs(10),
                Err(e) => {
                    let released = self
                        .lock_write("webseed_release")
                        .get_pieces_mut()
                        .map(|p| p.release_piece_owned_by(piece, addr))
                        .unwrap_or(false);
                    if released {
                        self.new_pieces_notify.notify_waiters();
                    }
                    warn!(url = %base, "web seed failed, retrying in {backoff:?}: {e:#}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    fn reserve_piece_for_webseed(
        &self,
        addr: SocketAddr,
    ) -> crate::Result<Option<ValidPieceIndex>> {
        let mut g = self.lock_write("webseed_acquire");
        let super::TorrentStateLocked {
            pieces,
            file_priorities,
            ..
        } = &mut **g;
        let Some(pieces) = pieces.as_mut() else {
            return Ok(None);
        };
        let started: Vec<_> = pieces.chunks().iter_started_pieces().collect();
        let result = pieces.acquire_piece(AcquireRequest {
            peer: addr,
            peer_avg_time: None,
            priority_pieces: started
                .into_iter()
                .chain(self.streams.iter_next_pieces(&self.lengths)),
            file_priorities,
            file_infos: &self.metadata.file_infos,
            // une source web a toutes les pièces ; ordre d'origine (lecture continue côté HTTP)
            peer_has_piece: |_| true,
            can_steal: |_| false,
            piece_availability: None,
        });
        Ok(match result {
            AcquireResult::Reserved(piece) | AcquireResult::Stolen { piece, .. } => Some(piece),
            AcquireResult::NoneAvailable => None,
        })
    }

    async fn download_piece_from_web(
        self: &Arc<Self>,
        base: &Url,
        addr: SocketAddr,
        counters: &Arc<PeerCountersAtomic>,
        layout: &WebSeedLayout,
        piece: ValidPieceIndex,
    ) -> anyhow::Result<()> {
        let session = self.shared.session.upgrade().context("session closed")?;
        let client = session
            .webseed_client()
            .context("no HTTP client for web seeds")?
            .clone();
        let piece_len = u64::from(self.lengths.piece_length(piece));
        let piece_offset = self.lengths.piece_offset(piece);
        let to_usize = |v: u64| usize::try_from(v).context("offset too large");

        let mut data = vec![0u8; to_usize(piece_len)?];
        for (file, start, len, at) in piece_slices(&layout.spans, piece_offset, piece_len) {
            if layout.padding[file] {
                continue; // fichier de remplissage (BEP 47) : que des zéros, jamais servi
            }
            let path: &[String] = if layout.single_file {
                &[]
            } else {
                &layout.paths[file]
            };
            let url = file_url(base, &layout.name, path, layout.single_file)?;
            let mut remaining = len;
            while remaining > 0 {
                let size = remaining.min(16 * 1024);
                remaining -= size;
                if let Some(size) = NonZeroU32::new(u32::try_from(size)?) {
                    self.ratelimits.prepare_for_download(size).await?;
                    session.ratelimits.prepare_for_download(size).await?;
                }
            }
            let response = client
                .get(url.clone())
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", start, start + len - 1),
                )
                .send()
                .await
                .with_context(|| format!("GET {url}"))?;
            let status = response.status();
            // 206 = plage demandée ; 200 accepté seulement si c'est le fichier entier
            let whole_file = start == 0 && len == layout.spans[file].1;
            if status != reqwest::StatusCode::PARTIAL_CONTENT
                && !(status.is_success() && whole_file)
            {
                bail!("{url} answered {status}");
            }
            let mut response = response;
            let target = &mut data[to_usize(at)?..to_usize(at + len)?];
            let mut written = 0usize;
            while let Some(bytes) = response
                .chunk()
                .await
                .context("reading web seed response")?
            {
                let take = bytes.len().min(target.len() - written);
                target[written..written + take].copy_from_slice(&bytes[..take]);
                written += take;
                if written == target.len() {
                    break;
                }
            }
            if written != target.len() {
                bail!("{url}: got {written} bytes instead of {len}");
            }
        }

        // même circuit que les blocs reçus d'un pair (écriture, empreinte, annonce)
        let already = self
            .lock_read("webseed_partial")
            .get_pieces()
            .map(|p| p.chunks().downloaded_chunks_bitmask(piece))
            .unwrap_or_default();
        let state = self.clone();
        let counters = counters.clone();
        self.shared
            .spawner
            .block_in_place_with_semaphore(move || -> anyhow::Result<()> {
                for chunk in state.lengths.iter_chunk_infos(piece) {
                    if already
                        .get(chunk.chunk_index as usize)
                        .copied()
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let from = chunk.offset as usize;
                    let block = &data[from..from + chunk.size as usize];
                    let size = block.len() as u64;
                    state.stats.fetched_bytes.fetch_add(size, Ordering::Relaxed);
                    state
                        .session_stats
                        .counters
                        .fetched_bytes
                        .fetch_add(size, Ordering::Relaxed);
                    write_chunk_and_verify(
                        &state,
                        addr,
                        &counters,
                        &Piece::from_data(piece.get(), chunk.offset, block),
                        &chunk,
                        false,
                    )?;
                }
                Ok(())
            })
            .await?;
        trace!(piece = piece.get(), "piece downloaded from web seed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_bep19() {
        let base = Url::parse("https://mirror.example/pub/").unwrap();
        let single = file_url(&base, "ubuntu 24.iso", &[], true).unwrap();
        assert_eq!(
            single.as_str(),
            "https://mirror.example/pub/ubuntu%2024.iso"
        );
        let direct = Url::parse("https://mirror.example/file.iso").unwrap();
        assert_eq!(
            file_url(&direct, "ignored.iso", &[], true)
                .unwrap()
                .as_str(),
            "https://mirror.example/file.iso"
        );
        let multi = file_url(
            &Url::parse("https://a.example/t").unwrap(),
            "Mon album",
            &["CD 1".into(), "01 #intro.flac".into()],
            false,
        )
        .unwrap();
        assert_eq!(
            multi.as_str(),
            "https://a.example/t/Mon%20album/CD%201/01%20%23intro.flac"
        );
    }

    #[test]
    fn url_list_accepts_string_or_list() {
        let one = b"d4:infod4:name1:xe8:url-list22:https://a.example/x.ise";
        assert_eq!(url_list(one).len(), 1);
        let many = b"d4:infod4:name1:xe8:url-listl18:https://a.example/12:ftp://b/nope17:http://c.example/ee";
        let urls: Vec<String> = url_list(many).iter().map(|u| u.to_string()).collect();
        assert_eq!(
            urls,
            vec!["https://a.example/", "http://c.example/"],
            "ftp ignoré"
        );
        assert!(url_list(b"d4:infod4:name1:xee").is_empty());
    }

    #[test]
    fn local_network_and_plain_http_are_filtered() {
        let ok = |s: &str, https_only: bool| {
            is_allowed_webseed(&Url::parse(s).unwrap(), https_only, false)
        };
        assert!(ok("https://releases.ubuntu.com/24.04/", false));
        assert!(ok("http://archive.org/download/x/", false));
        assert!(
            !ok("http://archive.org/download/x/", true),
            "HTTP refusé en mode discret"
        );
        for local in [
            "http://192.168.1.254/reboot",
            "http://10.0.0.1/",
            "http://172.20.0.5/",
            "http://127.0.0.1:8080/",
            "http://169.254.1.1/",
            "http://100.70.1.2/",
            "http://localhost/",
            "http://box.lan/",
            "http://imprimante.local/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:192.168.1.1]/",
        ] {
            assert!(!ok(local, false), "{local} devrait être refusé");
        }
        assert!(
            is_allowed_webseed(&Url::parse("http://127.0.0.1:8080/").unwrap(), false, true),
            "autorisé pour les tests"
        );
        assert!(ok("http://8.8.8.8/", false));
        assert!(!ok("http://224.0.0.1/", false), "multicast");
        assert!(
            !ok("http://[::ffff:100.64.0.1]/", false),
            "CGNAT déguisé en IPv6"
        );
    }

    /// Résolveur de test : tous les noms pointent vers `ip`.
    struct FixedResolver(IpAddr);

    impl reqwest::dns::Resolve for FixedResolver {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let addrs: reqwest::dns::Addrs = Box::new(std::iter::once(SocketAddr::new(self.0, 0)));
            Box::pin(async move { Ok(addrs) })
        }
    }

    /// Mini serveur HTTP : répond à chaque connexion par une redirection vers `location`.
    async fn redirect_server(location: &'static str) -> SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    fn protected_client(resolve_to: IpAddr, https_only: bool) -> reqwest::Client {
        protect_webseed_client(
            reqwest::Client::builder().no_proxy(),
            Arc::new(FixedResolver(resolve_to)),
            https_only,
        )
        .build()
        .unwrap()
    }

    #[tokio::test]
    async fn dns_rebinding_to_local_network_is_refused() {
        let server = redirect_server("/").await;
        // nom public en apparence, qui se résout vers 127.0.0.1
        let url = format!("http://seed.example:{}/file", server.port());
        let err = protected_client(IpAddr::V4(Ipv4Addr::LOCALHOST), false)
            .get(&url)
            .send()
            .await
            .unwrap_err();
        assert!(
            format!("{:?}", err).contains("local network"),
            "erreur inattendue : {err:?}"
        );
    }

    #[tokio::test]
    async fn redirects_are_checked() {
        let client = protected_client(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), false);
        // l'adresse de départ est littérale (ce test seul peut la joindre) ; la redirection vers
        // la box est refusée avant toute connexion
        let to_box = redirect_server("http://192.168.1.254/reboot").await;
        let err = client
            .get(format!("http://{to_box}/"))
            .send()
            .await
            .unwrap_err();
        assert!(err.is_redirect(), "{err:?}");

        let to_name = redirect_server("http://localhost:1/").await;
        let err = client
            .get(format!("http://{to_name}/"))
            .send()
            .await
            .unwrap_err();
        assert!(err.is_redirect(), "{err:?}");

        // mode discret : pas de repli vers HTTP en clair, même vers une adresse publique
        let https_client = protected_client(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), true);
        let to_plain = redirect_server("http://8.8.8.8/").await;
        let err = https_client
            .get(format!("http://{to_plain}/"))
            .send()
            .await
            .unwrap_err();
        assert!(err.is_redirect(), "{err:?}");
    }

    #[test]
    fn piece_spanning_files_is_split() {
        // fichiers de 10, 5 et 20 octets ; pièce de 12 octets à partir de l'octet 8
        let files = [(0, 10), (10, 5), (15, 20)];
        assert_eq!(
            piece_slices(&files, 15, 20),
            vec![(2, 0, 20, 0)],
            "dernier fichier seul"
        );
        assert_eq!(
            piece_slices(&files, 10, 5),
            vec![(1, 0, 5, 0)],
            "fichier du milieu exact"
        );
        // fichier vide au milieu : ignoré
        assert_eq!(
            piece_slices(&[(0, 4), (4, 0), (4, 4)], 2, 4),
            vec![(0, 2, 2, 0), (2, 0, 2, 2)]
        );
        assert_eq!(
            piece_slices(&files, 8, 12),
            vec![(0, 8, 2, 0), (1, 0, 5, 2), (2, 0, 5, 7)]
        );
        assert_eq!(piece_slices(&files, 0, 10), vec![(0, 0, 10, 0)]);
    }
}
