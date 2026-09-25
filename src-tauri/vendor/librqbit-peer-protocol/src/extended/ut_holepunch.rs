//! Perçage de NAT (BEP 55, `ut_holepunch`), ajouté pour Pocket Torrent.
//!
//! Format binaire (pas de bencode) : type (1 octet), famille d'adresse (1 octet : 0 IPv4,
//! 1 IPv6), adresse (4 ou 16 octets), port (2 octets), puis pour une erreur seulement, son code
//! (4 octets). Tout est en ordre réseau (gros-boutiste).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::{DoubleBufHelper, MessageDeserializeError};

const RENDEZVOUS: u8 = 0x00;
const CONNECT: u8 = 0x01;
const ERROR: u8 = 0x02;

/// Codes d'erreur du BEP 55.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum HolepunchError {
    /// L'intermédiaire ne connaît pas ce pair.
    NoSuchPeer = 1,
    /// L'intermédiaire n'est pas connecté à ce pair.
    NotConnected = 2,
    /// Ce pair ne gère pas le perçage.
    NoSupport = 3,
    /// Le demandeur a désigné l'intermédiaire lui-même.
    NoSelf = 4,
}

impl HolepunchError {
    fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::NoSuchPeer,
            2 => Self::NotConnected,
            3 => Self::NoSupport,
            4 => Self::NoSelf,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtHolepunch {
    /// Demande à l'intermédiaire de mettre en relation avec ce pair.
    Rendezvous(SocketAddr),
    /// L'intermédiaire annonce : se connecter maintenant à ce pair (qui fait de même).
    Connect(SocketAddr),
    /// L'intermédiaire refuse la mise en relation avec ce pair.
    Error(SocketAddr, HolepunchError),
}

impl UtHolepunch {
    fn parts(&self) -> (u8, SocketAddr, Option<HolepunchError>) {
        match *self {
            Self::Rendezvous(addr) => (RENDEZVOUS, addr, None),
            Self::Connect(addr) => (CONNECT, addr, None),
            Self::Error(addr, err) => (ERROR, addr, Some(err)),
        }
    }

    pub fn serialize(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let (kind, addr, err) = self.parts();
        out.write_all(&[kind])?;
        match addr.ip() {
            IpAddr::V4(v4) => {
                out.write_all(&[0])?;
                out.write_all(&v4.octets())?;
            }
            IpAddr::V6(v6) => {
                out.write_all(&[1])?;
                out.write_all(&v6.octets())?;
            }
        }
        out.write_all(&addr.port().to_be_bytes())?;
        if let Some(err) = err {
            out.write_all(&(err as u32).to_be_bytes())?;
        }
        Ok(())
    }

    pub fn deserialize(buf: DoubleBufHelper<'_>) -> Result<Self, MessageDeserializeError> {
        let bad = || MessageDeserializeError::UtHolepunchMalformed;
        let mut bytes = [0u8; 1 + 1 + 16 + 2 + 4];
        let len = buf.len();
        if len > bytes.len() {
            return Err(bad());
        }
        let [a, b] = buf.get();
        bytes[..a.len()].copy_from_slice(a);
        bytes[a.len()..a.len() + b.len()].copy_from_slice(b);
        let bytes = &bytes[..len];
        let (&kind, rest) = bytes.split_first().ok_or_else(bad)?;
        let (&family, rest) = rest.split_first().ok_or_else(bad)?;
        let (ip, rest): (IpAddr, &[u8]) = match family {
            0 => {
                let (ip, rest) = rest.split_at_checked(4).ok_or_else(bad)?;
                (
                    Ipv4Addr::from(<[u8; 4]>::try_from(ip).map_err(|_| bad())?).into(),
                    rest,
                )
            }
            1 => {
                let (ip, rest) = rest.split_at_checked(16).ok_or_else(bad)?;
                (
                    Ipv6Addr::from(<[u8; 16]>::try_from(ip).map_err(|_| bad())?).into(),
                    rest,
                )
            }
            _ => return Err(bad()),
        };
        let (port, rest) = rest.split_at_checked(2).ok_or_else(bad)?;
        let addr = SocketAddr::new(ip, u16::from_be_bytes([port[0], port[1]]));
        match (kind, rest.len()) {
            (RENDEZVOUS, 0) => Ok(Self::Rendezvous(addr)),
            (CONNECT, 0) => Ok(Self::Connect(addr)),
            (ERROR, 4) => {
                let code = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
                Ok(Self::Error(
                    addr,
                    HolepunchError::from_code(code).ok_or_else(bad)?,
                ))
            }
            _ => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(msg: UtHolepunch) -> Vec<u8> {
        let mut out = Vec::new();
        msg.serialize(&mut out).unwrap();
        for split in 0..=out.len() {
            let (a, b) = out.split_at(split);
            assert_eq!(
                UtHolepunch::deserialize(DoubleBufHelper::new(a, b)).unwrap(),
                msg
            );
        }
        out
    }

    /// Octets attendus d'après le BEP 55.
    #[test]
    fn wire_format_matches_bep_55() {
        let v4: SocketAddr = "1.2.3.4:6881".parse().unwrap();
        assert_eq!(
            round_trip(UtHolepunch::Rendezvous(v4)),
            [0, 0, 1, 2, 3, 4, 0x1a, 0xe1]
        );
        assert_eq!(
            round_trip(UtHolepunch::Connect(v4)),
            [1, 0, 1, 2, 3, 4, 0x1a, 0xe1]
        );
        assert_eq!(
            round_trip(UtHolepunch::Error(v4, HolepunchError::NotConnected)),
            [2, 0, 1, 2, 3, 4, 0x1a, 0xe1, 0, 0, 0, 2]
        );
        let v6: SocketAddr = "[2001:db8::1]:51413".parse().unwrap();
        let bytes = round_trip(UtHolepunch::Connect(v6));
        assert_eq!(bytes.len(), 1 + 1 + 16 + 2);
        assert_eq!(bytes[1], 1);
    }

    #[test]
    fn malformed_messages_are_errors() {
        for bytes in [
            &[][..],
            &[0],
            &[0, 0, 1, 2, 3],
            &[0, 2, 1, 2, 3, 4, 0, 1],             // famille inconnue
            &[9, 0, 1, 2, 3, 4, 0, 1],             // type inconnu
            &[2, 0, 1, 2, 3, 4, 0, 1],             // erreur sans code
            &[2, 0, 1, 2, 3, 4, 0, 1, 0, 0, 0, 9], // code inconnu
            &[0, 0, 1, 2, 3, 4, 0, 1, 7],          // octet en trop
        ] {
            assert!(
                UtHolepunch::deserialize(DoubleBufHelper::new(bytes, &[])).is_err(),
                "{bytes:?}"
            );
        }
        assert!(UtHolepunch::deserialize(DoubleBufHelper::new(&[0u8; 200], &[])).is_err());
    }
}
