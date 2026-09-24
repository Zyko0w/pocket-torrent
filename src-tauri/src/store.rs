//! Informations propres à Pocket Torrent sur chaque téléchargement (catégorie, priorité,
//! destination, raison d'une pause, envoi R2…), enregistrées dans `jobs.json`.
//! L'état des torrents eux-mêmes est gardé par librqbit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

impl Priority {
    pub fn rank(self) -> u8 {
        match self {
            Priority::High => 0,
            Priority::Normal => 1,
            Priority::Low => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dest {
    #[default]
    Local,
    R2,
}

/// Pourquoi un téléchargement est arrêté.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hold {
    /// Mis en pause par l'utilisateur.
    User,
    /// En attente d'une place dans la file.
    Queue,
    /// Mis en pause par le mode jeu, repris automatiquement ensuite.
    Game,
    /// Espace disque insuffisant.
    Disk,
    /// VPN exigé mais coupé : reprise automatique quand il revient.
    Vpn,
    /// Limite de partage (ratio ou durée) atteinte.
    Seeded,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Common {
    pub category: Option<String>,
    pub priority: Priority,
    pub dest: Dest,
    pub added_at: u64,
    pub finished_at: Option<u64>,
    pub hold: Option<Hold>,
    /// « Démarrer maintenant » : ignore la limite de la file d'attente.
    pub forced: bool,
    pub warning: Option<String>,
    pub error: Option<String>,
    pub error_notified: bool,
    pub delivery: Option<Delivery>,
}

impl Common {
    pub fn new(category: Option<String>, priority: Priority, dest: Dest) -> Self {
        Self { category, priority, dest, added_at: now_ms(), ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    Uploading,
    Done,
    Error,
}

/// Envoi sur Cloudflare R2.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub state: DeliveryState,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub links: Vec<Link>,
    /// Fin de validité des liens signés (absent avec une URL publique).
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub name: String,
    pub size: u64,
    pub key: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TorrentEntry {
    #[serde(flatten)]
    pub common: Common,
    /// Copie des infos du torrent, utile une fois retiré du moteur (après un envoi R2).
    pub name: Option<String>,
    pub wanted_bytes: u64,
    pub file_count: usize,
    pub selected_files: usize,
    pub folder: Option<PathBuf>,
    /// Faux une fois le torrent retiré du moteur (fichiers envoyés sur R2 puis effacés).
    pub removed: bool,
    /// Créé avec « Partager en torrent » depuis ce téléchargement direct.
    pub shared_from: Option<String>,
    /// Ordre de téléchargement des fichiers choisi par l'utilisateur (§4.1). Le moteur ne le
    /// garde que tant que le torrent tourne : réappliqué après une pause ou un redémarrage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_priorities: Option<Vec<usize>>,
    /// Fichier .torrent local d'où vient ce torrent (absent pour un magnet ou un lien http),
    /// proposé à la suppression avec le téléchargement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<PathBuf>,
}

/// Magnet en attente de ses métadonnées.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTorrent {
    pub id: String,
    pub link: String,
    pub name: Option<String>,
    pub only_files: Option<Vec<usize>>,
    #[serde(flatten)]
    pub common: Common,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectEntry {
    pub id: String,
    pub link: String,
    /// Type des entrées d'avant le retrait de 1fichier (`http` ou `1fichier`), absent des nouvelles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub limit_kbps: u32,
    pub name: Option<String>,
    pub total_bytes: u64,
    pub received_bytes: u64,
    pub folder: PathBuf,
    pub part_path: Option<PathBuf>,
    pub final_path: Option<PathBuf>,
    /// Torrent créé à partir de ce fichier.
    pub shared_hash: Option<String>,
    #[serde(flatten)]
    pub common: Common,
}

impl DirectEntry {
    /// Ancien téléchargement 1fichier : son lien mène à une page, plus à un fichier.
    pub fn is_legacy_1fichier(&self) -> bool {
        self.kind.as_deref() == Some("1fichier")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoreData {
    /// Clé : empreinte du torrent.
    pub torrents: BTreeMap<String, TorrentEntry>,
    pub pending: Vec<PendingTorrent>,
    pub directs: Vec<DirectEntry>,
    /// Statistiques cumulées (octets reçus et envoyés), voir `manager::stats`.
    pub totals: TrafficTotals,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrafficTotals {
    pub downloaded: u64,
    pub uploaded: u64,
    /// Début du décompte (ms depuis 1970), 0 tant que rien n'a été compté.
    pub since: u64,
}

impl StoreData {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::error!("jobs.json illisible ({err}), liste repartie de zéro");
                let _ = std::fs::copy(path, path.with_extension("json.illisible"));
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            // sur le disque avant le renommage : une coupure de courant (arrêt du PC programmé
            // compris) ne laisse jamais un jobs.json vide
            file.sync_all()?;
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn direct_mut(&mut self, id: &str) -> Option<&mut DirectEntry> {
        self.directs.iter_mut().find(|d| d.id == id)
    }

    pub fn pending_mut(&mut self, id: &str) -> Option<&mut PendingTorrent> {
        self.pending.iter_mut().find(|p| p.id == id)
    }
}
