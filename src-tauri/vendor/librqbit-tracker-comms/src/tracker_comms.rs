use std::collections::HashSet;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::net::SocketAddrV6;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::bail;
use backon::ExponentialBuilder;
use backon::Retryable;
use futures::FutureExt;
use futures::StreamExt;
use futures::future::Either;
use futures::stream::BoxStream;
use futures::stream::FuturesUnordered;
use tracing::Instrument;
use tracing::debug;
use tracing::debug_span;
use tracing::trace;
use tracing::trace_span;
use url::Url;

use crate::tracker_comms_http;
use crate::tracker_comms_udp;
use crate::tracker_comms_udp::UdpTrackerClient;
use librqbit_core::hash_id::Id20;

/// Torrent Cloud : n'utiliser que les trackers HTTPS (les annonces HTTP et UDP circulent en clair
/// et révèlent au fournisseur d'accès qu'il s'agit de BitTorrent).
static HTTPS_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_https_trackers_only(enabled: bool) {
    HTTPS_ONLY.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub struct TrackerComms {
    info_hash: Id20,
    peer_id: Id20,
    stats: Box<dyn TorrentStatsProvider>,
    force_tracker_interval: Option<Duration>,
    tx: Sender,
    // This MUST be set as trackers don't work with 0 port.
    /// Relu à chaque annonce : le port externe UPnP peut arriver après le démarrage.
    announce_port: std::sync::Arc<std::sync::atomic::AtomicU16>,
    reqwest_client: reqwest::Client,
    key: u32,
    /// Pocket Torrent : état de chaque tracker, retiré du registre quand ce flux s'arrête.
    status: crate::status::StatusOwner,
}

#[derive(Default)]
pub enum TrackerCommsStatsState {
    #[default]
    None,
    Initializing,
    Paused,
    Live,
}

#[derive(Default)]
pub struct TrackerCommsStats {
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub torrent_state: TrackerCommsStatsState,
}

impl TrackerCommsStats {
    pub fn get_left_to_download_bytes(&self) -> u64 {
        let total = self.total_bytes;
        let down = self.downloaded_bytes;
        if total >= down {
            return total - down;
        }
        0
    }

    pub fn is_completed(&self) -> bool {
        self.downloaded_bytes >= self.total_bytes
    }
}

pub trait TorrentStatsProvider: Send + Sync {
    fn get(&self) -> TrackerCommsStats;
}

impl TorrentStatsProvider for () {
    fn get(&self) -> TrackerCommsStats {
        Default::default()
    }
}

type Sender = tokio::sync::mpsc::Sender<SocketAddr>;

/// Taille maximale d'une réponse de tracker HTTP (quelques Kio en pratique, même avec des
/// centaines de pairs).
const MAX_HTTP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

enum SupportedTracker {
    Udp(Url),
    Http(Url),
}

impl std::fmt::Debug for SupportedTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupportedTracker::Udp(u) => std::fmt::Display::fmt(u, f),
            SupportedTracker::Http(u) => std::fmt::Display::fmt(u, f),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum UdpTrackerResolveResult {
    One(SocketAddr),
    Two(SocketAddrV4, SocketAddrV6),
}

async fn udp_tracker_to_socket_addrs(
    host: url::Host<&str>,
    port: u16,
) -> anyhow::Result<UdpTrackerResolveResult> {
    let res = match host {
        url::Host::Domain(name) => {
            // Use the first IPv4 and the first IPv6 addresses only.

            let mut v4: Option<SocketAddrV4> = None;
            let mut v6: Option<SocketAddrV6> = None;
            // Torrent Cloud : résolveur remplaçable (DNS chiffré par le VPN)
            for addr in librqbit_dualstack_sockets::resolve::lookup_host(name, port)
                .await
                .with_context(|| format!("error looking up hostname {name}"))?
            {
                match (v4, v6, addr) {
                    (None, _, SocketAddr::V4(addr)) => v4 = Some(addr),
                    (_, None, SocketAddr::V6(addr)) => v6 = Some(addr),
                    _ => continue,
                }
            }
            let res = match (v4, v6) {
                (Some(v4), Some(v6)) => UdpTrackerResolveResult::Two(v4, v6),
                (Some(v4), None) => UdpTrackerResolveResult::One(v4.into()),
                (None, Some(v6)) => UdpTrackerResolveResult::One(v6.into()),
                _ => anyhow::bail!("zero addresses returned looking up {name}"),
            };
            trace!(?res, "resolved");
            res
        }
        url::Host::Ipv4(addr) => UdpTrackerResolveResult::One((addr, port).into()),
        url::Host::Ipv6(addr) => UdpTrackerResolveResult::One((addr, port).into()),
    };
    Ok(res)
}

impl TrackerComms {
    /// Port annoncé aux trackers (4240 si inconnu, comme avant).
    fn announce_port(&self) -> u16 {
        match self
            .announce_port
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            0 => 4240,
            p => p,
        }
    }

    // TODO: fix too many args
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        info_hash: Id20,
        peer_id: Id20,
        trackers: HashSet<Url>,
        stats: Box<dyn TorrentStatsProvider>,
        force_interval: Option<Duration>,
        announce_port: std::sync::Arc<std::sync::atomic::AtomicU16>,
        reqwest_client: reqwest::Client,
        udp_client: UdpTrackerClient,
    ) -> Option<BoxStream<'static, SocketAddr>> {
        let https_only = HTTPS_ONLY.load(std::sync::atomic::Ordering::Relaxed);
        let status = crate::status::StatusOwner::new(info_hash);
        let trackers = trackers
            .into_iter()
            .filter(|t| {
                let keep = !https_only || t.scheme() == "https";
                if !keep {
                    status.skipped(t.as_str(), "ignoré : mode discret (trackers HTTPS seulement)");
                }
                keep
            })
            .filter_map(|t| match t.scheme() {
                "http" | "https" => Some(SupportedTracker::Http(t)),
                "udp" => Some(SupportedTracker::Udp(t)),
                _ => {
                    debug!("unsupported tracker URL: {}", t);
                    status.skipped(t.as_str(), "type de tracker non pris en charge");
                    None
                }
            })
            .collect::<Vec<_>>();
        if trackers.is_empty() {
            debug!(?info_hash, "trackers list is empty");
            return None;
        }

        tracing::trace!(?trackers);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<SocketAddr>(16);

        let s = async_stream::stream! {
            use futures::StreamExt;
            let comms = Arc::new(Self {
                info_hash,
                peer_id,
                stats,
                force_tracker_interval: force_interval,
                tx,
                announce_port,
                reqwest_client,
                key: rand::random(),
                status,
            });
            let mut futures = FuturesUnordered::new();
            for tracker in trackers {
                futures.push(comms.add_tracker(tracker, &udp_client))
            }
            while !(futures.is_empty()) {
                tokio::select! {
                    addr = rx.recv() => {
                        if let Some(addr) = addr {
                            yield addr;
                        }
                    }
                    e = futures.next(), if !futures.is_empty() => {
                        if let Some(Err(e)) = e {
                            debug!("error: {e}");
                        }
                    }
                }
            }
        };

        Some(s.boxed())
    }

    fn add_tracker(
        &self,
        url: SupportedTracker,
        client: &UdpTrackerClient,
    ) -> Either<
        impl std::future::Future<Output = anyhow::Result<()>> + '_ + Send,
        impl std::future::Future<Output = anyhow::Result<()>> + '_ + Send,
    > {
        let info_hash = self.info_hash;
        match url {
            SupportedTracker::Udp(url) => {
                let span = debug_span!(parent: None, "udp_tracker", tracker = %url, info_hash = ?info_hash);
                self.task_single_tracker_monitor_udp(url, client.clone())
                    .instrument(span)
                    .right_future()
            }
            SupportedTracker::Http(url) => {
                let span = debug_span!(
                    parent: None,
                    "http_tracker",
                    tracker = %url,
                    info_hash = ?info_hash
                );
                self.task_single_tracker_monitor_http(url)
                    .instrument(span)
                    .left_future()
            }
        }
    }

    async fn task_single_tracker_monitor_http(&self, tracker_url: Url) -> anyhow::Result<()> {
        trace!(url=%tracker_url, "starting monitor");
        let mut event = Some(tracker_comms_http::TrackerRequestEvent::Started);
        self.status.connecting(tracker_url.as_str());

        loop {
            let interval = (|| self.tracker_one_request_http(&tracker_url, event))
                .retry(
                    ExponentialBuilder::new()
                        .without_max_times()
                        .with_jitter()
                        .with_factor(2.)
                        .with_min_delay(Duration::from_secs(10))
                        .with_max_delay(Duration::from_secs(600)),
                )
                .notify(|err, retry_in| {
                    debug!(?retry_in, "error calling tracker: {err:#}");
                    self.status.failure(tracker_url.as_str(), &format!("{err:#}"), Some(retry_in));
                })
                .await
                .context("this shouldn't fail")?;

            event = None;
            let interval = self.force_tracker_interval.unwrap_or(interval);
            debug!("sleeping for {:?} after calling tracker", interval);
            tokio::time::sleep(interval).await;
        }
    }

    async fn tracker_one_request_http(
        &self,
        tracker_url: &Url,
        event: Option<tracker_comms_http::TrackerRequestEvent>,
    ) -> anyhow::Result<Duration> {
        let stats = self.stats.get();
        let request = tracker_comms_http::TrackerRequest {
            info_hash: &self.info_hash,
            peer_id: &self.peer_id,
            port: self.announce_port(),
            uploaded: stats.uploaded_bytes,
            downloaded: stats.downloaded_bytes,
            left: stats.get_left_to_download_bytes(),
            compact: true,
            no_peer_id: false,
            event,
            ip: None,
            numwant: None,
            key: Some(self.key),
            trackerid: None,
        };

        let mut url = tracker_url.clone();

        let mut queries = request.as_querystring();
        if let Some(url_query) = url.query() {
            queries.push_str(&format!("&{}", url_query));
        }
        url.set_query(Some(&queries));

        let mut response: reqwest::Response = self.reqwest_client.get(url).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("tracker responded with {:?}", response.status());
        }
        // Pocket Torrent : réponse lue par morceaux et plafonnée ; `bytes()` lisait tout en
        // mémoire, un tracker hostile pouvait envoyer un flux sans fin et arrêter l'application.
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > MAX_HTTP_RESPONSE_BYTES {
                bail!("tracker response too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        if let Ok((error, _)) =
            bencode::from_bytes_with_rest::<tracker_comms_http::TrackerError>(&bytes)
        {
            anyhow::bail!(
                "tracker returned failure. Failure reason: {}",
                error.failure_reason
            )
        };
        let response = bencode::from_bytes_with_rest::<tracker_comms_http::TrackerResponse>(&bytes)
            .map_err(|e| {
                tracing::trace!("error deserializing TrackerResponse: {e:#}");
                e.into_kind()
            })?
            .0;

        let mut peers = 0;
        for peer in response.iter_peers() {
            self.tx.send(peer).await?;
            peers += 1;
        }
        let interval = Duration::from_secs(response.min_interval.unwrap_or(response.interval));
        let swarm = match (response.complete, response.incomplete) {
            (Some(seeders), Some(leechers)) => Some(crate::status::Swarm { seeders, leechers }),
            _ => None,
        };
        self.status.success(
            tracker_url.as_str(),
            peers,
            swarm,
            self.force_tracker_interval.unwrap_or(interval),
        );
        Ok(interval)
    }

    async fn task_single_tracker_monitor_udp(
        &self,
        url: Url,
        client: UdpTrackerClient,
    ) -> anyhow::Result<()> {
        if url.scheme() != "udp" {
            bail!("expected UDP scheme in {}", url);
        }
        let (host, port) = (
            url.host().context("missing host")?,
            url.port().context("missing port")?,
        );

        let mut sleep_interval: Option<Duration> = None;
        let mut prev_addrs: Option<UdpTrackerResolveResult> = None;
        self.status.connecting(url.as_str());
        loop {
            if let Some(i) = sleep_interval {
                trace!(interval=?sleep_interval, "sleeping");
                tokio::time::sleep(i).await;
            }

            // This should retry forever until the addrs are resolved.
            let addrs = (async || {
                udp_tracker_to_socket_addrs(host.clone(), port)
                    .instrument(trace_span!("resolve", ?host))
                    .await
                    .or_else(|err| prev_addrs.ok_or(err))
            })
            .retry(
                ExponentialBuilder::new()
                    .without_max_times()
                    .with_max_delay(Duration::from_secs(60))
                    .with_jitter(),
            )
            .notify(|err, retry| {
                debug!(retry_in=?retry, "error resolving tracker: {err:#}");
                self.status.failure(url.as_str(), &format!("{err:#}"), Some(retry));
            })
            .await
            .context("this shouldn't happen: failed resolving tracker addrs")?;

            prev_addrs = Some(addrs);

            match addrs {
                UdpTrackerResolveResult::One(addr) => {
                    match self
                        .tracker_one_request_udp(addr, &client)
                        .instrument(trace_span!("udp request", ?addr))
                        .await
                    {
                        Ok((sleep, peers, swarm)) => {
                            self.status.success(url.as_str(), peers, Some(swarm), sleep);
                            sleep_interval = Some(sleep)
                        }
                        Err(err) => {
                            let retry = sleep_interval.unwrap_or(Duration::from_secs(60));
                            self.status.failure(url.as_str(), &format!("{err:#}"), Some(retry));
                            sleep_interval = Some(retry)
                        }
                    }
                }
                UdpTrackerResolveResult::Two(v4, v6) => {
                    let (r4, r6) = tokio::join!(
                        self.tracker_one_request_udp(v4.into(), &client)
                            .instrument(trace_span!("udp request", addr=?v4)),
                        self.tracker_one_request_udp(v6.into(), &client)
                            .instrument(trace_span!("udp request", addr=?v6))
                    );
                    let peers = r4.as_ref().map_or(0, |r| r.1) + r6.as_ref().map_or(0, |r| r.1);
                    // même essaim vu en IPv4 et en IPv6 : le plus grand des deux comptes
                    let swarm = [&r4, &r6]
                        .into_iter()
                        .filter_map(|r| r.as_ref().ok().map(|r| r.2))
                        .reduce(|a, b| crate::status::Swarm {
                            seeders: a.seeders.max(b.seeders),
                            leechers: a.leechers.max(b.leechers),
                        });
                    let error = match (&r4, &r6) {
                        (Err(e), Err(_)) => Some(format!("{e:#}")),
                        _ => None,
                    };
                    sleep_interval = Some(
                        r4.or(r6)
                            .ok()
                            .map(|r| r.0)
                            .or(sleep_interval)
                            .unwrap_or(Duration::from_secs(60)),
                    );
                    let next = sleep_interval.unwrap_or(Duration::from_secs(60));
                    match error {
                        Some(error) => self.status.failure(url.as_str(), &error, Some(next)),
                        None => self.status.success(url.as_str(), peers, swarm, next),
                    }
                }
            }
        }
    }

    async fn tracker_one_request_udp(
        &self,
        addr: SocketAddr,
        client: &UdpTrackerClient,
    ) -> anyhow::Result<(Duration, usize, crate::status::Swarm)> {
        use tracker_comms_udp::*;

        let stats = self.stats.get();
        let request = AnnounceFields {
            info_hash: self.info_hash,
            peer_id: self.peer_id,
            downloaded: stats.downloaded_bytes,
            left: stats.get_left_to_download_bytes(),
            uploaded: stats.uploaded_bytes,
            event: match stats.torrent_state {
                TrackerCommsStatsState::None => EVENT_NONE,
                TrackerCommsStatsState::Initializing => EVENT_STARTED,
                TrackerCommsStatsState::Paused => EVENT_STOPPED,
                TrackerCommsStatsState::Live => {
                    if stats.is_completed() {
                        EVENT_COMPLETED
                    } else {
                        EVENT_STARTED
                    }
                }
            },
            key: self.key,
            port: self.announce_port(),
        };

        match client.announce(addr, request).await {
            Ok(response) => {
                trace!(len = response.addrs.len(), "received announce response");
                let peers = response.addrs.len();
                for addr in response.addrs {
                    self.tx.send(addr).await.context("rx closed")?;
                }
                let sleep = response.interval.max(5);
                let sleep = Duration::from_secs(sleep as u64);
                let swarm = crate::status::Swarm {
                    seeders: response.seeders.into(),
                    leechers: response.leechers.into(),
                };
                Ok((sleep, peers, swarm))
            }
            Err(e) => {
                debug!(?addr, "error reading announce response: {e:#}");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Faux tracker HTTP en bac à sable (127.0.0.1) : répond `body` en boucle `repeat` fois.
    async fn fake_tracker(body: &'static [u8], repeat: usize) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = [0u8; 4096];
                    let _ = socket.read(&mut request).await;
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").await;
                    for _ in 0..repeat {
                        if socket.write_all(body).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Url::parse(&format!("http://{addr}/announce")).unwrap()
    }

    fn comms(info_hash: Id20) -> (TrackerComms, tokio::sync::mpsc::Receiver<SocketAddr>) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let comms = TrackerComms {
            info_hash,
            peer_id: Id20::new([2; 20]),
            stats: Box::new(()),
            force_tracker_interval: None,
            tx,
            announce_port: Arc::new(std::sync::atomic::AtomicU16::new(6881)),
            reqwest_client: reqwest::Client::new(),
            key: 0,
            status: crate::status::StatusOwner::new(info_hash),
        };
        (comms, rx)
    }

    #[tokio::test]
    async fn http_announce_reports_the_swarm() {
        let url = fake_tracker(b"d8:completei40e10:incompletei3e8:intervali1800e5:peers6:\x7f\x00\x00\x01\x1a\xe1e", 1).await;
        let hash = Id20::new([3; 20]);
        let (comms, mut rx) = comms(hash);
        comms.tracker_one_request_http(&url, None).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), "127.0.0.1:6881".parse().unwrap());
        let status = crate::status::tracker_statuses(&hash);
        assert_eq!(status[0].swarm, Some(crate::status::Swarm { seeders: 40, leechers: 3 }));
    }

    #[tokio::test]
    async fn endless_http_answer_is_cut_instead_of_filling_memory() {
        // 64 Mio envoyés (au-delà du plafond de 4 Mio) : la réponse est abandonnée
        let url = fake_tracker(&[b'x'; 64 * 1024], 1024).await;
        let (comms, _rx) = comms(Id20::new([4; 20]));
        let err = comms.tracker_one_request_http(&url, None).await.unwrap_err();
        assert!(format!("{err:#}").contains("too large"), "{err:#}");
    }
}
