//! Adresses, routes et métrique de la carte du tunnel.
//!
//! La route par défaut du tunnel a une métrique très haute : Windows continue d'utiliser la
//! connexion normale pour tout le reste. Seules les sockets liées explicitement à la carte
//! (IP_UNICAST_IF, adresse source du tunnel) l'empruntent.

use std::net::IpAddr;

use anyhow::bail;
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, ERROR_OBJECT_ALREADY_EXISTS, NO_ERROR, WIN32_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CreateIpForwardEntry2, CreateUnicastIpAddressEntry, GetIpInterfaceEntry, InitializeIpForwardEntry,
    InitializeIpInterfaceEntry, InitializeUnicastIpAddressEntry, MIB_IPFORWARD_ROW2, MIB_IPINTERFACE_ROW,
    MIB_UNICASTIPADDRESS_ROW, SetIpInterfaceEntry,
};
use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows_sys::Win32::Networking::WinSock::{ADDRESS_FAMILY, AF_INET, AF_INET6, IpDadStatePreferred, SOCKADDR_INET};

/// Métrique de la carte : bien au-dessus des cartes réelles (généralement 5 à 100).
const TUNNEL_METRIC: u32 = 5000;
/// MIB_IPPROTO_NETMGMT
const PROTOCOL_NETMGMT: i32 = 3;

fn check(code: WIN32_ERROR, what: &str) -> anyhow::Result<()> {
    if code == NO_ERROR || code == ERROR_OBJECT_ALREADY_EXISTS {
        return Ok(());
    }
    bail!("{what} : {}", std::io::Error::from_raw_os_error(code as i32))
}

fn sockaddr(ip: IpAddr) -> SOCKADDR_INET {
    // SAFETY: union remplie selon la famille d'adresse.
    unsafe {
        let mut out: SOCKADDR_INET = std::mem::zeroed();
        match ip {
            IpAddr::V4(v4) => {
                out.si_family = AF_INET;
                out.Ipv4.sin_addr.S_un.S_addr = u32::from(v4).to_be();
            }
            IpAddr::V6(v6) => {
                out.si_family = AF_INET6;
                out.Ipv6.sin6_addr.u.Byte = v6.octets();
            }
        }
        out
    }
}

pub fn add_address(luid: NET_LUID_LH, ip: IpAddr, prefix: u8) -> anyhow::Result<()> {
    // SAFETY: ligne initialisée par l'API puis complétée.
    unsafe {
        let mut row: MIB_UNICASTIPADDRESS_ROW = std::mem::zeroed();
        InitializeUnicastIpAddressEntry(&mut row);
        row.InterfaceLuid = luid;
        row.Address = sockaddr(ip);
        row.OnLinkPrefixLength = prefix;
        row.DadState = IpDadStatePreferred;
        check(CreateUnicastIpAddressEntry(&row), &format!("ajout de l'adresse {ip}/{prefix}"))
    }
}

/// Route par défaut (0.0.0.0/0 ou ::/0) vers le tunnel, sans passerelle.
pub fn add_default_route(luid: NET_LUID_LH, ipv6: bool) -> anyhow::Result<()> {
    let unspecified: IpAddr =
        if ipv6 { std::net::Ipv6Addr::UNSPECIFIED.into() } else { std::net::Ipv4Addr::UNSPECIFIED.into() };
    // SAFETY: ligne initialisée par l'API puis complétée.
    unsafe {
        let mut row: MIB_IPFORWARD_ROW2 = std::mem::zeroed();
        InitializeIpForwardEntry(&mut row);
        row.InterfaceLuid = luid;
        row.DestinationPrefix.Prefix = sockaddr(unspecified);
        row.DestinationPrefix.PrefixLength = 0;
        row.NextHop = sockaddr(unspecified);
        row.Metric = 0;
        row.Protocol = PROTOCOL_NETMGMT;
        check(CreateIpForwardEntry2(&row), "ajout de la route du tunnel")
    }
}

/// Métrique très haute (le tunnel n'attire pas le trafic du PC) et MTU éventuelle.
pub fn set_interface(luid: NET_LUID_LH, family: ADDRESS_FAMILY, mtu: Option<u16>) -> anyhow::Result<()> {
    // SAFETY: ligne initialisée par l'API, lue puis modifiée.
    unsafe {
        let mut row: MIB_IPINTERFACE_ROW = std::mem::zeroed();
        InitializeIpInterfaceEntry(&mut row);
        row.Family = family;
        row.InterfaceLuid = luid;
        let code = GetIpInterfaceEntry(&mut row);
        if code == ERROR_NOT_FOUND {
            return Ok(()); // IPv6 désactivé sur ce PC
        }
        check(code, "lecture de la carte du tunnel")?;
        row.UseAutomaticMetric = false;
        row.Metric = TUNNEL_METRIC;
        if let Some(mtu) = mtu {
            row.NlMtu = u32::from(mtu);
        }
        if family == AF_INET {
            // exigé par SetIpInterfaceEntry pour IPv4
            row.SitePrefixLength = 0;
        }
        check(SetIpInterfaceEntry(&mut row), "réglage de la carte du tunnel")
    }
}
