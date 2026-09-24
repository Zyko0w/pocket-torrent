//! Redirection de port par NAT-PMP (RFC 6886), telle que la propose ProtonVPN : on demande au
//! serveur VPN, à travers le tunnel, un port public qui renvoie vers nous. Sans elle, derrière un
//! VPN, les autres pairs ne peuvent pas se connecter à nous : moins de sources, moins de débit.
//!
//! ProtonVPN attribue lui-même le port public (la demande porte « port public 1, port local 0 »,
//! comme dans sa documentation) et le garde tant qu'on renouvelle avant l'expiration (60 s).
//! Le client doit alors écouter sur ce même numéro de port.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};

/// Port NAT-PMP de la passerelle.
pub const PORT: u16 = 5351;
/// Durée demandée pour chaque redirection ; renouvelée avant (voir `manager::forwarding`).
pub const LIFETIME_SECS: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Udp = 1,
    Tcp = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub public_port: u16,
    pub lifetime_secs: u32,
}

/// Demande de redirection : version 0, code d'opération, 2 octets réservés, port local, port
/// public souhaité, durée (grand-boutien).
pub fn encode_request(protocol: Protocol, internal_port: u16, external_port: u16, lifetime: u32) -> [u8; 12] {
    let mut packet = [0u8; 12];
    packet[1] = protocol as u8;
    packet[4..6].copy_from_slice(&internal_port.to_be_bytes());
    packet[6..8].copy_from_slice(&external_port.to_be_bytes());
    packet[8..12].copy_from_slice(&lifetime.to_be_bytes());
    packet
}

/// Réponse : version 0, code d'opération + 128, code de résultat, époque, port local, port
/// public attribué, durée. Tout ce qui ne correspond pas exactement est refusé.
pub fn decode_response(protocol: Protocol, packet: &[u8]) -> anyhow::Result<Mapping> {
    if packet.len() < 16 || packet[0] != 0 || packet[1] != 128 + protocol as u8 {
        bail!("réponse NAT-PMP illisible");
    }
    let result = u16::from_be_bytes([packet[2], packet[3]]);
    if result != 0 {
        bail!(match result {
            1 => "version de NAT-PMP non prise en charge par le serveur VPN",
            2 => "redirection de port refusée par le serveur VPN (serveur sans P2P, ou option désactivée)",
            3 => "serveur VPN sans connexion Internet pour la redirection",
            4 => "le serveur VPN n'a plus de port libre",
            5 => "le serveur VPN ne gère pas cette demande",
            _ => "erreur NAT-PMP inconnue",
        });
    }
    let public_port = u16::from_be_bytes([packet[10], packet[11]]);
    if public_port == 0 {
        bail!("le serveur VPN n'a attribué aucun port");
    }
    // ports système (partage de fichiers, web…) : jamais attribués par un vrai serveur VPN, et
    // le moteur devrait y écouter à la place d'un service de Windows
    if public_port < 1024 {
        bail!("port {public_port} refusé : réservé au système");
    }
    Ok(Mapping { public_port, lifetime_secs: u32::from_be_bytes([packet[12], packet[13], packet[14], packet[15]]) })
}

/// Passerelle NAT-PMP du VPN : première adresse du réseau de l'adresse VPN (10.2.0.2 → 10.2.0.1
/// chez ProtonVPN). IPv4 seulement : NAT-PMP n'existe pas en IPv6.
pub fn gateway_for(vpn_ip: IpAddr) -> Option<SocketAddr> {
    let IpAddr::V4(ip) = vpn_ip else { return None };
    let [a, b, c, _] = ip.octets();
    Some(SocketAddr::from((Ipv4Addr::new(a, b, c, 1), PORT)))
}

/// Demande une redirection à `gateway`, depuis l'adresse `local_ip` (celle du VPN : la demande
/// doit passer par le tunnel). Délai doublé à chaque essai (250 ms, 500 ms, 1 s, 2 s), comme le
/// prévoit la RFC, et seules les réponses venues de la passerelle sont lues.
pub async fn request(local_ip: IpAddr, gateway: SocketAddr, protocol: Protocol) -> anyhow::Result<Mapping> {
    let socket = tokio::net::UdpSocket::bind((local_ip, 0)).await.context("envoi NAT-PMP impossible depuis le VPN")?;
    exchange(&socket, gateway, protocol).await
}

/// Échange sur `socket`, relié (`connect`) à la passerelle : le système n'y laisse entrer que
/// les réponses venues d'elle, une fausse réponse envoyée par un autre programme est écartée.
async fn exchange(socket: &tokio::net::UdpSocket, gateway: SocketAddr, protocol: Protocol) -> anyhow::Result<Mapping> {
    socket.connect(gateway).await.context("passerelle NAT-PMP injoignable")?;
    let packet = encode_request(protocol, 0, 1, LIFETIME_SECS);
    let mut wait = Duration::from_millis(250);
    let mut buffer = [0u8; 64];
    for _ in 0..4 {
        socket.send(&packet).await.context("envoi NAT-PMP impossible")?;
        if let Ok(received) = tokio::time::timeout(wait, socket.recv(&mut buffer)).await {
            let len = received.context("réception NAT-PMP impossible")?;
            return decode_response(protocol, &buffer[..len]);
        }
        wait *= 2;
    }
    Err(anyhow!(
        "le serveur VPN ne répond pas à la demande de redirection de port (NAT-PMP) : choisis un serveur P2P, \
         avec la redirection de port activée chez ton fournisseur"
    ))
}

/// Tests dans un bac à sable : un faux serveur NAT-PMP sur 127.0.0.1, jamais le vrai VPN.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_matches_protonvpn_documentation() {
        // natpmpc -a 1 0 tcp 60 : port public 1, port local 0, 60 s
        assert_eq!(encode_request(Protocol::Tcp, 0, 1, 60), [0, 2, 0, 0, 0, 0, 0, 1, 0, 0, 0, 60]);
    }

    fn response(opcode: u8, result: u16, port: u16) -> Vec<u8> {
        let mut packet = vec![0, opcode];
        packet.extend(result.to_be_bytes());
        packet.extend(1234u32.to_be_bytes()); // époque
        packet.extend(0u16.to_be_bytes());
        packet.extend(port.to_be_bytes());
        packet.extend(60u32.to_be_bytes());
        packet
    }

    #[test]
    fn responses_are_checked_strictly() {
        assert_eq!(decode_response(Protocol::Udp, &response(129, 0, 51234)).unwrap().public_port, 51234);
        // réponse à une autre demande (TCP au lieu d'UDP), tronquée, en erreur, ou sans port
        assert!(decode_response(Protocol::Udp, &response(130, 0, 51234)).is_err());
        assert!(decode_response(Protocol::Udp, &response(129, 0, 51234)[..10]).is_err());
        let refused = decode_response(Protocol::Udp, &response(129, 2, 0)).unwrap_err();
        assert!(format!("{refused}").contains("refusée"));
        assert!(decode_response(Protocol::Udp, &response(129, 0, 0)).is_err());
        // serveur VPN hostile : ports système refusés (445 = partage de fichiers Windows)
        for port in [1, 80, 445, 1023] {
            assert!(decode_response(Protocol::Udp, &response(129, 0, port)).is_err(), "{port}");
        }
        assert!(decode_response(Protocol::Udp, &response(129, 0, 1024)).is_ok());
    }

    #[tokio::test]
    async fn answers_from_anyone_but_the_gateway_are_ignored() {
        // la vraie passerelle se tait ; un autre programme connaît le port du client et lui
        // envoie une fausse réponse (port 3389, bureau à distance) pendant toute l'attente
        let gateway = fake_gateway(None).await;
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_addr = socket.local_addr().unwrap();
        let attacker = tokio::spawn(async move {
            let attacker = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            for _ in 0..80 {
                let _ = attacker.send_to(&response(129, 0, 3389), client_addr).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let err = exchange(&socket, gateway, Protocol::Udp).await.unwrap_err();
        attacker.abort();
        assert!(format!("{err}").contains("ne répond pas"), "fausse réponse acceptée : {err}");
    }

    #[test]
    fn gateway_is_the_first_address_of_the_vpn_network() {
        assert_eq!(gateway_for("10.2.0.2".parse().unwrap()), Some("10.2.0.1:5351".parse().unwrap()));
        assert_eq!(gateway_for("fd00::2".parse().unwrap()), None);
    }

    /// Faux serveur : répond `answer` à chaque demande reçue (ou ne répond jamais si `None`).
    async fn fake_gateway(answer: Option<Vec<u8>>) -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 64];
            while let Ok((_, from)) = socket.recv_from(&mut buffer).await {
                if let Some(answer) = &answer {
                    let _ = socket.send_to(answer, from).await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn port_is_obtained_from_the_gateway() {
        let gateway = fake_gateway(Some(response(130, 0, 45678))).await;
        let mapping = request("127.0.0.1".parse().unwrap(), gateway, Protocol::Tcp).await.unwrap();
        assert_eq!(mapping, Mapping { public_port: 45678, lifetime_secs: 60 });
    }

    #[tokio::test]
    async fn silent_gateway_gives_a_clear_error() {
        let gateway = fake_gateway(None).await;
        let err = request("127.0.0.1".parse().unwrap(), gateway, Protocol::Udp).await.unwrap_err();
        assert!(format!("{err}").contains("ne répond pas"), "{err}");
    }
}
