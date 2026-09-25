//! DNS chiffré (DNS over HTTPS, RFC 8484) pour tout le trafic de téléchargement.
//!
//! Les noms des trackers, des nœuds DHT et des sites de téléchargement ne passent plus en clair par le DNS
//! du fournisseur d'accès. Avec un VPN, les requêtes partent de son adresse ; VPN coupé, tout échoue.
//! Les serveurs sont appelés par adresse IP : aucun nom à résoudre pour les joindre.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use parking_lot::Mutex;

/// Serveurs DoH joignables par IP (certificats valables pour ces adresses).
const SERVERS: &[&str] = &["https://1.1.1.1/dns-query", "https://9.9.9.9/dns-query", "https://8.8.8.8/dns-query"];
const TYPE_A: u16 = 1;
/// Noms gardés en cache au plus.
const MAX_CACHED_NAMES: usize = 4096;
const TYPE_AAAA: u16 = 28;

/// Autorités de certification intégrées (Mozilla) : pas de vérification de révocation en ligne par Windows,
/// qui partirait hors du VPN.
pub fn root_certificates() -> &'static [reqwest::Certificate] {
    static ROOTS: OnceLock<Vec<reqwest::Certificate>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|cert| reqwest::Certificate::from_der(cert.as_ref()).ok())
            .collect()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Sans VPN : DNS chiffré, et DNS du système en secours s'il est bloqué.
    Free,
    /// Par le VPN (adresse source imposée), sans secours.
    Vpn(IpAddr),
    /// VPN coupé : aucune résolution.
    Blocked,
}

pub struct Resolver {
    mode: Mode,
    client: Option<reqwest::Client>,
    cache: Mutex<HashMap<String, (Instant, Vec<IpAddr>)>>,
}

impl Resolver {
    pub fn new(mode: Mode) -> Arc<Self> {
        let client = match mode {
            Mode::Blocked => None,
            Mode::Free | Mode::Vpn(_) => reqwest::Client::builder()
                .local_address(match mode {
                    Mode::Vpn(ip) => Some(ip),
                    _ => None,
                })
                .tls_certs_only(root_certificates().iter().cloned())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(8))
                .build()
                .ok(),
        };
        Arc::new(Self { mode, client, cache: Mutex::new(HashMap::new()) })
    }

    pub async fn lookup(&self, host: &str) -> std::io::Result<Vec<IpAddr>> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if host == "localhost" {
            return Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)]);
        }
        if let Some((expires, ips)) = self.cache.lock().get(&host)
            && *expires > Instant::now()
        {
            return Ok(ips.clone());
        }

        let result = match &self.client {
            None => Err(anyhow::anyhow!("VPN coupé : résolution de noms bloquée")),
            Some(client) => self.query_servers(client, &host).await,
        };
        match result {
            Ok((ips, ttl)) => {
                let ttl = Duration::from_secs(u64::from(ttl).clamp(30, 600));
                let mut cache = self.cache.lock();
                if cache.len() >= MAX_CACHED_NAMES {
                    // noms expirés retirés ; s'il en reste trop, tout est oublié (rare : il faut
                    // des milliers de noms différents en dix minutes)
                    let now = Instant::now();
                    cache.retain(|_, (expires, _)| *expires > now);
                    if cache.len() >= MAX_CACHED_NAMES {
                        cache.clear();
                    }
                }
                cache.insert(host, (Instant::now() + ttl, ips.clone()));
                Ok(ips)
            }
            Err(err) if self.mode == Mode::Free => {
                tracing::debug!("DNS chiffré indisponible pour {host} ({err:#}), DNS du système utilisé");
                Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.map(|a| a.ip()).collect())
            }
            Err(err) => Err(std::io::Error::other(format!("{host} : {err:#}"))),
        }
    }

    async fn query_servers(&self, client: &reqwest::Client, host: &str) -> anyhow::Result<(Vec<IpAddr>, u32)> {
        let mut last_error = None;
        for server in SERVERS {
            let (v4, v6) = tokio::join!(query(client, server, host, TYPE_A), query(client, server, host, TYPE_AAAA));
            match (v4, v6) {
                (Err(err), Err(_)) => last_error = Some(err),
                (a, b) => {
                    let mut ips = Vec::new();
                    let mut ttl = u32::MAX;
                    for (list, t) in [a, b].into_iter().flatten() {
                        ips.extend(list);
                        ttl = ttl.min(t);
                    }
                    if ips.is_empty() {
                        bail!("{host} : aucune adresse");
                    }
                    return Ok((ips, ttl));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("aucun serveur DNS joignable")))
    }
}

/// Une requête DNS (format binaire) envoyée en HTTPS.
async fn query(client: &reqwest::Client, server: &str, host: &str, qtype: u16) -> anyhow::Result<(Vec<IpAddr>, u32)> {
    let response = client
        .post(server)
        .header("content-type", "application/dns-message")
        .header("accept", "application/dns-message")
        .body(encode_query(host, qtype)?)
        .send()
        .await
        .map_err(|e| anyhow::Error::from(e.without_url()))?;
    if !response.status().is_success() {
        bail!("serveur DNS : HTTP {}", response.status());
    }
    let body = response.bytes().await?;
    decode_answer(&body, qtype)
}

fn encode_query(host: &str, qtype: u16) -> anyhow::Result<Vec<u8>> {
    // identifiant 0 (recommandé par la RFC 8484), récursion demandée, une question
    let mut message = vec![0, 0, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            bail!("nom d'hôte invalide : {host}");
        }
        message.push(label.len() as u8);
        message.extend_from_slice(label.as_bytes());
    }
    message.push(0);
    message.extend_from_slice(&qtype.to_be_bytes());
    message.extend_from_slice(&1u16.to_be_bytes()); // classe IN
    Ok(message)
}

fn skip_name(data: &[u8], mut pos: usize) -> anyhow::Result<usize> {
    loop {
        let len = *data.get(pos).context("réponse DNS tronquée")?;
        if len & 0xC0 == 0xC0 {
            return Ok(pos + 2);
        }
        if len == 0 {
            return Ok(pos + 1);
        }
        pos += 1 + len as usize;
    }
}

fn read_u16(data: &[u8], pos: usize) -> anyhow::Result<u16> {
    Ok(u16::from_be_bytes([
        *data.get(pos).context("réponse DNS tronquée")?,
        *data.get(pos + 1).context("réponse DNS tronquée")?,
    ]))
}

fn decode_answer(data: &[u8], qtype: u16) -> anyhow::Result<(Vec<IpAddr>, u32)> {
    if data.len() < 12 {
        bail!("réponse DNS tronquée");
    }
    let rcode = data[3] & 0x0F;
    if rcode == 3 {
        return Ok((Vec::new(), 60)); // nom inexistant
    }
    if rcode != 0 {
        bail!("erreur du serveur DNS (code {rcode})");
    }
    let questions = read_u16(data, 4)?;
    let answers = read_u16(data, 6)?;
    let mut pos = 12;
    for _ in 0..questions {
        pos = skip_name(data, pos)? + 4;
    }
    let mut ips = Vec::new();
    let mut ttl = u32::MAX;
    for _ in 0..answers {
        pos = skip_name(data, pos)?;
        let rtype = read_u16(data, pos)?;
        let record_ttl = u32::from_be_bytes(data.get(pos + 4..pos + 8).context("réponse DNS tronquée")?.try_into()?);
        let rdlen = read_u16(data, pos + 8)? as usize;
        let rdata = data.get(pos + 10..pos + 10 + rdlen).context("réponse DNS tronquée")?;
        pos += 10 + rdlen;
        match (rtype, rdlen) {
            (TYPE_A, 4) if qtype == TYPE_A => {
                ips.push(IpAddr::V4(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3])));
                ttl = ttl.min(record_ttl);
            }
            (TYPE_AAAA, 16) if qtype == TYPE_AAAA => {
                let octets: [u8; 16] = rdata.try_into()?;
                ips.push(IpAddr::V6(Ipv6Addr::from(octets)));
                ttl = ttl.min(record_ttl);
            }
            _ => {} // CNAME et autres : les adresses finales suivent dans la réponse
        }
    }
    Ok((ips, if ttl == u32::MAX { 300 } else { ttl }))
}

/// Pour reqwest : résolution par le même résolveur.
pub struct ReqwestResolver(pub Arc<Resolver>);

impl reqwest::dns::Resolve for ReqwestResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = self.0.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let ips =
                resolver.lookup(&host).await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let addrs: reqwest::dns::Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// Installe le résolveur pour les trackers UDP et la DHT de librqbit.
pub fn install_global(resolver: Arc<Resolver>) {
    librqbit_dualstack_sockets::resolve::set_resolver(Some(Arc::new(move |host: String, port: u16| {
        let resolver = resolver.clone();
        Box::pin(async move {
            let ips = resolver.lookup(&host).await?;
            Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
        })
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes() {
        let query = encode_query("tracker.example.org", TYPE_A).unwrap();
        assert_eq!(&query[12..20], b"\x07tracker");
        // réponse : question recopiée + un enregistrement A 93.184.216.34 (TTL 120) avec pointeur de nom
        let mut answer = query.clone();
        answer[2] = 0x81;
        answer[3] = 0x80;
        answer[7] = 1;
        answer.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 120, 0, 4, 93, 184, 216, 34]);
        let (ips, ttl) = decode_answer(&answer, TYPE_A).unwrap();
        assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
        assert_eq!(ttl, 120);
    }
}
