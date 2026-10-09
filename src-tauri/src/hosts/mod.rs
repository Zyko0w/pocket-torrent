//! Hébergeurs de fichiers pour l'envoi en ligne des téléchargements terminés.
//!
//! Chaque service est un module qui implémente [`FileHost`] ; [`Host`] les réunit, et le reste
//! de l'application (envoi, suppression, bouton « Tester ») ne passe que par lui. Pour brancher
//! un nouveau service :
//! 1. un module `hosts/<service>.rs` qui implémente `FileHost` (aides communes dans `http.rs`) ;
//! 2. une valeur dans `settings::UploadHost` (nom enregistré et libellé) ;
//! 3. une variante de [`Host`], ouverte dans [`Host::open`] et ajoutée à la macro `dispatch!` ;
//! 4. si le service a un compte : une clé dans `secrets::Secret`, et son champ dans les réglages ;
//! 5. côté interface : une entrée dans `UPLOAD_HOSTS` (src/uploadHosts.ts).
//!
//! Sans toucher au code, l'utilisateur peut aussi ajouter son propre hébergeur (`custom.rs`).

pub mod custom;
pub mod gofile;
mod http;
pub mod storage_to;
pub mod vikingfile;

use std::path::Path;

use anyhow::Context;

use crate::settings::{StorageToSettings, UploadHost};

use custom::Custom;
use gofile::Gofile;
use storage_to::StorageTo;
use vikingfile::Vikingfile;

/// Fichier ou groupe de fichiers mis en ligne : lien de partage et preuve de propriété.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hosted {
    pub id: String,
    pub url: String,
    /// Ce qui permet de le supprimer ensuite (vide si le compte des réglages suffit, ou si le
    /// service ne le permet pas). Gardé avec le lien, jamais envoyé à l'interface.
    pub owner_token: String,
}

/// Résultat de l'envoi d'un fichier.
#[derive(Debug, Default)]
pub struct Uploaded {
    /// Groupe créé pour ce téléchargement (au premier fichier) : un seul lien pour le tout.
    pub group: Option<Hosted>,
    /// Lien propre au fichier (aucun si le service ne donne que celui du groupe).
    pub file: Option<Hosted>,
}

/// Genre d'un lien enregistré : un fichier, ou le groupe de tous les fichiers d'un téléchargement.
pub const FILE: &str = "file";
pub const GROUP: &str = "collection";

/// Ce que chaque service d'hébergement doit savoir faire.
// Futures utilisées seulement à travers `Host` (types connus) : elles restent `Send`.
#[allow(async_fn_in_trait)]
pub trait FileHost {
    /// Vérifie l'accès (bouton « Tester ») et renvoie une phrase pour l'utilisateur.
    async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String>;

    /// Envoie un fichier d'un téléchargement qui en compte `file_count`. `group` : groupe créé
    /// par un fichier précédent du même envoi. `on_progress` : octets de ce fichier déjà envoyés.
    async fn upload(
        &self,
        client: &reqwest::Client,
        path: &Path,
        file_name: &str,
        file_count: usize,
        group: Option<&Hosted>,
        on_progress: &mut (dyn FnMut(u64) + Send),
    ) -> anyhow::Result<Uploaded>;

    /// Supprime un fichier ou un groupe (`kind` : [`FILE`] ou [`GROUP`]). Déjà absent : succès.
    async fn delete(&self, client: &reqwest::Client, kind: &str, id: &str, owner_token: &str) -> anyhow::Result<()>;

    /// Les fichiers envoyés maintenant pourront être supprimés depuis Pocket Torrent.
    fn can_delete(&self) -> bool {
        true
    }

    /// Durée de vie des liens, en jours ; `None` : le service les efface lui-même (inactivité).
    fn expiry_days(&self) -> Option<u8> {
        None
    }

    /// Taille maximale d'un fichier envoyé ; `None` : pas de limite connue.
    fn max_file_bytes(&self) -> Option<u64> {
        None
    }
}

/// Service choisi, prêt à l'emploi.
pub enum Host {
    StorageTo(StorageTo),
    Gofile(Gofile),
    Vikingfile(Vikingfile),
    Custom(Custom),
}

/// Appelle la même méthode du trait sur le service, quel qu'il soit (appel explicite : un service
/// peut avoir ses propres méthodes du même nom).
macro_rules! dispatch {
    ($self:ident, $host:ident => $call:expr) => {
        match $self {
            Host::StorageTo($host) => $call,
            Host::Gofile($host) => $call,
            Host::Vikingfile($host) => $call,
            Host::Custom($host) => $call,
        }
    };
}

impl Host {
    /// `limits` : durée de vie (jours) et limite de téléchargements choisies pour cet envoi, à la
    /// place des réglages (services qui les proposent seulement).
    pub fn open(kind: &UploadHost, settings: &StorageToSettings, limits: Option<(u8, u16)>) -> anyhow::Result<Self> {
        Ok(match kind {
            UploadHost::StorageTo => {
                let storage = StorageTo::from_settings(settings)?;
                Host::StorageTo(match limits {
                    Some((days, downloads)) => storage.with_limits(days, downloads),
                    None => storage,
                })
            }
            UploadHost::Gofile => Host::Gofile(Gofile::from_secrets()),
            UploadHost::Vikingfile => Host::Vikingfile(Vikingfile::from_secrets()),
            UploadHost::Custom(id) => {
                let config = settings
                    .custom_hosts
                    .iter()
                    .find(|h| &h.id == id)
                    .context("hébergeur personnalisé retiré des réglages")?;
                Host::Custom(Custom::open(config)?)
            }
        })
    }

    /// Nom affiché du service.
    pub fn name(&self) -> String {
        match self {
            Host::StorageTo(_) => UploadHost::StorageTo.label().to_owned(),
            Host::Gofile(_) => UploadHost::Gofile.label().to_owned(),
            Host::Vikingfile(_) => UploadHost::Vikingfile.label().to_owned(),
            Host::Custom(custom) => custom.name().to_owned(),
        }
    }

    pub async fn check(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        dispatch!(self, host => FileHost::check(host, client).await)
    }

    pub async fn upload(
        &self,
        client: &reqwest::Client,
        path: &Path,
        file_name: &str,
        file_count: usize,
        group: Option<&Hosted>,
        on_progress: &mut (dyn FnMut(u64) + Send),
    ) -> anyhow::Result<Uploaded> {
        dispatch!(self, host => FileHost::upload(host, client, path, file_name, file_count, group, on_progress).await)
    }

    pub async fn delete(
        &self,
        client: &reqwest::Client,
        kind: &str,
        id: &str,
        owner_token: &str,
    ) -> anyhow::Result<()> {
        if !matches!(kind, FILE | GROUP) {
            anyhow::bail!("genre de lien inconnu");
        }
        dispatch!(self, host => FileHost::delete(host, client, kind, id, owner_token).await)
    }

    pub fn can_delete(&self) -> bool {
        dispatch!(self, host => FileHost::can_delete(host))
    }

    pub fn expiry_days(&self) -> Option<u8> {
        dispatch!(self, host => FileHost::expiry_days(host))
    }

    pub fn max_file_bytes(&self) -> Option<u64> {
        dispatch!(self, host => FileHost::max_file_bytes(host))
    }
}
