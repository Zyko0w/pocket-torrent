//! Informations propres à Pocket Torrent sur chaque téléchargement (catégorie, priorité,
//! destination, raison d'une pause, envoi en ligne…), enregistrées dans `jobs.json`.
//! L'état des torrents eux-mêmes est gardé par librqbit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Lecture d'un fichier JSON abîmé : valeurs par défaut, remplacées par chaque valeur
/// enregistrée qui reste valide (champ par champ ; pour une liste, élément par élément).
/// Un seul champ ou un seul téléchargement illisible ne fait plus tout perdre.
pub fn salvage<T: Default + Serialize + serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    use serde_json::Value;
    fn leaves(defaults: &Value, path: Vec<String>, saved: Value, out: &mut Vec<(Vec<String>, Value)>) {
        match (defaults, saved) {
            (Value::Object(defaults), Value::Object(saved)) => {
                for (key, value) in saved {
                    let mut path = path.clone();
                    path.push(key.clone());
                    match defaults.get(&key) {
                        Some(default) => leaves(default, path, value, out),
                        None => out.push((path, value)),
                    }
                }
            }
            (_, saved) => out.push((path, saved)),
        }
    }
    let valid = |candidate: &Value| serde_json::from_value::<T>(candidate.clone()).is_ok();
    // le chemin ne traverse que des objets (ceux des valeurs par défaut)
    let slot = |root: &mut Value, path: &[String], value: Value| {
        *path.iter().fold(root, |node, key| &mut node[key.as_str()]) = value;
    };
    let Ok(saved) = serde_json::from_slice::<Value>(bytes) else { return T::default() };
    let Ok(mut kept) = serde_json::to_value(T::default()) else { return T::default() };
    let mut found = Vec::new();
    leaves(&kept, Vec::new(), saved, &mut found);
    for (path, value) in found {
        let mut candidate = kept.clone();
        slot(&mut candidate, &path, value.clone());
        if valid(&candidate) {
            kept = candidate;
        } else if let Value::Array(items) = value {
            let mut good = Vec::new();
            for item in items {
                good.push(item);
                let mut candidate = kept.clone();
                slot(&mut candidate, &path, Value::Array(good.clone()));
                if !valid(&candidate) {
                    good.pop();
                }
            }
            slot(&mut kept, &path, Value::Array(good));
            if !valid(&kept) {
                slot(&mut kept, &path, Value::Array(Vec::new()));
            }
        }
    }
    serde_json::from_value(kept).unwrap_or_default()
}

/// Écrit un fichier sans jamais laisser de version à moitié écrite : fichier temporaire mis sur
/// le disque, puis renommé. Une coupure de courant (arrêt du PC programmé compris) laisse
/// l'ancienne version ou la nouvelle, jamais un fichier vide.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Dest {
    /// Garder sur le PC.
    #[default]
    Local,
    /// Envoyer sur storage.to une fois terminé.
    StorageTo,
}

/// Une destination inconnue devient « garder sur le PC » : rien n'est envoyé en ligne sans que
/// l'utilisateur l'ait choisi.
impl<'de> Deserialize<'de> for Dest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "storageTo" => Dest::StorageTo,
            _ => Dest::Local,
        })
    }
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
    /// Analyse Microsoft Defender du téléchargement terminé (option des réglages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
}

/// Résultat de l'analyse Microsoft Defender d'un téléchargement terminé, affiché sous celui-ci.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Scan {
    /// En cours (jamais conservé d'un lancement à l'autre).
    Running,
    Clean,
    Threat {
        threats: Vec<String>,
    },
    /// Defender absent, désactivé ou remplacé par un autre antivirus.
    Unavailable,
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

/// Envoi en ligne (storage.to) une fois le téléchargement fini.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub state: DeliveryState,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub links: Vec<Link>,
    /// Fin de validité des liens (les fichiers disparaissent ensuite de storage.to).
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub name: String,
    pub size: u64,
    /// Identifiant storage.to du fichier ou de la collection.
    pub key: String,
    pub url: String,
    /// « file » ou « collection » (tous les fichiers du téléchargement sous un seul lien).
    #[serde(default = "file_kind")]
    pub kind: String,
    /// Preuve de propriété renvoyée par storage.to, pour supprimer le fichier. Jamais envoyée à
    /// l'interface (retirée par `overview`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner_token: String,
}

fn file_kind() -> String {
    "file".into()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TorrentEntry {
    #[serde(flatten)]
    pub common: Common,
    /// Copie des infos du torrent, utile une fois retiré du moteur (après un envoi en ligne).
    pub name: Option<String>,
    pub wanted_bytes: u64,
    pub file_count: usize,
    pub selected_files: usize,
    pub folder: Option<PathBuf>,
    /// Faux une fois le torrent retiré du moteur (fichiers envoyés en ligne puis effacés).
    pub removed: bool,
    /// Ordre de téléchargement des fichiers choisi par l'utilisateur (§4.1). Le moteur ne le
    /// garde que tant que le torrent tourne : réappliqué après une pause ou un redémarrage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_priorities: Option<Vec<usize>>,
    /// Vitesse de réception maximale choisie pour ce torrent, en Ko/s (0 = aucune limite propre).
    pub limit_kbps: u32,
    /// Fichier .torrent local d'où vient ce torrent (absent pour un magnet ou un lien http),
    /// proposé à la suppression avec le téléchargement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<PathBuf>,
    /// Téléchargé dans le dossier « en cours » (`incomplete_folder`) : dossier où le déplacer une
    /// fois terminé. Retiré après le déplacement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_folder: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_folder: Option<PathBuf>,
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
    pub limit_kbps: u32,
    pub name: Option<String>,
    pub total_bytes: u64,
    pub received_bytes: u64,
    pub folder: PathBuf,
    /// Dossier du fichier « .part » s'il diffère de `folder` (dossier « en cours ») : le
    /// fichier terminé est ensuite déplacé dans `folder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_folder: Option<PathBuf>,
    pub part_path: Option<PathBuf>,
    pub final_path: Option<PathBuf>,
    #[serde(flatten)]
    pub common: Common,
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
        let Ok(bytes) = std::fs::read(path) else { return Self::default() };
        serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::error!(
                "jobs.json illisible ({err}) : téléchargements valides repris, copie dans jobs.json.illisible"
            );
            let _ = std::fs::copy(path, path.with_extension("json.illisible"));
            salvage(&bytes)
        })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        write_atomic(path, &serde_json::to_vec(self)?)
    }

    pub fn direct_mut(&mut self, id: &str) -> Option<&mut DirectEntry> {
        self.directs.iter_mut().find(|d| d.id == id)
    }

    pub fn pending_mut(&mut self, id: &str) -> Option<&mut PendingTorrent> {
        self.pending.iter_mut().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod dest_tests {
    use super::Dest;

    #[test]
    fn unknown_destinations_fall_back_to_the_pc() {
        let parse = |text: &str| serde_json::from_str::<Dest>(text).unwrap();
        assert_eq!(parse("\"storageTo\""), Dest::StorageTo);
        assert_eq!(parse("\"local\""), Dest::Local);
        assert_eq!(parse("\"inconnue\""), Dest::Local, "destination inconnue : aucun envoi");
        assert_eq!(serde_json::to_string(&Dest::StorageTo).unwrap(), "\"storageTo\"");
    }
}

#[cfg(test)]
mod salvage_tests {
    use super::StoreData;

    /// jobs.json abîmé (bac à sable : dossier temporaire dédié) : seuls les éléments illisibles
    /// sont perdus, une copie du fichier est gardée.
    #[test]
    fn one_damaged_download_does_not_lose_the_whole_list() {
        let dir = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jobs.json");
        let good_direct = |id: &str| {
            serde_json::json!({
                "id": id, "link": "https://exemple.org/a.iso", "limitKbps": 0, "name": null, "totalBytes": 0,
                "receivedBytes": 0, "folder": "C:\\x", "partPath": null, "finalPath": null,
            })
        };
        let json = serde_json::json!({
            "torrents": { "aaaa": {}, "bbbb": 42, "cccc": {} },
            "directs": [good_direct("d1"), { "id": [1, 2] }, good_direct("d3")],
            "totals": { "downloaded": 5, "uploaded": "beaucoup", "since": 1 },
        });
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

        let data = StoreData::load(&path);
        assert_eq!(data.torrents.keys().collect::<Vec<_>>(), ["aaaa", "cccc"]);
        assert_eq!(data.directs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1", "d3"]);
        assert_eq!((data.totals.downloaded, data.totals.uploaded, data.totals.since), (5, 0, 1));
        assert!(dir.join("jobs.json.illisible").exists());

        for bytes in [&b"{\"torrents\": {\"aa"[..], &[0xff, 0x00], b"[]", b""] {
            std::fs::write(&path, bytes).unwrap();
            let _ = StoreData::load(&path);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod write_tests {
    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("pocket-torrent-sandbox-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("reglages.json");
        super::write_atomic(&path, b"{\"a\":1}").unwrap();
        super::write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "aucun fichier temporaire laissé");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod scan_tests {
    use super::Scan;

    /// Forme lue par l'interface (`src/api.ts`, type `Scan`).
    #[test]
    fn scan_results_have_the_shape_the_interface_reads() {
        let json = |scan: &Scan| serde_json::to_string(scan).unwrap();
        assert_eq!(json(&Scan::Running), r#"{"state":"running"}"#);
        assert_eq!(json(&Scan::Clean), r#"{"state":"clean"}"#);
        assert_eq!(json(&Scan::Unavailable), r#"{"state":"unavailable"}"#);
        assert_eq!(
            json(&Scan::Threat { threats: vec!["Virus:X".into()] }),
            r#"{"state":"threat","threats":["Virus:X"]}"#
        );
    }
}
