#[cfg(test)]
pub(crate) mod tests;

use crate::Error;
use std::{ffi::CString, num::NonZeroU32, str::FromStr};

#[derive(Debug, Clone)]
pub struct BindDevice {
    #[allow(unused)]
    index: NonZeroU32,
    #[allow(unused)]
    name: CString,
    /// Windows : adresses de l'interface, utilisées comme adresse source.
    #[cfg(windows)]
    ipv4: Option<std::net::Ipv4Addr>,
    #[cfg(windows)]
    ipv6: Option<std::net::Ipv6Addr>,
}

impl BindDevice {
    #[cfg(not(windows))]
    pub fn new_from_name(name: &str) -> crate::Result<Self> {
        let name = CString::new(name).map_err(|_| Error::BindDeviceInvalid)?;

        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        let index = NonZeroU32::new(index)
            .ok_or_else(|| Error::BindDeviceInvalidError(std::io::Error::last_os_error()))?;
        Ok(Self { index, name })
    }

    /// Windows : `name` est le nom de la carte réseau tel qu'affiché par Windows (ex. « WireGuard »).
    #[cfg(windows)]
    pub fn new_from_name(name: &str) -> crate::Result<Self> {
        let adapter = windows_impl::find_adapter(name)?;
        Ok(Self {
            index: adapter.index,
            name: CString::new(name).map_err(|_| Error::BindDeviceInvalid)?,
            ipv4: adapter.ipv4,
            ipv6: adapter.ipv6,
        })
    }

    pub fn index(&self) -> NonZeroU32 {
        self.index
    }

    pub fn name(&self) -> &str {
        // We constructed from a string so this can't fail
        unsafe { std::str::from_utf8_unchecked(self.name.to_bytes()) }
    }

    /// Adresse IPv4 de l'interface (Windows seulement).
    #[cfg(windows)]
    pub fn ipv4(&self) -> Option<std::net::Ipv4Addr> {
        self.ipv4
    }

    /// Adresse IPv6 globale de l'interface (Windows seulement).
    #[cfg(windows)]
    pub fn ipv6(&self) -> Option<std::net::Ipv6Addr> {
        self.ipv6
    }

    #[cfg(target_os = "macos")]
    pub fn bind_sref(&self, sref: &socket2::Socket, is_v6: bool) -> crate::Result<()> {
        if is_v6 {
            sref.bind_device_by_index_v6(Some(self.index))
                .map_err(Error::BindDeviceSetDeviceError)
        } else {
            sref.bind_device_by_index_v4(Some(self.index))
                .map_err(Error::BindDeviceSetDeviceError)
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn bind_sref(&self, sref: &socket2::Socket, _is_v6: bool) -> crate::Result<()> {
        let name = self.name.as_bytes_with_nul();
        sref.bind_device(Some(name))
            .map_err(Error::BindDeviceSetDeviceError)
    }

    /// Windows : IP_UNICAST_IF / IPV6_UNICAST_IF forcent l'interface de sortie.
    /// Une socket IPv6 double pile reçoit les deux options (le trafic IPv4 y passe aussi).
    #[cfg(windows)]
    pub fn bind_sref(&self, sref: &socket2::Socket, is_v6: bool) -> crate::Result<()> {
        use std::os::windows::io::AsRawSocket;
        let socket = sref.as_raw_socket() as usize;
        if is_v6 {
            windows_impl::set_unicast_if_v6(socket, self.index.get())
                .map_err(Error::BindDeviceSetDeviceError)?;
            // sans effet sur une socket IPv6 pure : l'erreur éventuelle est sans importance
            let _ = windows_impl::set_unicast_if_v4(socket, self.index.get());
            Ok(())
        } else {
            windows_impl::set_unicast_if_v4(socket, self.index.get())
                .map_err(Error::BindDeviceSetDeviceError)
        }
    }
}

impl FromStr for BindDevice {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new_from_name(s)
    }
}

#[cfg(windows)]
pub(crate) mod windows_impl {
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::num::NonZeroU32;

    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, IPPROTO_IP, IPPROTO_IPV6, SOCKADDR_IN, SOCKADDR_IN6,
        SOCKET_ERROR, WSAIoctl, setsockopt,
    };

    use crate::Error;

    const IP_UNICAST_IF: i32 = 31;
    const IPV6_UNICAST_IF: i32 = 31;
    /// _WSAIOW(IOC_VENDOR, 12)
    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;

    pub(crate) struct Adapter {
        pub index: NonZeroU32,
        pub ipv4: Option<Ipv4Addr>,
        pub ipv6: Option<Ipv6Addr>,
    }

    fn wide_to_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: chaîne terminée par un zéro fournie par GetAdaptersAddresses.
        unsafe {
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        }
    }

    /// Cherche une carte réseau active par son nom Windows.
    pub(crate) fn find_adapter(name: &str) -> crate::Result<Adapter> {
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
                code => {
                    return Err(Error::BindDeviceInvalidError(
                        std::io::Error::from_raw_os_error(code as i32),
                    ));
                }
            }
        }

        let mut current = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: liste chaînée remplie par GetAdaptersAddresses, valide tant que `buffer` vit.
        unsafe {
            while !current.is_null() {
                let adapter = &*current;
                if wide_to_string(adapter.FriendlyName).eq_ignore_ascii_case(name) {
                    if adapter.OperStatus != IfOperStatusUp {
                        return Err(Error::BindDeviceInvalidError(std::io::Error::other(
                            "la carte réseau est déconnectée",
                        )));
                    }
                    let index = NonZeroU32::new(adapter.Anonymous1.Anonymous.IfIndex)
                        .or_else(|| NonZeroU32::new(adapter.Ipv6IfIndex))
                        .ok_or(Error::BindDeviceInvalid)?;
                    let mut ipv4 = None;
                    let mut ipv6 = None;
                    let mut unicast = adapter.FirstUnicastAddress;
                    while !unicast.is_null() {
                        let sockaddr = (*unicast).Address.lpSockaddr;
                        if !sockaddr.is_null() {
                            match (*sockaddr).sa_family {
                                AF_INET if ipv4.is_none() => {
                                    let v4 = &*sockaddr.cast::<SOCKADDR_IN>();
                                    ipv4 =
                                        Some(Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr)));
                                }
                                AF_INET6 if ipv6.is_none() => {
                                    let v6 = &*sockaddr.cast::<SOCKADDR_IN6>();
                                    let ip = Ipv6Addr::from(v6.sin6_addr.u.Byte);
                                    // adresse globale seulement (pas fe80::)
                                    if (ip.segments()[0] & 0xffc0) != 0xfe80 {
                                        ipv6 = Some(ip);
                                    }
                                }
                                _ => {}
                            }
                        }
                        unicast = (*unicast).Next;
                    }
                    return Ok(Adapter { index, ipv4, ipv6 });
                }
                current = adapter.Next;
            }
        }
        Err(Error::BindDeviceInvalidError(std::io::Error::other(
            format!("carte réseau « {name} » introuvable"),
        )))
    }

    fn set_option(socket: usize, level: i32, option: i32, value: u32) -> std::io::Result<()> {
        // SAFETY: valeur DWORD de 4 octets passée par pointeur.
        let result = unsafe {
            setsockopt(
                socket,
                level,
                option,
                (&value as *const u32).cast::<u8>(),
                std::mem::size_of::<u32>() as i32,
            )
        };
        if result == SOCKET_ERROR {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// IPv4 : l'index est passé en ordre réseau.
    pub(crate) fn set_unicast_if_v4(socket: usize, index: u32) -> std::io::Result<()> {
        set_option(socket, IPPROTO_IP, IP_UNICAST_IF, index.to_be())
    }

    /// IPv6 : l'index est passé en ordre de l'hôte.
    pub(crate) fn set_unicast_if_v6(socket: usize, index: u32) -> std::io::Result<()> {
        set_option(socket, IPPROTO_IPV6, IPV6_UNICAST_IF, index)
    }

    /// Désactive le signalement des « port injoignable » ICMP sur une socket UDP : sans ça, Windows
    /// fait échouer le recv_from suivant avec l'erreur 10054 et arrête DHT et uTP.
    pub(crate) fn disable_udp_connreset(socket: usize) -> std::io::Result<()> {
        let enabled: u32 = 0;
        let mut returned: u32 = 0;
        // SAFETY: entrée BOOL de 4 octets, pas de sortie.
        let result = unsafe {
            WSAIoctl(
                socket,
                SIO_UDP_CONNRESET,
                (&enabled as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
                None,
            )
        };
        if result == SOCKET_ERROR {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
