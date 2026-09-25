mod mappings;
pub mod natpmp;

pub use mappings::ActiveMappings;

use anyhow::{Context, bail};
use bstr::BStr;
use futures::{StreamExt, TryFutureExt, stream::FuturesUnordered};
use librqbit_dualstack_sockets::{BindDevice, UdpSocket};
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use reqwest::Client;
use serde_derive::Deserialize;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tracing::{Instrument, Span, debug, debug_span, trace, warn};
use url::Url;

const SERVICE_TYPE_WAN_IP_CONNECTION: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";
const SSDP_MULTICAST_IP: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900));
pub const SSDP_SEARCH_WAN_IPCONNECTION_ST: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";
pub const SSDP_SEARCH_ROOT_ST: &str = "upnp:rootdevice";

/// Pocket Torrent : n'importe quel appareil du réseau local peut répondre à la recherche SSDP
/// (Wi-Fi public, appareil piraté). Limites : réponses de 1 Mio au plus, 10 s au plus, et
/// 32 appareils par recherche ; voir aussi `parse_upnp_discover_response` et
/// `get_wan_ip_control_urls` (adresses tenues à l'appareil qui a répondu).
const MAX_UPNP_RESPONSE_BYTES: usize = 1024 * 1024;
const UPNP_HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DEVICES_PER_SEARCH: usize = 32;

fn upnp_client() -> Client {
    Client::builder()
        .timeout(UPNP_HTTP_TIMEOUT)
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// Corps d'une réponse d'un appareil du réseau local, lu par morceaux et plafonné.
async fn read_capped(mut response: reqwest::Response) -> anyhow::Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_UPNP_RESPONSE_BYTES {
            bail!("UPnP response too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Même machine : adresse écrite dans `url` égale à `ip` (IPv4 éventuellement notée en IPv6).
fn url_is_on(url: &Url, ip: IpAddr) -> bool {
    let canonical = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    let host = match url.host() {
        Some(url::Host::Ipv4(v4)) => IpAddr::V4(v4),
        Some(url::Host::Ipv6(v6)) => IpAddr::V6(v6),
        _ => return false,
    };
    matches!(url.scheme(), "http" | "https") && canonical(host) == canonical(ip)
}

pub fn make_ssdp_search_request(kind: &str) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
            Host: 239.255.255.250:1900\r\n\
            Man: \"ssdp:discover\"\r\n\
            MX: 3\r\n\
            ST: {kind}\r\n\
            \r\n"
    )
}

pub fn get_local_ip_relative_to(
    local_dest: SocketAddr,
    interfaces: &[NetworkInterface],
) -> anyhow::Result<IpAddr> {
    fn masked_v4(ip: Ipv4Addr, mask: Ipv4Addr) -> u32 {
        ip.to_bits() & mask.to_bits()
    }

    fn masked_v6(ip: Ipv6Addr, mask: Ipv6Addr) -> u128 {
        ip.to_bits() & mask.to_bits()
    }

    for i in interfaces {
        for addr in i.addr.iter() {
            match (local_dest, addr.ip(), addr.netmask()) {
                // We are connecting to ourselves, return itself.
                (l, a, _) if l.ip() == a => return Ok(addr.ip()),
                // IPv4 masks match.
                (SocketAddr::V4(l), IpAddr::V4(a), Some(IpAddr::V4(m)))
                    if masked_v4(*l.ip(), m) == masked_v4(a, m) =>
                {
                    return Ok(addr.ip());
                }
                // Return IPv6 link-local addresses when source is link-local address and there's a scope_id set.
                (SocketAddr::V6(l), IpAddr::V6(a), _)
                    if l.ip().is_unicast_link_local() && l.scope_id() > 0 =>
                {
                    if a.is_unicast_link_local() && l.scope_id() == i.index {
                        return Ok(addr.ip());
                    }
                }
                // If V6 masks match, return.
                (SocketAddr::V6(l), IpAddr::V6(a), Some(IpAddr::V6(m)))
                    if masked_v6(*l.ip(), m) == masked_v6(a, m) =>
                {
                    return Ok(addr.ip());
                }
                // For IPv6 fallback to returning a random (first encountered) IPv6 address.
                (SocketAddr::V6(_), IpAddr::V6(_), None) => return Ok(addr.ip()),
                _ => continue,
            }
        }
    }
    bail!("couldn't find a local ip address")
}

/// Échec d'une redirection : code d'erreur UPnP renvoyé par la box (dans le corps de la
/// réponse HTTP 500), ou autre problème (réseau, réponse illisible).
#[derive(Debug)]
enum ForwardError {
    Upnp { code: u32, description: String },
    Other(anyhow::Error),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardError::Upnp { code, description } => {
                write!(f, "UPnP error {code} {description}")
            }
            ForwardError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

/// Codes qui veulent dire « prends un autre port externe » : 718 conflit (port déjà pris, ou
/// hors de la plage autorisée — cas des Freebox en IPv4 partagée), 716 caractère générique
/// refusé, 724 ports interne et externe doivent être égaux (on reste alors sur le port interne).
const RETRY_OTHER_EXTERNAL_PORT: [u32; 2] = [716, 718];
/// La box n'accepte que les redirections permanentes.
const ONLY_PERMANENT_LEASES: u32 = 725;

fn soap_value<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + xml[start..].find(&format!("</{tag}>"))?;
    Some(xml[start..end].trim())
}

#[allow(clippy::too_many_arguments)]
async fn forward_port(
    control_url: Url,
    local_ip: IpAddr,
    external_port: u16,
    internal_port: u16,
    protocol: &str,
    lease_duration: Duration,
    description: &str,
) -> Result<(), ForwardError> {
    let request_body = format!(
        r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
<u:AddPortMapping xmlns:u="{SERVICE_TYPE_WAN_IP_CONNECTION}">
<NewRemoteHost></NewRemoteHost>
<NewExternalPort>{external_port}</NewExternalPort>
<NewProtocol>{protocol}</NewProtocol>
<NewInternalPort>{internal_port}</NewInternalPort>
<NewInternalClient>{local_ip}</NewInternalClient>
<NewEnabled>1</NewEnabled>
<NewPortMappingDescription>{description}</NewPortMappingDescription>
<NewLeaseDuration>{}</NewLeaseDuration>
</u:AddPortMapping>
</s:Body>
</s:Envelope>"#,
        lease_duration.as_secs()
    );

    let response = upnp_client()
        .post(control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPAction",
            format!("\"{SERVICE_TYPE_WAN_IP_CONNECTION}#AddPortMapping\""),
        )
        .body(request_body)
        .send()
        .await
        .context("error sending")
        .map_err(ForwardError::Other)?;
    let status = response.status();
    let text = read_capped(response)
        .await
        .context("error reading response text")
        .map_err(ForwardError::Other)?;
    trace!(%status, text, protocol, external_port, "AddPortMapping response");
    if status.is_success() {
        return Ok(());
    }
    match soap_value(&text, "errorCode").and_then(|c| c.parse().ok()) {
        Some(code) => Err(ForwardError::Upnp {
            code,
            description: soap_value(&text, "errorDescription")
                .unwrap_or_default()
                .to_owned(),
        }),
        None => Err(ForwardError::Other(anyhow::anyhow!(
            "failed port forwarding: {status}"
        ))),
    }
}

/// Ports externes à essayer quand la box refuse le port interne (pseudo-aléatoires, jamais
/// sous 1024 ; pas besoin d'un vrai aléa, juste de ne pas toujours tomber au même endroit).
fn candidate_external_ports(seed: u16) -> impl Iterator<Item = u16> {
    let mut x = u64::from(seed) ^ 0x9E37_79B9_7F4A_7C15;
    std::iter::repeat_with(move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        1024 + (x % (65536 - 1024)) as u16
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RootDesc {
    #[serde(rename = "device")]
    pub devices: Vec<Device>,
}

#[derive(Default, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct DeviceList {
    #[serde(rename = "device")]
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Device {
    #[serde(rename = "deviceType")]
    pub device_type: String,
    #[serde(rename = "friendlyName", default)]
    pub friendly_name: String,
    #[serde(rename = "serviceList", default)]
    pub service_list: ServiceList,
    #[serde(rename = "deviceList", default)]
    pub device_list: DeviceList,
}

impl Device {
    pub fn iter_services(
        &self,
        parent: Span,
    ) -> Box<dyn Iterator<Item = (tracing::Span, &Service)> + '_> {
        let self_span = self.span(parent);
        let services = self.service_list.services.iter().map({
            let self_span = self_span.clone();
            move |s| (s.span(self_span.clone()), s)
        });
        Box::new(services.chain(self.device_list.devices.iter().flat_map({
            let self_span = self_span.clone();
            move |d| d.iter_services(self_span.clone())
        })))
    }

    pub fn span(&self, parent: tracing::Span) -> tracing::Span {
        debug_span!(parent: parent, "device", device = self.name())
    }
}

impl Device {
    pub fn name(&self) -> &str {
        if self.friendly_name.is_empty() {
            return &self.device_type;
        }
        &self.friendly_name
    }
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
pub struct ServiceList {
    #[serde(rename = "service", default)]
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Service {
    #[serde(rename = "serviceType")]
    pub service_type: String,
    #[serde(rename = "controlURL")]
    pub control_url: String,
    #[serde(rename = "SCPDURL")]
    pub scpd_url: String,
    #[serde(rename = "eventSubURL", default)]
    pub event_sub_url: Option<String>,
}

impl Service {
    pub fn span(&self, parent: tracing::Span) -> tracing::Span {
        debug_span!(parent: parent, "service", url = self.control_url)
    }
}

#[derive(Debug)]
struct UpnpEndpoint {
    discover_response: UpnpDiscoverResponse,
    data: RootDesc,
    nics: Vec<NetworkInterface>,
}

impl UpnpEndpoint {
    fn location(&self) -> &Url {
        &self.discover_response.location
    }

    fn span(&self) -> tracing::Span {
        debug_span!("upnp_endpoint", location = %self.location())
    }

    fn iter_services(&self) -> impl Iterator<Item = (tracing::Span, &Service)> + '_ {
        let self_span = self.span();
        self.data
            .devices
            .iter()
            .flat_map(move |d| d.iter_services(self_span.clone()))
    }

    fn my_local_ip(&self) -> anyhow::Result<IpAddr> {
        let received_from = self.discover_response.received_from;
        let local_ip = get_local_ip_relative_to(received_from, &self.nics)
            .with_context(|| format!("can't determine local IP relative to {received_from}"))?;
        Ok(local_ip)
    }

    fn get_wan_ip_control_urls(&self) -> impl Iterator<Item = (tracing::Span, Url)> + '_ {
        self.iter_services()
            .filter(|(_, s)| s.service_type == SERVICE_TYPE_WAN_IP_CONNECTION)
            .map(|(span, s)| (span, self.discover_response.location.join(&s.control_url)))
            .filter_map(|(span, url)| match url {
                // commandes envoyées seulement à l'appareil qui a répondu (voir `url_is_on`)
                Ok(url) if url_is_on(&url, self.discover_response.received_from.ip()) => {
                    Some((span, url))
                }
                Ok(url) => {
                    debug!("control url {url} is not on the answering device");
                    None
                }
                Err(e) => {
                    debug!("bad control url: {e:#}");
                    None
                }
            })
    }
}

#[derive(Debug)]
pub struct UpnpDiscoverResponse {
    pub received_from: SocketAddr,
    pub location: Url,
}

pub async fn discover_services(location: Url) -> anyhow::Result<RootDesc> {
    let response = upnp_client()
        .get(location.clone())
        .send()
        .await
        .context("failed to send GET request")?;
    let response = read_capped(response)
        .await
        .context("failed to read response body")?;
    trace!("received from {location}: {response}");
    let root_desc: RootDesc = quick_xml::de::from_str(&response)
        .context("failed to parse response body as xml")
        .inspect_err(|e| {
            debug!("failed to parse this XML: {response}. Error: {e:#}");
        })?;
    Ok(root_desc)
}

pub fn parse_upnp_discover_response(
    buf: &[u8],
    received_from: SocketAddr,
) -> anyhow::Result<UpnpDiscoverResponse> {
    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut resp = httparse::Response::new(&mut headers);
    resp.parse(buf).context("error parsing response")?;

    trace!(?resp, "parsed SSDP response");
    match resp.code {
        Some(200) => {}
        other => anyhow::bail!("bad response code {other:?}, expected 200"),
    }
    let mut location = None;
    for header in resp.headers {
        match header.name {
            "location" | "LOCATION" | "Location" => {
                location = Some(
                    std::str::from_utf8(header.value).context("bad utf-8 in location header")?,
                )
            }
            _ => continue,
        }
    }
    let location = location.context("missing location header")?;
    let location =
        Url::parse(location).with_context(|| format!("failed parsing location {location}"))?;
    // la description doit être sur l'appareil qui a répondu : sinon, n'importe quel appareil
    // du réseau local ferait lire à l'appli l'adresse de son choix (autre machine, Internet)
    if !url_is_on(&location, received_from.ip()) {
        bail!("location {location} is not on the answering device {received_from}");
    }
    Ok(UpnpDiscoverResponse {
        location,
        received_from,
    })
}

/// Adresses IPv4 d'où envoyer la recherche : chaque carte active (hors boucle locale et hors
/// 169.254.x.x, adresses automatiques sans réseau). Liée à 0.0.0.0, la recherche partait par la
/// carte que Windows choisit pour le multicast, parfois une carte inactive : la box ne
/// répondait qu'après plusieurs tentatives d'une minute (Torrent Cloud).
fn ssdp_source_addresses() -> Vec<Ipv4Addr> {
    let addrs: Vec<Ipv4Addr> = NetworkInterface::show()
        .unwrap_or_default()
        .iter()
        .flat_map(|nic| nic.addr.iter())
        .filter_map(|a| match a.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() => {
                Some(ip)
            }
            _ => None,
        })
        .collect();
    if addrs.is_empty() {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        addrs
    }
}

pub async fn discover_once(
    tx: &UnboundedSender<UpnpDiscoverResponse>,
    kind: &str,
    timeout: Duration,
    bind_device: Option<&BindDevice>,
) -> anyhow::Result<()> {
    // carte imposée (VPN) : une seule recherche, par elle
    let sources = if bind_device.is_some() {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        ssdp_source_addresses()
    };
    let results = futures::future::join_all(
        sources
            .into_iter()
            .map(|source| discover_from(tx, kind, timeout, bind_device, source)),
    )
    .await;
    if results.iter().all(|r| r.is_err()) {
        return results.into_iter().next().unwrap_or(Ok(()));
    }
    Ok(())
}

async fn discover_from(
    tx: &UnboundedSender<UpnpDiscoverResponse>,
    kind: &str,
    timeout: Duration,
    bind_device: Option<&BindDevice>,
    source: Ipv4Addr,
) -> anyhow::Result<()> {
    // TODO: do we need IPv6 support here? I can't test it, don't have the hardware for it (router / IPv6 provider).
    let socket = UdpSocket::bind_udp(
        (source, 0).into(),
        librqbit_dualstack_sockets::BindOpts {
            device: bind_device,
            ..Default::default()
        },
    )?;

    let message = make_ssdp_search_request(kind);
    socket
        .send_to(message.as_bytes(), SSDP_MULTICAST_IP)
        .await
        .with_context(|| format!("failed to send SSDP search request to {SSDP_MULTICAST_IP}"))?;

    let mut buffer = [0; 2048];

    let timeout = tokio::time::sleep(timeout);
    let mut timed_out = false;
    tokio::pin!(timeout);

    let mut discovered = 0;
    let mut seen = HashSet::<Url>::new();

    while !timed_out && discovered < MAX_DEVICES_PER_SEARCH {
        tokio::select! {
            _ = &mut timeout, if !timed_out => {
                timed_out = true;
            }
            Ok((len, addr)) = socket.recv_from(&mut buffer), if !timed_out => {
                let response = &buffer[..len];
                match parse_upnp_discover_response(response, addr) {
                    // une box répond souvent plusieurs fois (une par service)
                    Ok(r) if !seen.insert(r.location.clone()) => {},
                    Ok(r) => {
                        tx.send(r)?;
                        discovered += 1;
                    },
                    Err(e) => warn!(response=?BStr::new(response), "failed to parse SSDP response: {e:#}"),
                };
            },
        }
    }

    debug!(%source, "discovered {discovered} endpoints");
    Ok(())
}

/// Appelé avec (port interne, port externe) à chaque redirection obtenue.
pub type ExternalPortCallback = Arc<dyn Fn(u16, u16) + Send + Sync>;

pub struct UpnpPortForwarderOptions {
    pub lease_duration: Duration,
    pub discover_interval: Duration,
    pub discover_timeout: Duration,
    /// Nom affiché dans la liste des redirections de la box.
    pub description: String,
    /// Port externe obtenu : il peut différer du port interne si la box le refuse.
    pub on_external_port: Option<ExternalPortCallback>,
    /// Redirections obtenues, à retirer à la fermeture (`ActiveMappings::remove_all`).
    pub mappings: Option<Arc<ActiveMappings>>,
}

impl Default for UpnpPortForwarderOptions {
    fn default() -> Self {
        Self {
            discover_interval: Duration::from_secs(60),
            discover_timeout: Duration::from_secs(10),
            // 20 min, renouvelées toutes les 6 min 40 : retirées à la fermeture normale, et une
            // redirection oubliée (appli tuée, plantage) ne traîne pas plus de 20 min
            lease_duration: Duration::from_secs(20 * 60),
            description: "rqbit".to_owned(),
            on_external_port: None,
            mappings: None,
        }
    }
}

pub struct UpnpPortForwarder {
    ports: Vec<u16>,
    opts: UpnpPortForwarderOptions,
    bind_device: Option<BindDevice>,
}

impl UpnpPortForwarder {
    pub fn new(
        ports: Vec<u16>,
        opts: Option<UpnpPortForwarderOptions>,
        bind_device: Option<BindDevice>,
    ) -> anyhow::Result<Self> {
        if ports.is_empty() {
            bail!("empty ports")
        }
        Ok(Self {
            ports,
            opts: opts.unwrap_or_default(),
            bind_device,
        })
    }

    async fn parse_endpoint(
        &self,
        discover_response: UpnpDiscoverResponse,
    ) -> anyhow::Result<UpnpEndpoint> {
        let services = discover_services(discover_response.location.clone()).await?;
        let nics = network_interface::NetworkInterface::show()
            .context("error listing network interfaces")?;
        Ok(UpnpEndpoint {
            discover_response,
            data: services,
            nics,
        })
    }

    async fn discover_once(
        &self,
        tx: &UnboundedSender<UpnpDiscoverResponse>,
    ) -> anyhow::Result<()> {
        discover_once(
            tx,
            SSDP_SEARCH_WAN_IPCONNECTION_ST,
            self.opts.discover_timeout,
            self.bind_device.as_ref(),
        )
        .await
    }

    async fn discovery(&self, tx: UnboundedSender<UpnpDiscoverResponse>) -> anyhow::Result<()> {
        let mut discover_interval = tokio::time::interval(self.opts.discover_interval);
        discover_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            discover_interval.tick().await;
            if let Err(e) = self.discover_once(&tx).await {
                warn!("failed to run SSDP/UPNP discovery: {e:#}");
            }
        }
    }

    /// Redirige TCP (indispensable) et UDP (µTP, au mieux) du même port externe.
    async fn forward_both(
        &self,
        control_url: &Url,
        local_ip: IpAddr,
        external: u16,
        internal: u16,
        lease: Duration,
    ) -> Result<(), ForwardError> {
        let description = &self.opts.description;
        forward_port(
            control_url.clone(),
            local_ip,
            external,
            internal,
            "TCP",
            lease,
            description,
        )
        .await?;
        if let Err(e) = forward_port(
            control_url.clone(),
            local_ip,
            external,
            internal,
            "UDP",
            lease,
            description,
        )
        .await
        {
            debug!(external, "UDP port forwarding failed (TCP ok): {e}");
        }
        Ok(())
    }

    async fn manage_port(&self, control_url: Url, local_ip: IpAddr, port: u16) -> ! {
        let mut lease = self.opts.lease_duration;
        let mut external = port;
        let mut last_published = None;
        let mut failing = false;
        loop {
            let result = match self
                .forward_both(&control_url, local_ip, external, port, lease)
                .await
            {
                Err(ForwardError::Upnp {
                    code: ONLY_PERMANENT_LEASES,
                    ..
                }) if !lease.is_zero() => {
                    debug!("router only accepts permanent leases, retrying with lease 0");
                    lease = Duration::ZERO;
                    continue;
                }
                Err(ForwardError::Upnp { code, .. })
                    if RETRY_OTHER_EXTERNAL_PORT.contains(&code) =>
                {
                    // port refusé : la box a sa propre plage autorisée, on en cherche un autre
                    let mut found = Err(ForwardError::Upnp {
                        code,
                        description: String::new(),
                    });
                    for candidate in candidate_external_ports(port ^ external).take(64) {
                        found = self
                            .forward_both(&control_url, local_ip, candidate, port, lease)
                            .await;
                        match &found {
                            Ok(()) => {
                                external = candidate;
                                break;
                            }
                            Err(ForwardError::Upnp { code, .. })
                                if RETRY_OTHER_EXTERNAL_PORT.contains(code) =>
                            {
                                continue;
                            }
                            Err(_) => break,
                        }
                    }
                    found
                }
                other => other,
            };
            let wait = match result {
                Ok(()) => {
                    if let Some(mappings) = &self.opts.mappings {
                        mappings.upnp(&control_url, external);
                    }
                    if last_published != Some(external) {
                        tracing::info!(internal = port, external, "UPnP port forwarding active");
                        if let Some(cb) = &self.opts.on_external_port {
                            cb(port, external);
                        }
                        last_published = Some(external);
                    }
                    failing = false;
                    if lease.is_zero() {
                        Duration::from_secs(20 * 60)
                    } else {
                        lease / 3
                    }
                }
                Err(e) => {
                    // un seul avertissement par série d'échecs, pas un toutes les 30 s
                    if !failing {
                        warn!("failed to forward port {port}: {e}");
                    } else {
                        debug!("failed to forward port {port}: {e}");
                    }
                    failing = true;
                    Duration::from_secs(5 * 60)
                }
            };
            tokio::time::sleep(wait).await;
        }
    }

    async fn manage_service(&self, control_url: Url, local_ip: IpAddr) -> anyhow::Result<()> {
        futures::future::join_all(self.ports.iter().cloned().map(|port| {
            self.manage_port(control_url.clone(), local_ip, port)
                .instrument(debug_span!("manage_port", port = port))
        }))
        .await;
        Ok(())
    }

    pub async fn run_forever(self) -> ! {
        let (discover_tx, mut discover_rx) = unbounded_channel();
        let discovery = self.discovery(discover_tx);

        let mut spawned_tasks = HashSet::<Url>::new();

        let mut endpoints = FuturesUnordered::new();
        let mut service_managers = FuturesUnordered::new();

        tokio::pin!(discovery);

        loop {
            tokio::select! {
                _ = &mut discovery => {},
                r = discover_rx.recv() => {
                    let r = r.unwrap();
                    let location = r.location.clone();
                    endpoints.push(self.parse_endpoint(r).map_err(|e| {
                        debug!("error parsing endpoint: {e:#}");
                        e
                    }).instrument(debug_span!("parse endpoint", location=location.to_string())));
                },
                Some(Ok(endpoint)) = endpoints.next(), if !endpoints.is_empty() => {
                    let mut local_ip = None;
                    for (span, control_url) in endpoint.get_wan_ip_control_urls() {
                        if spawned_tasks.contains(&control_url) {
                            debug!("already spawned for {}", control_url);
                            continue;
                        }
                        let ip = match local_ip {
                            Some(ip) => ip,
                            None => {
                                match endpoint.my_local_ip() {
                                    Ok(ip) => {
                                        local_ip = Some(ip);
                                        ip
                                    },
                                    Err(e) => {
                                        warn!("failed to determine local IP for endpoint at {}: {:#}", endpoint.location(), e);
                                        break;
                                    }
                                }
                            }
                        };
                        spawned_tasks.insert(control_url.clone());
                        service_managers.push(self.manage_service(control_url, ip).instrument(span))
                    }
                },
                _ = service_managers.next(), if !service_managers.is_empty() => {

                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use quick_xml::de::from_str;

    fn ssdp(location: &str) -> Vec<u8> {
        format!("HTTP/1.1 200 OK\r\nLOCATION: {location}\r\nST: upnp:rootdevice\r\n\r\n")
            .into_bytes()
    }

    /// Appareil du réseau local qui veut faire lire une autre adresse à l'appli.
    #[test]
    fn ssdp_answers_must_point_to_the_answering_device() {
        let from: std::net::SocketAddr = "192.168.1.254:1900".parse().unwrap();
        let ok = crate::parse_upnp_discover_response(
            &ssdp("http://192.168.1.254:49152/rootDesc.xml"),
            from,
        );
        assert!(ok.is_ok());
        for location in [
            "http://192.168.1.10:8080/admin",
            "http://127.0.0.1:1234/x",
            "https://attaquant.example/rootDesc.xml",
            "http://box.lan/rootDesc.xml",
            "file:///C:/Windows/win.ini",
        ] {
            assert!(
                crate::parse_upnp_discover_response(&ssdp(location), from).is_err(),
                "{location}"
            );
        }
        // IPv4 notée en IPv6 (socket double pile)
        let mapped: std::net::SocketAddr = "[::ffff:192.168.1.254]:1900".parse().unwrap();
        assert!(
            crate::parse_upnp_discover_response(&ssdp("http://192.168.1.254:5000/d.xml"), mapped)
                .is_ok()
        );
    }

    /// Faux appareil en bac à sable (127.0.0.1) qui renvoie une description sans fin.
    #[tokio::test]
    async fn endless_device_description_is_cut() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .await;
            let block = vec![b'<'; 64 * 1024];
            for _ in 0..1024 {
                if socket.write_all(&block).await.is_err() {
                    return;
                }
            }
        });
        let url = url::Url::parse(&format!("http://{addr}/rootDesc.xml")).unwrap();
        let err = crate::discover_services(url).await.err().unwrap();
        assert!(format!("{err:#}").contains("too large"), "{err:#}");
    }

    #[test]
    fn soap_fault_is_parsed() {
        let fault = "<s:Fault><detail><UPnPError><errorCode>718</errorCode><errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault>";
        assert_eq!(crate::soap_value(fault, "errorCode"), Some("718"));
        assert_eq!(
            crate::soap_value(fault, "errorDescription"),
            Some("ConflictInMappingEntry")
        );
        assert_eq!(crate::soap_value("<ok/>", "errorCode"), None);
    }

    #[test]
    fn candidate_ports_are_varied_and_valid() {
        let ports: Vec<u16> = crate::candidate_external_ports(64418).take(64).collect();
        assert!(ports.iter().all(|&p| p >= 1024));
        let distinct: std::collections::HashSet<_> = ports.iter().collect();
        assert!(distinct.len() > 60, "ports trop répétitifs");
        // assez de ports dans chaque quart de la plage (les Freebox n'en autorisent qu'un)
        for quarter in 0..4u32 {
            let (lo, hi) = (quarter * 16384, (quarter + 1) * 16384);
            assert!(
                ports
                    .iter()
                    .filter(|&&p| u32::from(p) >= lo && u32::from(p) < hi)
                    .count()
                    >= 5,
                "quart {quarter}"
            );
        }
    }

    /// Vraie box (Torrent Cloud) : le port 64418 est refusé par les Freebox en IPv4 partagée ;
    /// le gestionnaire doit trouver seul un port externe accepté. Ignoré par défaut.
    #[tokio::test]
    #[ignore]
    async fn probe_forwarder_finds_allowed_port() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        let got = Arc::new(Mutex::new(None));
        let sink = got.clone();
        let opts = crate::UpnpPortForwarderOptions {
            lease_duration: Duration::from_secs(120),
            description: "Torrent Cloud test".into(),
            on_external_port: Some(Arc::new(move |internal, external| {
                *sink.lock().unwrap() = Some((internal, external));
            })),
            ..Default::default()
        };
        let fwd = crate::UpnpPortForwarder::new(vec![64418], Some(opts), None).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(20), fwd.run_forever()).await;
        let result = *got.lock().unwrap();
        eprintln!("redirection obtenue (interne, externe) : {result:?}");
        assert!(result.is_some(), "aucune redirection obtenue");
    }

    /// Diagnostic sur la vraie box (Torrent Cloud) : rejoue découverte, choix de l'IP locale et
    /// AddPortMapping, et affiche tout. Ignoré par défaut ; la redirection expire en 60 s.
    #[tokio::test]
    #[ignore]
    async fn probe_real_router() {
        use std::time::Duration;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::discover_once(
            &tx,
            crate::SSDP_SEARCH_WAN_IPCONNECTION_ST,
            Duration::from_secs(3),
            None,
        )
        .await
        .unwrap();
        let fwd = crate::UpnpPortForwarder::new(vec![47393], None, None).unwrap();
        while let Ok(resp) = rx.try_recv() {
            eprintln!("réponse de {} : {}", resp.received_from, resp.location);
            let ep = fwd.parse_endpoint(resp).await.unwrap();
            for nic in &ep.nics {
                for a in &nic.addr {
                    eprintln!(
                        "  carte {} (index {}) : {} masque {:?}",
                        nic.name,
                        nic.index,
                        a.ip(),
                        a.netmask()
                    );
                }
            }
            let ip = ep.my_local_ip();
            eprintln!("IP locale choisie : {ip:?}");
            for (_, url) in ep.get_wan_ip_control_urls() {
                for port in [47393u16, 64418] {
                    let r = crate::forward_port(
                        url.clone(),
                        *ip.as_ref().unwrap(),
                        port,
                        port,
                        "TCP",
                        Duration::from_secs(60),
                        "Torrent Cloud test",
                    )
                    .await;
                    eprintln!(
                        "AddPortMapping {url} port {port} : {:?}",
                        r.map_err(|e| e.to_string())
                    );
                }
            }
        }
    }

    use crate::{Device, DeviceList, RootDesc, Service, ServiceList};

    #[test]
    fn test_parse_root_desc() {
        let actual = from_str::<RootDesc>(include_str!("resources/test/devices-0.xml")).unwrap();
        let expected = RootDesc {
            devices: vec![Device {
                device_type: "urn:schemas-upnp-org:device:InternetGatewayDevice:1".into(),
                friendly_name: "ARRIS TG3492LG".into(),
                service_list: ServiceList {
                    services: vec![Service {
                        service_type: "urn:schemas-upnp-org:service:Layer3Forwarding:1".into(),
                        control_url: "/upnp/control/Layer3Forwarding".into(),
                        scpd_url: "/Layer3ForwardingSCPD.xml".into(),
                        event_sub_url: Some("/upnp/event/Layer3Forwarding".into()),
                    }],
                },
                device_list: DeviceList {
                    devices: vec![Device {
                        device_type: "urn:schemas-upnp-org:device:WANDevice:1".into(),
                        friendly_name: "WANDevice:1".into(),
                        service_list: ServiceList {
                            services: vec![Service {
                                service_type:
                                    "urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1".into(),
                                control_url: "/upnp/control/WANCommonInterfaceConfig0".into(),
                                scpd_url: "/WANCommonInterfaceConfigSCPD.xml".into(),
                                event_sub_url: Some("/upnp/event/WANCommonInterfaceConfig0".into()),
                            }],
                        },
                        device_list: DeviceList {
                            devices: vec![Device {
                                device_type: "urn:schemas-upnp-org:device:WANConnectionDevice:1"
                                    .into(),
                                friendly_name: "WANConnectionDevice:1".into(),
                                service_list: ServiceList {
                                    services: vec![Service {
                                        service_type:
                                            "urn:schemas-upnp-org:service:WANIPConnection:1".into(),
                                        control_url: "/upnp/control/WANIPConnection0".into(),
                                        scpd_url: "/WANIPConnectionServiceSCPD.xml".into(),
                                        event_sub_url: Some("/upnp/event/WANIPConnection0".into()),
                                    }],
                                },
                                device_list: DeviceList { devices: vec![] },
                            }],
                        },
                    }],
                },
            }],
        };
        assert_eq!(actual, expected);
    }
}
