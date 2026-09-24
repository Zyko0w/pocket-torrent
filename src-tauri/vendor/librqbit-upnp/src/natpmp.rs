//! NAT-PMP (RFC 6886), ajouté pour Torrent Cloud : l'équivalent d'UPnP sur les box qui ne
//! parlent que ce protocole (routeurs Apple, certains autres). Même principe : on demande à la
//! passerelle de rediriger un port externe vers notre port d'écoute, TCP et UDP.
//!
//! La passerelle n'est pas connue à l'avance : on interroge les candidates (celle trouvée par
//! UPnP si fournie, et les adresses .1 et .254 de chaque réseau local). Si aucune ne répond
//! (cas le plus courant : la box ne fait que de l'UPnP), on se tait et on réessaie plus tard.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use tokio::net::UdpSocket;
use tracing::{debug, info};

use crate::{ActiveMappings, ExternalPortCallback};

pub const NATPMP_PORT: u16 = 5351;
const OP_MAP_UDP: u8 = 1;
const OP_MAP_TCP: u8 = 2;
/// Durée demandée pour chaque redirection (renouvelée à mi-parcours ; retirée à la fermeture).
const LIFETIME_SECS: u32 = 20 * 60;
/// Sans passerelle NAT-PMP, nouvel essai au bout de ce délai.
const RETRY_WITHOUT_GATEWAY: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapping {
    pub external_port: u16,
    pub lifetime: u32,
}

/// Requête de redirection (RFC 6886 §3.3) : version 0, opération, 2 octets réservés, port
/// interne, port externe suggéré, durée.
fn map_request(op: u8, internal: u16, suggested_external: u16, lifetime: u32) -> [u8; 12] {
    let mut req = [0u8; 12];
    req[1] = op;
    req[4..6].copy_from_slice(&internal.to_be_bytes());
    req[6..8].copy_from_slice(&suggested_external.to_be_bytes());
    req[8..12].copy_from_slice(&lifetime.to_be_bytes());
    req
}

/// Réponse (§3.3) : version 0, opération + 128, code de résultat, époque, port interne, port
/// externe obtenu, durée accordée.
fn parse_map_response(op: u8, internal: u16, data: &[u8]) -> anyhow::Result<Mapping> {
    if data.len() < 16 || data[0] != 0 || data[1] != op + 128 {
        bail!("not a NAT-PMP mapping response");
    }
    let result = u16::from_be_bytes([data[2], data[3]]);
    if result != 0 {
        bail!("NAT-PMP error code {result}");
    }
    if u16::from_be_bytes([data[8], data[9]]) != internal {
        bail!("NAT-PMP response for another port");
    }
    Ok(Mapping {
        external_port: u16::from_be_bytes([data[10], data[11]]),
        lifetime: u32::from_be_bytes([data[12], data[13], data[14], data[15]]),
    })
}

/// Une demande de redirection, avec les réémissions de la RFC (250 ms, doublé, 3 essais).
pub async fn map_port(
    gateway: SocketAddr,
    tcp: bool,
    internal: u16,
    suggested: u16,
    lifetime: u32,
) -> anyhow::Result<Mapping> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.connect(gateway).await?;
    let op = if tcp { OP_MAP_TCP } else { OP_MAP_UDP };
    let req = map_request(op, internal, suggested, lifetime);
    let mut wait = Duration::from_millis(250);
    let mut buf = [0u8; 32];
    for _ in 0..3 {
        socket.send(&req).await?;
        match tokio::time::timeout(wait, socket.recv(&mut buf)).await {
            Ok(Ok(n)) => return parse_map_response(op, internal, &buf[..n]),
            // port fermé (ICMP) : pas de NAT-PMP ici, inutile d'insister
            Ok(Err(e)) => return Err(e).context("NAT-PMP gateway unreachable"),
            Err(_) => wait *= 2,
        }
    }
    bail!("no NAT-PMP answer from {gateway}")
}

/// Retire une redirection (§3.4 : durée 0 et port externe 0).
pub async fn unmap_port(gateway: SocketAddr, tcp: bool, internal: u16) -> anyhow::Result<()> {
    map_port(gateway, tcp, internal, 0, 0).await.map(|_| ())
}

/// Passerelles possibles : celles fournies, puis .1 et .254 de chaque réseau IPv4 local.
fn candidate_gateways(extra: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = extra.to_vec();
    for nic in NetworkInterface::show().unwrap_or_default() {
        for addr in &nic.addr {
            if let IpAddr::V4(ip) = addr.ip()
                && !ip.is_loopback()
                && !ip.is_link_local()
            {
                let [a, b, c, _] = ip.octets();
                for last in [1, 254] {
                    let gw = Ipv4Addr::new(a, b, c, last);
                    if gw != ip && !out.contains(&gw) {
                        out.push(gw);
                    }
                }
            }
        }
    }
    out
}

pub struct NatPmpForwarder {
    port: u16,
    known_gateways: Vec<Ipv4Addr>,
    on_external_port: Option<ExternalPortCallback>,
    mappings: Option<Arc<ActiveMappings>>,
}

impl NatPmpForwarder {
    pub fn new(
        port: u16,
        known_gateways: Vec<Ipv4Addr>,
        on_external_port: Option<ExternalPortCallback>,
    ) -> Self {
        Self {
            port,
            known_gateways,
            on_external_port,
            mappings: None,
        }
    }

    /// Note les redirections obtenues, pour les retirer à la fermeture.
    pub fn with_mappings(mut self, mappings: Arc<ActiveMappings>) -> Self {
        self.mappings = Some(mappings);
        self
    }

    /// Redirige TCP (indispensable) puis UDP (au mieux) via la première passerelle qui répond.
    async fn map_once(&self, gateways: &[SocketAddr]) -> Option<(SocketAddr, Mapping)> {
        for &gw in gateways {
            match map_port(gw, true, self.port, self.port, LIFETIME_SECS).await {
                Ok(tcp) => {
                    if let Err(e) =
                        map_port(gw, false, self.port, tcp.external_port, LIFETIME_SECS).await
                    {
                        debug!(%gw, "NAT-PMP UDP mapping failed (TCP ok): {e:#}");
                    }
                    return Some((gw, tcp));
                }
                Err(e) => debug!(%gw, "NAT-PMP: {e:#}"),
            }
        }
        None
    }

    pub async fn run_forever(self) -> ! {
        let mut last_published = None;
        loop {
            let gateways: Vec<SocketAddr> = candidate_gateways(&self.known_gateways)
                .into_iter()
                .map(|ip| SocketAddr::from((ip, NATPMP_PORT)))
                .collect();
            let wait = match self.map_once(&gateways).await {
                Some((gw, mapping)) => {
                    if let Some(mappings) = &self.mappings {
                        mappings.natpmp(gw, self.port);
                    }
                    if last_published != Some(mapping.external_port) {
                        info!(%gw, internal = self.port, external = mapping.external_port, "NAT-PMP port forwarding active");
                        if let Some(cb) = &self.on_external_port {
                            cb(self.port, mapping.external_port);
                        }
                        last_published = Some(mapping.external_port);
                    }
                    Duration::from_secs(u64::from(mapping.lifetime.max(120)) / 2)
                }
                None => {
                    debug!("no NAT-PMP gateway answered, retrying later");
                    RETRY_WITHOUT_GATEWAY
                }
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_response_follow_rfc6886() {
        let req = map_request(OP_MAP_TCP, 55869, 55869, 3600);
        assert_eq!(req, [0, 2, 0, 0, 0xDA, 0x3D, 0xDA, 0x3D, 0, 0, 0x0E, 0x10]);
        let mut resp = [0u8; 16];
        resp[1] = 130;
        resp[8..10].copy_from_slice(&55869u16.to_be_bytes());
        resp[10..12].copy_from_slice(&40001u16.to_be_bytes());
        resp[12..16].copy_from_slice(&7200u32.to_be_bytes());
        assert_eq!(
            parse_map_response(OP_MAP_TCP, 55869, &resp).unwrap(),
            Mapping {
                external_port: 40001,
                lifetime: 7200
            }
        );
        resp[3] = 2; // code 2 : non autorisé
        assert!(parse_map_response(OP_MAP_TCP, 55869, &resp).is_err());
    }

    /// Faux routeur NAT-PMP en local : attribue « port interne + 1000 ».
    #[tokio::test]
    async fn maps_through_a_fake_gateway() {
        let gw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = gw.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 32];
            while let Ok((n, from)) = gw.recv_from(&mut buf).await {
                if n < 12 || buf[0] != 0 {
                    continue;
                }
                let internal = u16::from_be_bytes([buf[4], buf[5]]);
                let mut resp = [0u8; 16];
                resp[1] = buf[1] + 128;
                resp[8..10].copy_from_slice(&internal.to_be_bytes());
                resp[10..12].copy_from_slice(&(internal + 1000).to_be_bytes());
                resp[12..16].copy_from_slice(&buf[8..12]);
                let _ = gw.send_to(&resp, from).await;
            }
        });
        let tcp = map_port(addr, true, 6881, 6881, 3600).await.unwrap();
        assert_eq!(
            tcp,
            Mapping {
                external_port: 7881,
                lifetime: 3600
            }
        );
        let udp = map_port(addr, false, 6881, 7881, 3600).await.unwrap();
        assert_eq!(udp.external_port, 7881);
        // retrait à la fermeture : durée 0 (le faux routeur renvoie la durée demandée)
        unmap_port(addr, true, 6881).await.unwrap();
        let mappings = ActiveMappings::default();
        mappings.natpmp(addr, 6881);
        mappings.remove_all(Duration::from_secs(3)).await;
        assert!(mappings.is_empty());
        // aucune passerelle : erreur rapide, pas de blocage
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let started = std::time::Instant::now();
        assert!(
            map_port(silent.local_addr().unwrap(), true, 6881, 6881, 60)
                .await
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
