//! Sécurité des identifiants DHT (BEP 42), ajoutée pour Pocket Torrent.
//!
//! Les 21 premiers bits de l'identifiant d'un nœud sont tirés de son adresse IP : un attaquant ne
//! peut plus choisir sa place dans la DHT pour entourer un torrent précis (relever qui le
//! télécharge, ou le faire paraître sans pairs). Notre identifiant est calculé à partir de notre
//! adresse publique, apprise des réponses des autres nœuds ; les nœuds conformes sont préférés
//! dans la table de routage.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;

use librqbit_core::hash_id::Id20;

/// CRC-32C (Castagnoli), comme l'exige le BEP 42.
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

/// Adresses exemptées par le BEP 42 (réseau local, boucle, lien local) et non publiques.
fn exempt(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xFE00) == 0xFC00
                || (first & 0xFFC0) == 0xFE80
        }
    }
}

/// CRC des bits significatifs de `ip` combinés au petit nombre `r` (0 à 7).
fn ip_crc(ip: IpAddr, r: u8) -> u32 {
    match canonical(ip) {
        IpAddr::V4(v4) => {
            const MASK: [u8; 4] = [0x03, 0x0F, 0x3F, 0xFF];
            let mut bytes = v4.octets();
            for (b, m) in bytes.iter_mut().zip(MASK) {
                *b &= m;
            }
            bytes[0] |= (r & 7) << 5;
            crc32c(&bytes)
        }
        IpAddr::V6(v6) => {
            const MASK: [u8; 8] = [0x01, 0x03, 0x07, 0x0F, 0x1F, 0x3F, 0x7F, 0xFF];
            let mut bytes: [u8; 8] = v6.octets()[..8].try_into().expect("8 octets");
            for (b, m) in bytes.iter_mut().zip(MASK) {
                *b &= m;
            }
            bytes[0] |= (r & 7) << 5;
            crc32c(&bytes)
        }
    }
}

/// Identifiant conforme pour `ip` ; `random` fournit les octets libres (le dernier octet devient
/// le « rand » du BEP 42).
pub fn node_id(ip: IpAddr, random: [u8; 20]) -> Id20 {
    let rand = random[19];
    let crc = ip_crc(ip, rand & 7);
    let mut id = random;
    id[0] = (crc >> 24) as u8;
    id[1] = (crc >> 16) as u8;
    id[2] = ((crc >> 8) as u8 & 0xF8) | (random[2] & 0x07);
    id[19] = rand;
    Id20::new(id)
}

/// L'identifiant `id` est-il conforme pour un nœud joint à `ip` ? Toujours vrai pour une adresse
/// exemptée (réseau local).
pub fn is_compliant(id: &Id20, ip: IpAddr) -> bool {
    if exempt(ip) {
        return true;
    }
    let crc = ip_crc(ip, id.0[19] & 7);
    id.0[0] == (crc >> 24) as u8
        && id.0[1] == (crc >> 16) as u8
        && (id.0[2] & 0xF8) == ((crc >> 8) as u8 & 0xF8)
}

/// Nœud à adresse publique dont l'identifiant ne suit pas le BEP 42 (identifiant choisi).
pub fn is_suspect(id: &Id20, ip: IpAddr) -> bool {
    !exempt(ip) && !is_compliant(id, ip)
}

/// Notre adresse publique d'après le champ `ip` des réponses (BEP 42) : la plus citée parmi les
/// `WINDOW` derniers votes, par au moins `MIN_VOTERS` nœuds différents. Fenêtre glissante : une
/// nouvelle adresse (IP dynamique, autre serveur VPN) finit par l'emporter sur l'ancienne.
#[derive(Default)]
pub struct ExternalIpVotes {
    votes: parking_lot::Mutex<Votes>,
}

#[derive(Default)]
struct Votes {
    /// (nœud qui vote, adresse qu'il nous voit), du plus ancien au plus récent.
    window: VecDeque<(IpAddr, IpAddr)>,
    /// Adresse retenue.
    current: Option<IpAddr>,
}

const MIN_VOTERS: usize = 3;
const WINDOW: usize = 64;

impl ExternalIpVotes {
    /// `voter` dit nous voir en `reported`. Renvoie l'adresse retenue si elle vient de changer.
    pub fn record(&self, voter: IpAddr, reported: IpAddr) -> Option<IpAddr> {
        let (voter, reported) = (canonical(voter), canonical(reported));
        if exempt(reported) {
            return None;
        }
        let mut guard = self.votes.lock();
        let Votes { window, current } = &mut *guard;
        // un nœud ne compte qu'une fois dans la fenêtre (son vote le plus récent)
        window.retain(|(v, _)| *v != voter);
        window.push_back((voter, reported));
        if window.len() > WINDOW {
            window.pop_front();
        }
        let mut counts: HashMap<IpAddr, usize> = HashMap::new();
        for (_, ip) in window.iter() {
            *counts.entry(*ip).or_default() += 1;
        }
        let best = counts
            .into_iter()
            .filter(|(_, n)| *n >= MIN_VOTERS)
            .max_by_key(|(ip, n)| (*n, *ip))
            .map(|(ip, _)| ip);
        if best.is_some() && best != *current {
            *current = best;
            return best;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exemples du texte du BEP 42 : adresse, « rand », début de l'identifiant attendu.
    #[test]
    fn matches_the_bep_42_examples() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        for (ip, rand, prefix) in [
            ("124.31.75.21", 1u8, [0x5f, 0xbf, 0xb8]),
            ("21.75.31.124", 86, [0x5a, 0x3c, 0xe8]),
            ("65.23.51.170", 22, [0xa5, 0xd4, 0x30]),
            ("84.124.73.14", 65, [0x1b, 0x03, 0x20]),
            ("43.213.53.83", 90, [0xe5, 0x6f, 0x68]),
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            let mut random = [0u8; 20];
            random[19] = rand;
            let id = node_id(ip, random);
            assert_eq!([id.0[0], id.0[1], id.0[2] & 0xF8], prefix, "{ip}");
            assert_eq!(id.0[19], rand);
            assert!(is_compliant(&id, ip), "{ip}");
            // même identifiant vu d'une autre adresse : non conforme
            assert!(!is_compliant(&id, "8.8.8.8".parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn local_addresses_are_exempt_and_ipv6_works() {
        let random = [7u8; 20];
        let id = Id20::new(random);
        assert!(is_compliant(&id, "192.168.1.2".parse().unwrap()));
        assert!(!is_suspect(&id, "10.0.0.1".parse().unwrap()));
        let v6: IpAddr = "2001:db8:1234:5678::1".parse().unwrap();
        assert!(is_compliant(&node_id(v6, random), v6));
        assert!(is_suspect(&id, v6) || is_compliant(&id, v6));
        // IPv4 notée en IPv6 : même règle
        let v4: IpAddr = "81.10.20.30".parse().unwrap();
        assert!(is_compliant(
            &node_id(v4, random),
            "::ffff:81.10.20.30".parse().unwrap()
        ));
    }

    #[test]
    fn our_address_needs_several_public_votes_and_can_change() {
        let votes = ExternalIpVotes::default();
        let ours: IpAddr = "81.10.20.30".parse().unwrap();
        assert_eq!(votes.record("1.1.1.1".parse().unwrap(), ours), None);
        assert_eq!(
            votes.record("1.1.1.1".parse().unwrap(), ours),
            None,
            "un nœud ne vote qu'une fois"
        );
        assert_eq!(votes.record("2.2.2.2".parse().unwrap(), ours), None);
        assert_eq!(
            votes.record("3.3.3.3".parse().unwrap(), ours),
            Some(ours),
            "retenue au 3e vote"
        );
        assert_eq!(
            votes.record("4.4.4.4".parse().unwrap(), ours),
            None,
            "déjà retenue"
        );
        // adresses locales annoncées (menteurs, réseau local) : ignorées
        for i in 0..10 {
            assert_eq!(
                votes.record(
                    format!("9.9.9.{i}").parse().unwrap(),
                    "192.168.1.2".parse().unwrap()
                ),
                None
            );
        }
        // nouvelle adresse publique (autre serveur VPN) : l'emporte une fois majoritaire dans la fenêtre
        let new: IpAddr = "185.1.2.3".parse().unwrap();
        let mut adopted = None;
        for i in 0..70u32 {
            if let Some(ip) = votes.record(std::net::Ipv4Addr::from(0x0500_0000 + i).into(), new) {
                adopted = Some(ip);
            }
        }
        assert_eq!(adopted, Some(new));
    }
}
