//! Priorité canonique des pairs (BEP 40), ajoutée pour Pocket Torrent.
//!
//! Quand il y a plus de pairs connus que de connexions possibles, chaque client choisit ceux de
//! plus haute priorité. La priorité d'une paire de pairs est la même vue des deux côtés (calculée
//! sur les deux adresses), ce qui stabilise l'essaim et empêche un attaquant de se placer en tête
//! juste en choisissant son adresse dans le même réseau que sa cible.
//!
//! Il faut connaître sa propre adresse publique : elle est estimée à partir de ce que les pairs
//! disent voir (`yourip`, BEP 10), retenue seulement si plusieurs pairs différents s'accordent.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};

use crate::torrent_state::live::webseed::is_public_ip;

/// CRC-32C (Castagnoli), comme l'exige le BEP 40.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Priorité canonique de la paire (`a`, `b`) ; `None` si l'une est en IPv4 et l'autre en IPv6.
pub fn canonical_priority(a: SocketAddr, b: SocketAddr) -> Option<u32> {
    let (ip_a, ip_b) = (canonical(a.ip()), canonical(b.ip()));
    if ip_a == ip_b {
        let mut ports = [a.port(), b.port()];
        ports.sort_unstable();
        let bytes: Vec<u8> = ports.iter().flat_map(|p| p.to_be_bytes()).collect();
        return Some(crc32c(&bytes));
    }
    let (mut x, mut y, mask): (Vec<u8>, Vec<u8>, Vec<u8>) = match (ip_a, ip_b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let (a, b) = (a.octets(), b.octets());
            // même /24 : adresses entières ; même /16 : FF.FF.FF.55 ; sinon FF.FF.55.55
            let mask = if a[..3] == b[..3] {
                vec![0xFF; 4]
            } else if a[..2] == b[..2] {
                vec![0xFF, 0xFF, 0xFF, 0x55]
            } else {
                vec![0xFF, 0xFF, 0x55, 0x55]
            };
            (a.to_vec(), b.to_vec(), mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let (a, b) = (a.octets(), b.octets());
            // même /56 : entières ; même /48 : 7 octets gardés ; sinon 6 (le reste masqué par 55)
            let kept = if a[..7] == b[..7] {
                16
            } else if a[..6] == b[..6] {
                7
            } else {
                6
            };
            let mask = (0..16)
                .map(|i| if i < kept { 0xFF } else { 0x55 })
                .collect();
            (a.to_vec(), b.to_vec(), mask)
        }
        _ => return None,
    };
    for (byte, m) in x.iter_mut().zip(&mask) {
        *byte &= m;
    }
    for (byte, m) in y.iter_mut().zip(&mask) {
        *byte &= m;
    }
    let (low, high) = if x <= y { (x, y) } else { (y, x) };
    Some(crc32c(&[low, high].concat()))
}

/// Votes des pairs sur notre adresse publique (champ `yourip` de leur poignée de main).
#[derive(Default)]
pub struct ExternalIpVotes {
    votes: parking_lot::Mutex<HashMap<IpAddr, HashSet<IpAddr>>>,
}

/// Pairs différents qui doivent s'accorder avant qu'une adresse soit retenue.
const MIN_VOTERS: usize = 3;
/// Votants retenus au plus (mémoire bornée) : au-delà, les nouveaux votes sont ignorés.
const MAX_VOTERS: usize = 256;

impl ExternalIpVotes {
    /// `voter` (l'adresse du pair) dit nous voir en `reported`. Une adresse non publique est
    /// ignorée (pair du réseau local, ou menteur).
    pub fn record(&self, voter: IpAddr, reported: IpAddr) {
        let reported = canonical(reported);
        if !is_public_ip(reported) {
            return;
        }
        let voter = canonical(voter);
        let mut votes = self.votes.lock();
        if votes.values().any(|v| v.contains(&voter)) {
            return; // un pair ne vote qu'une fois
        }
        if votes.values().map(HashSet::len).sum::<usize>() >= MAX_VOTERS {
            return;
        }
        votes.entry(reported).or_default().insert(voter);
    }

    /// Adresse retenue : la plus citée, par au moins `MIN_VOTERS` pairs différents.
    pub fn best(&self) -> Option<IpAddr> {
        self.votes
            .lock()
            .iter()
            .filter(|(_, voters)| voters.len() >= MIN_VOTERS)
            .max_by_key(|(ip, voters)| (voters.len(), **ip))
            .map(|(ip, _)| *ip)
    }
}

/// Prochain pair à contacter parmi `pending` : priorité canonique la plus haute si notre adresse
/// est connue, sinon le plus ancien (ordre d'arrivée, comme avant).
pub fn take_next(
    pending: &mut std::collections::VecDeque<SocketAddr>,
    ours: Option<SocketAddr>,
) -> Option<SocketAddr> {
    let Some(ours) = ours else {
        return pending.pop_front();
    };
    let best = pending
        .iter()
        .enumerate()
        .max_by_key(|(i, addr)| {
            (
                canonical_priority(ours, **addr).unwrap_or(0),
                std::cmp::Reverse(*i),
            )
        })
        .map(|(i, _)| i)?;
    pending.remove(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// Exemples du texte du BEP 40.
    #[test]
    fn matches_the_bep_40_examples() {
        assert_eq!(
            crc32c(b"123456789"),
            0xE306_9283,
            "valeur de contrôle du CRC-32C"
        );
        assert_eq!(
            canonical_priority(addr("123.213.32.10:0"), addr("98.76.54.32:0")),
            Some(0xEC2D_7224)
        );
        assert_eq!(
            canonical_priority(addr("123.213.32.10:0"), addr("123.213.32.234:0")),
            Some(0x9956_8189)
        );
    }

    #[test]
    fn priority_is_the_same_from_both_sides() {
        for (a, b) in [
            ("1.2.3.4:6881", "5.6.7.8:51413"),
            ("1.2.3.4:6881", "1.2.9.9:6881"),
            ("1.2.3.4:6881", "1.2.3.4:6882"),
            ("[2001:db8::1]:1", "[2001:db8:0:1::2]:2"),
        ] {
            assert_eq!(
                canonical_priority(addr(a), addr(b)),
                canonical_priority(addr(b), addr(a)),
                "{a} {b}"
            );
        }
        // IPv4 notée en IPv6 : même priorité
        assert_eq!(
            canonical_priority(addr("[::ffff:1.2.3.4]:6881"), addr("5.6.7.8:1")),
            canonical_priority(addr("1.2.3.4:6881"), addr("5.6.7.8:1"))
        );
        assert_eq!(
            canonical_priority(addr("1.2.3.4:1"), addr("[2001:db8::1]:1")),
            None
        );
    }

    #[test]
    fn our_address_needs_several_public_votes() {
        let votes = ExternalIpVotes::default();
        let ours: IpAddr = "81.10.20.30".parse().unwrap();
        votes.record("1.1.1.1".parse().unwrap(), ours);
        votes.record("1.1.1.1".parse().unwrap(), ours); // même pair : une voix
        votes.record("2.2.2.2".parse().unwrap(), ours);
        assert_eq!(votes.best(), None, "deux pairs ne suffisent pas");
        votes.record("3.3.3.3".parse().unwrap(), ours);
        assert_eq!(votes.best(), Some(ours));
        // adresse privée annoncée par des menteurs : ignorée
        for i in 0..10 {
            votes.record(
                format!("9.9.9.{i}").parse().unwrap(),
                "192.168.1.2".parse().unwrap(),
            );
        }
        assert_eq!(votes.best(), Some(ours));
        // beaucoup de menteurs d'accord sur une autre adresse publique : mémoire bornée
        for i in 0..1000u32 {
            votes.record(
                IpAddr::from(std::net::Ipv4Addr::from(0x0A00_0000 + i)),
                "7.7.7.7".parse().unwrap(),
            );
        }
        assert!(votes.votes.lock().values().map(HashSet::len).sum::<usize>() <= MAX_VOTERS);
    }

    #[test]
    fn next_peer_is_the_highest_priority_once_our_address_is_known() {
        let peers: Vec<SocketAddr> = (1..=50u8)
            .map(|i| SocketAddr::from(([10 + i, i, 3, i], 6881)))
            .collect();
        let mut pending: std::collections::VecDeque<_> = peers.iter().copied().collect();
        // adresse inconnue : ordre d'arrivée
        assert_eq!(take_next(&mut pending, None), Some(peers[0]));
        let ours = addr("81.10.20.30:6881");
        let expected = *pending
            .iter()
            .max_by_key(|p| canonical_priority(ours, **p).unwrap())
            .unwrap();
        assert_eq!(take_next(&mut pending, Some(ours)), Some(expected));
        assert_eq!(pending.len(), 48);
    }
}
