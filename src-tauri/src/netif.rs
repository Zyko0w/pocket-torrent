//! Cartes réseau de Windows : liste, état et repérage des cartes de VPN.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    /// Nom affiché par Windows (ex. « Wi-Fi », « NordLynx »).
    pub name: String,
    pub description: String,
    pub up: bool,
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6: Option<Ipv6Addr>,
    /// Ressemble à une carte de VPN (tunnel, WireGuard, OpenVPN…).
    pub likely_vpn: bool,
}

impl Adapter {
    pub fn usable(&self) -> bool {
        self.up && (self.ipv4.is_some() || self.ipv6.is_some())
    }

    pub fn source_ip(&self) -> Option<IpAddr> {
        self.ipv4.map(IpAddr::V4).or(self.ipv6.map(IpAddr::V6))
    }
}

/// Mots qui trahissent une carte de VPN dans sa description ou son nom.
const VPN_HINTS: &[&str] = &[
    "wireguard",
    "wintun",
    "tap-windows",
    "tap-",
    "openvpn",
    "nordlynx",
    "nordvpn",
    "proton",
    "mullvad",
    "private internet access",
    "surfshark",
    "expressvpn",
    "cyberghost",
    "windscribe",
    "ivpn",
    "hotspot shield",
    "cisco anyconnect",
    "fortinet",
    "sonicwall",
    "zerotier",
    "tailscale",
    "softether",
    "vpn",
    "tunnel",
];

pub fn looks_like_vpn(name: &str, description: &str, if_type: u32) -> bool {
    // 53 : IF_TYPE_PROP_VIRTUAL, 131 : IF_TYPE_TUNNEL, 23 : IF_TYPE_PPP
    let text = format!("{name} {description}").to_lowercase();
    // les cartes virtuelles de Hyper-V, VirtualBox ou VMware ne sont pas des VPN
    if ["hyper-v", "virtualbox", "vmware", "loopback", "bluetooth", "wi-fi direct"].iter().any(|w| text.contains(w)) {
        return false;
    }
    matches!(if_type, 23 | 53 | 131) || VPN_HINTS.iter().any(|hint| text.contains(hint))
}

pub fn find(name: &str) -> Option<Adapter> {
    list().into_iter().find(|a| a.name.eq_ignore_ascii_case(name))
}

pub use imp::list;

#[cfg(windows)]
mod imp {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};

    use super::{Adapter, looks_like_vpn};

    fn wide(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: chaîne terminée par un zéro fournie par GetAdaptersAddresses.
        unsafe {
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        }
    }

    pub fn list() -> Vec<Adapter> {
        const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 32_000;
        let mut buffer: Vec<u8>;
        loop {
            buffer = vec![0u8; size as usize];
            // SAFETY: tampon de `size` octets.
            let result = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC as u32,
                    flags,
                    std::ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            match result {
                NO_ERROR => break,
                ERROR_BUFFER_OVERFLOW => continue,
                _ => return Vec::new(),
            }
        }

        let mut adapters = Vec::new();
        let mut current = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: liste chaînée valide tant que `buffer` vit.
        unsafe {
            while !current.is_null() {
                let a = &*current;
                current = a.Next;
                if a.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
                    continue;
                }
                let name = wide(a.FriendlyName);
                let description = wide(a.Description);
                let mut ipv4 = None;
                let mut ipv6 = None;
                let mut unicast = a.FirstUnicastAddress;
                while !unicast.is_null() {
                    let sockaddr = (*unicast).Address.lpSockaddr;
                    if !sockaddr.is_null() {
                        match (*sockaddr).sa_family {
                            AF_INET if ipv4.is_none() => {
                                let v4 = &*sockaddr.cast::<SOCKADDR_IN>();
                                let ip = Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr));
                                // 169.254.x.x : pas de vraie adresse (DHCP absent)
                                if !ip.is_link_local() {
                                    ipv4 = Some(ip);
                                }
                            }
                            AF_INET6 if ipv6.is_none() => {
                                let v6 = &*sockaddr.cast::<SOCKADDR_IN6>();
                                let ip = Ipv6Addr::from(v6.sin6_addr.u.Byte);
                                if (ip.segments()[0] & 0xffc0) != 0xfe80 {
                                    ipv6 = Some(ip);
                                }
                            }
                            _ => {}
                        }
                    }
                    unicast = (*unicast).Next;
                }
                adapters.push(Adapter {
                    likely_vpn: looks_like_vpn(&name, &description, a.IfType),
                    name,
                    description,
                    up: a.OperStatus == IfOperStatusUp,
                    ipv4,
                    ipv6,
                });
            }
        }
        adapters
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn list() -> Vec<super::Adapter> {
        Vec::new()
    }
}
