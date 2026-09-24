//! Appels au pilote WireGuard-NT (wireguard.dll, placée à côté du service).

use std::ffi::c_void;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use anyhow::{Context, bail};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
use windows_sys::core::GUID;

use pocket_torrent_tunnel::config::Config;

type Handle = *mut c_void;

const INTERFACE_HAS_PRIVATE_KEY: i32 = 1 << 1;
const INTERFACE_HAS_LISTEN_PORT: i32 = 1 << 2;
const INTERFACE_REPLACE_PEERS: i32 = 1 << 3;
const PEER_HAS_PUBLIC_KEY: i32 = 1 << 0;
const PEER_HAS_PRESHARED_KEY: i32 = 1 << 1;
const PEER_HAS_PERSISTENT_KEEPALIVE: i32 = 1 << 2;
const PEER_HAS_ENDPOINT: i32 = 1 << 3;
const PEER_REPLACE_ALLOWED_IPS: i32 = 1 << 5;
const ADAPTER_STATE_DOWN: i32 = 0;
const ADAPTER_STATE_UP: i32 = 1;

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct WgInterface {
    flags: i32,
    listen_port: u16,
    private_key: [u8; 32],
    public_key: [u8; 32],
    peers_count: u32,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct WgPeer {
    flags: i32,
    reserved: u32,
    public_key: [u8; 32],
    preshared_key: [u8; 32],
    persistent_keepalive: u16,
    endpoint: SOCKADDR_INET,
    tx_bytes: u64,
    rx_bytes: u64,
    last_handshake: u64,
    allowed_ips_count: u32,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct WgAllowedIp {
    address: [u8; 16],
    address_family: u16,
    cidr: u8,
    flags: i32,
}

// tailles imposées par wireguard.h
const _: () = assert!(std::mem::size_of::<WgInterface>() == 80);
const _: () = assert!(std::mem::size_of::<WgPeer>() == 136);
const _: () = assert!(std::mem::size_of::<WgAllowedIp>() == 24);

/// GUID fixe : Windows garde le même profil réseau d'un lancement à l'autre.
const ADAPTER_GUID: GUID = GUID::from_u128(0x7c3aed00_7a1c_4c1a_9f1e_746f7272656e);

// signatures de wireguard.h
type CreateAdapterFn = unsafe extern "system" fn(*const u16, *const u16, *const GUID) -> Handle;
type CloseAdapterFn = unsafe extern "system" fn(Handle);
type GetAdapterLuidFn = unsafe extern "system" fn(Handle, *mut NET_LUID_LH);
type SetConfigurationFn = unsafe extern "system" fn(Handle, *const c_void, u32) -> i32;
type GetConfigurationFn = unsafe extern "system" fn(Handle, *mut c_void, *mut u32) -> i32;
type SetAdapterStateFn = unsafe extern "system" fn(Handle, i32) -> i32;
/// Type renvoyé par `GetProcAddress`, avant conversion vers la vraie signature.
type RawProc = unsafe extern "system" fn() -> isize;

struct Functions {
    create_adapter: CreateAdapterFn,
    close_adapter: CloseAdapterFn,
    get_adapter_luid: GetAdapterLuidFn,
    set_configuration: SetConfigurationFn,
    get_configuration: GetConfigurationFn,
    set_adapter_state: SetAdapterStateFn,
}

pub struct Driver {
    module: HMODULE,
    functions: Functions,
}

// SAFETY: les fonctions de wireguard.dll sont utilisables depuis n'importe quel fil d'exécution.
unsafe impl Send for Driver {}
unsafe impl Sync for Driver {}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Driver {
    pub fn load(dll: &Path) -> anyhow::Result<Self> {
        let path = wide(&dll.to_string_lossy());
        // SAFETY: chemin complet terminé par un zéro.
        let module = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if module.is_null() {
            bail!("wireguard.dll introuvable ou invalide ({}) : {}", dll.display(), std::io::Error::last_os_error());
        }
        macro_rules! function {
            ($name:literal, $signature:ty) => {{
                // SAFETY: nom terminé par un zéro.
                let pointer = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) }
                    .with_context(|| format!("fonction {} absente de wireguard.dll", $name))?;
                // SAFETY: d'un pointeur de fonction à un autre ; `$signature` est celle que
                // wireguard.h donne à cette fonction.
                unsafe { std::mem::transmute::<RawProc, $signature>(pointer) }
            }};
        }
        let functions = Functions {
            create_adapter: function!("WireGuardCreateAdapter", CreateAdapterFn),
            close_adapter: function!("WireGuardCloseAdapter", CloseAdapterFn),
            get_adapter_luid: function!("WireGuardGetAdapterLUID", GetAdapterLuidFn),
            set_configuration: function!("WireGuardSetConfiguration", SetConfigurationFn),
            get_configuration: function!("WireGuardGetConfiguration", GetConfigurationFn),
            set_adapter_state: function!("WireGuardSetAdapterState", SetAdapterStateFn),
        };
        Ok(Self { module, functions })
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        // SAFETY: module chargé par LoadLibraryExW.
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

pub struct Adapter {
    driver: std::sync::Arc<Driver>,
    handle: Handle,
}

// SAFETY: la poignée d'une carte WireGuard est utilisable depuis n'importe quel fil ; le service
// n'y accède que sous verrou.
unsafe impl Send for Adapter {}

impl Adapter {
    pub fn create(driver: std::sync::Arc<Driver>, name: &str) -> anyhow::Result<Self> {
        let name_w = wide(name);
        let tunnel_type = wide("Pocket Torrent");
        // SAFETY: chaînes terminées par un zéro, GUID valide.
        let handle = unsafe { (driver.functions.create_adapter)(name_w.as_ptr(), tunnel_type.as_ptr(), &ADAPTER_GUID) };
        if handle.is_null() {
            bail!("création de la carte WireGuard impossible : {}", std::io::Error::last_os_error());
        }
        Ok(Self { driver, handle })
    }

    pub fn luid(&self) -> NET_LUID_LH {
        let mut luid: NET_LUID_LH = unsafe { std::mem::zeroed() };
        // SAFETY: poignée valide, pointeur de sortie valide.
        unsafe { (self.driver.functions.get_adapter_luid)(self.handle, &mut luid) };
        luid
    }

    pub fn set_up(&self, up: bool) -> anyhow::Result<()> {
        let state = if up { ADAPTER_STATE_UP } else { ADAPTER_STATE_DOWN };
        // SAFETY: poignée valide.
        if unsafe { (self.driver.functions.set_adapter_state)(self.handle, state) } == 0 {
            bail!("changement d'état du tunnel impossible : {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Envoie clés, serveurs et adresses autorisées au pilote.
    pub fn configure(&self, config: &Config, endpoints: &[SocketAddr]) -> anyhow::Result<()> {
        let allowed_total: usize = config.peers.iter().map(|p| p.allowed_ips.len()).sum();
        let bytes = std::mem::size_of::<WgInterface>()
            + config.peers.len() * std::mem::size_of::<WgPeer>()
            + allowed_total * std::mem::size_of::<WgAllowedIp>();
        // mémoire alignée sur 8 octets comme l'exige le pilote
        let mut buffer = vec![0u64; bytes.div_ceil(8)];
        let base = buffer.as_mut_ptr().cast::<u8>();
        let mut offset = 0usize;

        let mut flags = INTERFACE_HAS_PRIVATE_KEY | INTERFACE_REPLACE_PEERS;
        if config.listen_port.is_some() {
            flags |= INTERFACE_HAS_LISTEN_PORT;
        }
        let interface = WgInterface {
            flags,
            listen_port: config.listen_port.unwrap_or(0),
            private_key: config.private_key,
            public_key: [0; 32],
            peers_count: config.peers.len() as u32,
        };
        // SAFETY: écritures dans les limites du tampon calculé ci-dessus.
        unsafe {
            base.add(offset).cast::<WgInterface>().write(interface);
            offset += std::mem::size_of::<WgInterface>();
            for (peer, endpoint) in config.peers.iter().zip(endpoints) {
                let mut flags = PEER_HAS_PUBLIC_KEY | PEER_HAS_ENDPOINT | PEER_REPLACE_ALLOWED_IPS;
                if peer.preshared_key.is_some() {
                    flags |= PEER_HAS_PRESHARED_KEY;
                }
                if peer.persistent_keepalive.is_some() {
                    flags |= PEER_HAS_PERSISTENT_KEEPALIVE;
                }
                let wg_peer = WgPeer {
                    flags,
                    reserved: 0,
                    public_key: peer.public_key,
                    preshared_key: peer.preshared_key.unwrap_or([0; 32]),
                    persistent_keepalive: peer.persistent_keepalive.unwrap_or(0),
                    endpoint: sockaddr_inet(*endpoint),
                    tx_bytes: 0,
                    rx_bytes: 0,
                    last_handshake: 0,
                    allowed_ips_count: peer.allowed_ips.len() as u32,
                };
                base.add(offset).cast::<WgPeer>().write(wg_peer);
                offset += std::mem::size_of::<WgPeer>();
                for (ip, cidr) in &peer.allowed_ips {
                    let mut address = [0u8; 16];
                    let family = match ip {
                        IpAddr::V4(v4) => {
                            address[..4].copy_from_slice(&v4.octets());
                            AF_INET
                        }
                        IpAddr::V6(v6) => {
                            address.copy_from_slice(&v6.octets());
                            AF_INET6
                        }
                    };
                    base.add(offset).cast::<WgAllowedIp>().write(WgAllowedIp {
                        address,
                        address_family: family,
                        cidr: *cidr,
                        flags: 0,
                    });
                    offset += std::mem::size_of::<WgAllowedIp>();
                }
            }
            if (self.driver.functions.set_configuration)(self.handle, buffer.as_ptr().cast(), bytes as u32) == 0 {
                bail!("configuration du tunnel refusée : {}", std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Octets reçus et envoyés, et dernière poignée de main (100 ns depuis 1601) du premier serveur.
    pub fn statistics(&self) -> Option<(u64, u64, u64)> {
        let mut size: u32 = 4096;
        loop {
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: tampon aligné de `size` octets au moins.
            let ok = unsafe {
                (self.driver.functions.get_configuration)(self.handle, buffer.as_mut_ptr().cast(), &mut size)
            };
            if ok == 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(234) {
                    continue; // ERROR_MORE_DATA : `size` a été mis à jour
                }
                return None;
            }
            // SAFETY: le pilote a rempli une interface suivie de ses pairs.
            unsafe {
                let base = buffer.as_ptr().cast::<u8>();
                let interface = base.cast::<WgInterface>().read();
                if interface.peers_count == 0 {
                    return Some((0, 0, 0));
                }
                let peer = base.add(std::mem::size_of::<WgInterface>()).cast::<WgPeer>().read();
                return Some((peer.rx_bytes, peer.tx_bytes, peer.last_handshake));
            }
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // SAFETY: poignée créée par WireGuardCreateAdapter ; la fermer supprime la carte.
        unsafe { (self.driver.functions.close_adapter)(self.handle) };
    }
}

fn sockaddr_inet(addr: SocketAddr) -> SOCKADDR_INET {
    // SAFETY: union remplie selon la famille d'adresse.
    unsafe {
        let mut out: SOCKADDR_INET = std::mem::zeroed();
        match addr {
            SocketAddr::V4(v4) => {
                out.Ipv4.sin_family = AF_INET;
                out.Ipv4.sin_port = v4.port().to_be();
                out.Ipv4.sin_addr.S_un.S_addr = u32::from(*v4.ip()).to_be();
            }
            SocketAddr::V6(v6) => {
                out.Ipv6.sin6_family = AF_INET6;
                out.Ipv6.sin6_port = v6.port().to_be();
                out.Ipv6.sin6_addr.u.Byte = v6.ip().octets();
            }
        }
        out
    }
}
