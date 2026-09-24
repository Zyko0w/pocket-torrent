//! Tunnel WireGuard intégré de Pocket Torrent.
//!
//! Créer une carte réseau WireGuard demande les droits administrateur : un petit service Windows
//! (`pocket-torrent-tunnel.exe service`) s'en charge. L'application lui parle par un canal nommé
//! réservé à l'utilisateur qui l'a installé.
//!
//! Le tunnel ne change pas la route par défaut de Windows : il ajoute sa route avec une métrique
//! très haute. Seuls les logiciels qui se lient explicitement à la carte (Pocket Torrent) l'utilisent.

pub mod config;
pub mod protocol;

#[cfg(windows)]
pub mod client;
