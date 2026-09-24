//! Réglages de l'application, enregistrés en JSON dans le dossier de configuration.
//! Les clés secrètes (R2, WireGuard) n'y sont pas : voir `secrets.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Dossier où arrivent les téléchargements sans catégorie.
    pub download_dir: PathBuf,
    pub categories: Vec<Category>,
    pub limits: SpeedLimits,
    pub queue: QueueSettings,
    pub schedule: ScheduleSettings,
    pub game_mode: GameModeSettings,
    pub notifications: NotificationSettings,
    /// Port d'écoute des torrents, 0 = automatique.
    pub torrent_port: u16,
    /// µTP (BEP 29), en plus du TCP : plus de pairs joignables derrière un NAT strict,
    /// mais le module µTP de librqbit est encore jeune (désactivé par défaut).
    pub utp_enabled: bool,
    /// Pairs connectés en même temps, par torrent ; 0 = illimité (§4.3).
    pub max_peers_per_torrent: u32,
    /// Places d'envoi par torrent (choking, §4.2) : les pairs qui nous envoient le plus sont
    /// servis en priorité, plus une place tournante. 0 = tous les pairs servis en même temps.
    pub upload_slots: u32,
    /// Espace à laisser libre sur le disque, en Mo.
    pub disk_margin_mb: u64,
    /// Continuer à partager un torrent une fois téléchargé.
    pub seed_when_done: bool,
    pub seeding: SeedingSettings,
    /// Fermer la fenêtre la cache (l'icône reste près de l'horloge).
    pub close_to_tray: bool,
    /// Empêche la mise en veille de Windows pendant qu'un transfert est actif.
    pub prevent_sleep: bool,
    /// Analyse chaque téléchargement terminé avec Microsoft Defender (§10.4). Rapport
    /// seulement : ne supprime ni ne met jamais en quarantaine automatiquement.
    pub scan_downloads: bool,
    /// Cherche une nouvelle version au démarrage (GitHub Releases) et la propose, sans
    /// jamais l'installer d'office. Seule connexion de l'appli qui ne concerne pas un transfert.
    pub check_updates: bool,
    /// Ancien programme associé aux liens magnet, remis en place si on désactive le nôtre.
    pub magnet_previous: Option<String>,
    /// Ancien programme associé aux fichiers .torrent, remis en place si on désactive le nôtre.
    pub torrent_previous: Option<String>,
    pub r2: R2Settings,
    pub vpn: VpnSettings,
    pub privacy: PrivacySettings,
    pub security: SecuritySettings,
    pub blocklist: BlocklistSettings,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionLevel {
    /// Connexions en clair.
    Off,
    /// Chiffre quand le pair le permet.
    Prefer,
    /// Refuse les pairs qui ne chiffrent pas : le trafic entre pairs n'est jamais reconnaissable.
    #[default]
    Require,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PrivacySettings {
    pub encryption: EncryptionLevel,
    /// Désactive ce qui reste reconnaissable malgré le chiffrement : DHT, découverte locale,
    /// trackers HTTP et UDP (seuls les trackers HTTPS restent).
    pub hide_announces: bool,
    /// Mode confidentialité renforcée (§9.6) : en plus de `hide_announces`, coupe l'UPnP,
    /// masque l'identifiant et la version du client auprès des pairs, et ne leur révèle pas
    /// l'IP qu'on leur voit (BEP 10 `yourip`) et ne cherche pas de mise à jour au démarrage.
    /// Aucune télémétrie n'existe de toute façon dans Pocket Torrent.
    pub hardened_mode: bool,
}

/// Liste de blocage d'IP (§9.7) : chemin d'un fichier local ou URL http(s) au format P2P
/// (« nom:début-fin » par ligne, gzip accepté). Efficacité faible en pratique — les IP
/// surveillées changent souvent — mais gardée à la demande du plan, avec l'avertissement en
/// interface : ce n'est pas une vraie protection.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct BlocklistSettings {
    pub enabled: bool,
    /// Chemin local ou URL http(s)/file. Rechargée à chaque démarrage du moteur (§7.3 :
    /// mise à jour automatique) — pas de vérification périodique en tâche de fond.
    pub source: String,
}

/// Extensions dangereuses recommandées par défaut (voir §15.3 du plan d'upgrade) :
/// exécutables et raccourcis qu'un torrent peut faire passer pour un média.
fn default_excluded_extensions() -> Vec<String> {
    [".exe", ".scr", ".lnk", ".bat", ".cmd", ".vbs", ".msi", ".ps1", ".js", ".jar", ".com", ".pif"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SecuritySettings {
    /// Extensions signalées avant le téléchargement (avec le point, ex. « .exe »).
    /// Ces fichiers sont décochés par défaut à l'ajout d'un torrent, sans être bloqués :
    /// l'utilisateur garde la main s'il les veut vraiment.
    pub excluded_extensions: Vec<String>,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self { excluded_extensions: default_excluded_extensions() }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VpnMode {
    /// Pas de VPN imposé.
    #[default]
    Off,
    /// Appli VPN installée : les téléchargements passent par sa carte réseau.
    Adapter,
    /// Tunnel WireGuard intégré à Pocket Torrent (seuls les téléchargements l'utilisent).
    Wireguard,
}

/// Nom de la carte réseau créée par le tunnel intégré.
pub const WIREGUARD_ADAPTER: &str = "TorrentCloud";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VpnSettings {
    pub mode: VpnMode,
    /// Carte réseau du VPN (mode `Adapter`).
    pub adapter: String,
}

impl VpnSettings {
    /// Carte réseau par laquelle tout doit passer, ou `None` sans VPN.
    pub fn required_adapter(&self) -> Option<&str> {
        match self.mode {
            VpnMode::Off => None,
            VpnMode::Adapter => Some(self.adapter.as_str()),
            VpnMode::Wireguard => Some(WIREGUARD_ADAPTER),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub folder: PathBuf,
}

/// Limites de vitesse en Ko/s, 0 = illimitée.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SpeedLimits {
    pub down_kbps: u32,
    pub up_kbps: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueSettings {
    /// Téléchargements actifs en même temps, 0 = illimité.
    pub max_active: u32,
    /// Un téléchargement quasi à l'arrêt ne compte pas dans cette limite, pour ne pas
    /// bloquer la file avec un torrent sans pair.
    pub ignore_slow: bool,
    /// Vitesse (réception + envoi) en dessous de laquelle un téléchargement est « lent ».
    pub slow_threshold_kbps: u32,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self { max_active: 3, ignore_slow: false, slow_threshold_kbps: 10 }
    }
}

/// Planificateur de limites (§5) : applique `down_kbps`/`up_kbps` pendant une plage horaire,
/// les jours cochés ; les limites normales (`Settings::limits`) s'appliquent en dehors.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ScheduleSettings {
    pub enabled: bool,
    /// Minutes depuis minuit, heure locale (0-1439).
    pub start_minutes: u16,
    pub end_minutes: u16,
    /// Jours où le planificateur s'applique : dimanche = 0 … samedi = 6.
    pub days: Vec<u8>,
    pub down_kbps: u32,
    pub up_kbps: u32,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            start_minutes: 0,
            end_minutes: 8 * 60,
            days: (0..=6).collect(),
            down_kbps: 0,
            up_kbps: 0,
        }
    }
}

impl ScheduleSettings {
    /// Vrai si l'heure locale actuelle tombe dans la plage programmée (gère le passage de minuit).
    pub fn active_now(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let now = chrono::Local::now();
        use chrono::{Datelike, Timelike};
        let weekday = now.weekday().num_days_from_sunday() as u8;
        if !self.days.contains(&weekday) {
            return false;
        }
        let minutes = u16::try_from(now.hour() * 60 + now.minute()).unwrap_or(0);
        if self.start_minutes <= self.end_minutes {
            minutes >= self.start_minutes && minutes < self.end_minutes
        } else {
            minutes >= self.start_minutes || minutes < self.end_minutes
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GameModeSwitch {
    Auto,
    On,
    Off,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GameAction {
    Limit,
    Pause,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameModeSettings {
    pub mode: GameModeSwitch,
    /// Programmes qui activent le mode jeu (ex. cs2.exe).
    pub processes: Vec<String>,
    pub action: GameAction,
    pub down_kbps: u32,
    pub up_kbps: u32,
}

impl Default for GameModeSettings {
    fn default() -> Self {
        Self {
            mode: GameModeSwitch::Auto,
            processes: vec!["cs2.exe".into(), "RocketLeague.exe".into()],
            action: GameAction::Limit,
            down_kbps: 500,
            up_kbps: 50,
        }
    }
}

/// Limites de partage après la fin d'un téléchargement (0 = illimité) : utile sur les trackers
/// privés, qui exigent souvent un ratio ou une durée de partage minimum, mais bannissent
/// aussi ceux qui abusent de la bande passante en partageant indéfiniment.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SeedingSettings {
    /// Ratio envoyé / reçu au-delà duquel le partage s'arrête, 0 = illimité.
    pub ratio_limit: f64,
    /// Minutes de partage après la fin du téléchargement, 0 = illimité.
    pub time_limit_minutes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    pub done: bool,
    pub error: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { done: true, error: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct R2Settings {
    pub account_id: String,
    pub bucket: String,
    /// Domaine public du bucket (liens permanents) ; vide = liens signés temporaires.
    pub public_url: String,
    pub link_expiry_days: u32,
    pub delete_after_upload: bool,
}

impl Default for R2Settings {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            bucket: String::new(),
            public_url: String::new(),
            link_expiry_days: 7,
            delete_after_upload: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            categories: Vec::new(),
            limits: SpeedLimits::default(),
            queue: QueueSettings::default(),
            schedule: ScheduleSettings::default(),
            game_mode: GameModeSettings::default(),
            notifications: NotificationSettings::default(),
            torrent_port: 0,
            utp_enabled: false,
            max_peers_per_torrent: 0,
            upload_slots: 8,
            disk_margin_mb: 1024,
            seed_when_done: false,
            seeding: SeedingSettings::default(),
            close_to_tray: true,
            prevent_sleep: true,
            scan_downloads: false,
            check_updates: true,
            magnet_previous: None,
            torrent_previous: None,
            r2: R2Settings::default(),
            vpn: VpnSettings::default(),
            privacy: PrivacySettings::default(),
            security: SecuritySettings::default(),
            blocklist: BlocklistSettings::default(),
        }
    }
}

fn default_download_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("Torrents")
}

const MAX_KBPS: u32 = 4_000_000; // ~4 Go/s, au-delà une limite n'a pas de sens

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // écriture atomique : jamais de fichier de réglages à moitié écrit
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn category(&self, id: Option<&str>) -> Option<&Category> {
        let id = id?;
        self.categories.iter().find(|c| c.id == id)
    }

    /// Dossier de destination d'un téléchargement selon sa catégorie.
    pub fn folder_for(&self, category: Option<&str>) -> PathBuf {
        self.category(category).map_or_else(|| self.download_dir.clone(), |c| c.folder.clone())
    }

    /// Tous les dossiers où l'application range des fichiers (garde-fou des suppressions).
    pub fn download_roots(&self) -> Vec<PathBuf> {
        std::iter::once(self.download_dir.clone()).chain(self.categories.iter().map(|c| c.folder.clone())).collect()
    }

    pub fn r2_ready(&self) -> bool {
        !self.r2.account_id.is_empty() && !self.r2.bucket.is_empty()
    }

    /// Nettoie les valeurs saisies puis les vérifie. Renvoie un message en français en cas d'erreur.
    pub fn normalize(&mut self) -> Result<(), String> {
        let absolute = |path: &Path, label: &str| -> Result<(), String> {
            if path.as_os_str().is_empty() || !path.is_absolute() {
                return Err(format!("{label} : chemin complet attendu (ex. D:\\Téléchargements)."));
            }
            Ok(())
        };
        absolute(&self.download_dir, "Dossier de téléchargement")?;

        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for (i, category) in self.categories.iter_mut().enumerate() {
            category.name = category.name.trim().to_owned();
            let label = format!("Catégorie {}", i + 1);
            if category.name.is_empty() || category.name.chars().count() > 40 {
                return Err(format!("{label} : nom de 1 à 40 caractères."));
            }
            if !names.insert(category.name.to_lowercase()) {
                return Err(format!("La catégorie « {} » existe déjà.", category.name));
            }
            absolute(&category.folder, &format!("Dossier de la catégorie « {} »", category.name))?;
            if !valid_id(&category.id) {
                category.id = slug(&category.name);
            }
            while !ids.insert(category.id.clone()) {
                category.id.push('-');
                category.id.push_str(&(i + 1).to_string());
            }
        }

        for (value, label) in [
            (self.limits.down_kbps, "Réception max"),
            (self.limits.up_kbps, "Envoi max"),
            (self.game_mode.down_kbps, "Réception en mode jeu"),
            (self.game_mode.up_kbps, "Envoi en mode jeu"),
            (self.queue.slow_threshold_kbps, "Seuil de vitesse lente"),
            (self.schedule.down_kbps, "Réception planifiée"),
            (self.schedule.up_kbps, "Envoi planifié"),
        ] {
            if value > MAX_KBPS {
                return Err(format!("{label} : {MAX_KBPS} Ko/s maximum."));
            }
        }
        if self.queue.max_active > 50 {
            return Err("Téléchargements simultanés : 50 maximum (0 = illimité).".into());
        }

        self.game_mode.processes =
            self.game_mode.processes.iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect();
        if self.game_mode.processes.len() > 30
            || self
                .game_mode
                .processes
                .iter()
                .any(|p| p.chars().count() > 60 || !p.chars().all(|c| c.is_alphanumeric() || " ._()-".contains(c)))
        {
            return Err("Mode jeu : noms de programmes invalides (ex. cs2.exe), 30 maximum.".into());
        }

        if self.torrent_port != 0 && self.torrent_port < 1024 {
            return Err("Port des torrents : 0 (automatique) ou un port entre 1024 et 65535.".into());
        }
        if self.disk_margin_mb > 1_000_000 {
            return Err("Marge d'espace disque : 1 000 000 Mo maximum.".into());
        }
        if self.max_peers_per_torrent > 10_000 {
            return Err("Pairs par torrent : 10 000 maximum (0 = illimité).".into());
        }
        if self.upload_slots > 500 {
            return Err("Places d'envoi par torrent : 500 maximum (0 = tous les pairs).".into());
        }

        let r2 = &mut self.r2;
        r2.account_id = r2.account_id.trim().to_owned();
        r2.bucket = r2.bucket.trim().to_owned();
        r2.public_url = r2.public_url.trim().trim_end_matches('/').to_owned();
        if !r2.account_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("R2 : l'identifiant de compte ne contient que des lettres et des chiffres.".into());
        }
        if !r2.bucket.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err("R2 : nom de bucket invalide (minuscules, chiffres et tirets).".into());
        }
        if !r2.public_url.is_empty()
            && !(r2.public_url.starts_with("https://") && !r2.public_url.contains(char::is_whitespace))
        {
            return Err("R2 : l'URL publique doit commencer par https://, ou rester vide.".into());
        }
        if !(1..=7).contains(&r2.link_expiry_days) {
            return Err("R2 : durée des liens entre 1 et 7 jours.".into());
        }

        self.vpn.adapter = self.vpn.adapter.trim().to_owned();
        if self.vpn.mode == VpnMode::Adapter && self.vpn.adapter.is_empty() {
            return Err("VPN : choisis la carte réseau de ton VPN.".into());
        }

        self.security.excluded_extensions =
            self.security.excluded_extensions.iter().map(|e| e.trim().to_lowercase()).collect();
        let mut extensions = HashSet::new();
        self.security.excluded_extensions.retain(|ext| !ext.is_empty() && extensions.insert(ext.clone()));
        if self.security.excluded_extensions.len() > 60
            || self
                .security
                .excluded_extensions
                .iter()
                .any(|e| e.len() > 20 || !e.starts_with('.') || !e[1..].chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err("Extensions signalées : un point suivi de lettres/chiffres (ex. « .exe »), 60 maximum.".into());
        }

        if self.schedule.start_minutes > 1439 || self.schedule.end_minutes > 1439 {
            return Err("Planificateur : heures entre 00:00 et 23:59.".into());
        }
        self.schedule.days.retain(|d| *d <= 6);
        self.schedule.days.sort_unstable();
        self.schedule.days.dedup();
        if self.schedule.enabled && self.schedule.days.is_empty() {
            return Err("Planificateur : coche au moins un jour.".into());
        }

        self.blocklist.source = self.blocklist.source.trim().to_owned();
        if self.blocklist.enabled && self.blocklist.source.is_empty() {
            return Err("Liste de blocage : indique un fichier ou une adresse http(s).".into());
        }
        if self.blocklist.source.chars().count() > 2000 {
            return Err("Liste de blocage : adresse trop longue.".into());
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// « Films HD » → « films-hd »
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' | 'À' | 'Â' | 'Ä' | 'Á' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'î' | 'ï' | 'í' | 'Î' | 'Ï' => 'i',
            'ô' | 'ö' | 'ó' | 'Ô' | 'Ö' => 'o',
            'ù' | 'û' | 'ü' | 'ú' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            c => c.to_ascii_lowercase(),
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(40).collect();
    if out.is_empty() { "categorie".into() } else { out }
}

// ------------------------------------------------------------ import version web

/// Réglages lus dans le `reglages.json` de la version web de Torrent Cloud.
pub struct WebImport {
    pub settings: Settings,
    pub r2_access_key_id: Option<String>,
    pub r2_secret_access_key: Option<String>,
}

/// Reprend les réglages de la version web par-dessus `current` (les réglages Windows restent ceux du logiciel).
pub fn import_web(path: &Path, current: &Settings) -> anyhow::Result<WebImport> {
    use anyhow::Context;
    use serde_json::Value;

    let text = std::fs::read_to_string(path).context("fichier illisible")?;
    let web: Value = serde_json::from_str(&text).context("ce fichier n'est pas un reglages.json valide")?;
    if web.get("downloadDir").is_none() || web.get("gameMode").is_none() {
        anyhow::bail!("ce fichier ne vient pas de la version web de Torrent Cloud");
    }

    let text_of = |v: &Value| v.as_str().map(str::to_owned);
    let num = |v: &Value| v.as_u64();
    let secret = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let mut s = current.clone();

    if let Some(dir) = text_of(&web["downloadDir"]) {
        s.download_dir = PathBuf::from(dir);
    }
    if let Some(list) = web["categories"].as_array() {
        s.categories = list
            .iter()
            .filter_map(|c| {
                Some(Category {
                    id: text_of(&c["id"]).unwrap_or_default(),
                    name: text_of(&c["name"])?,
                    folder: PathBuf::from(text_of(&c["folder"])?),
                })
            })
            .collect();
    }
    let kbps = |v: &Value, fallback: u32| num(v).map_or(fallback, |n| n.min(MAX_KBPS as u64) as u32);
    s.limits.down_kbps = kbps(&web["limits"]["down"], s.limits.down_kbps);
    s.limits.up_kbps = kbps(&web["limits"]["up"], s.limits.up_kbps);
    s.queue.max_active = num(&web["queue"]["maxActive"]).map_or(s.queue.max_active, |n| n.min(50) as u32);

    let gm = &web["gameMode"];
    s.game_mode.mode = match gm["mode"].as_str() {
        Some("on") => GameModeSwitch::On,
        Some("off") => GameModeSwitch::Off,
        _ => GameModeSwitch::Auto,
    };
    s.game_mode.action = if gm["action"].as_str() == Some("pause") { GameAction::Pause } else { GameAction::Limit };
    if let Some(list) = gm["processes"].as_array() {
        s.game_mode.processes = list.iter().filter_map(text_of).collect();
    }
    s.game_mode.down_kbps = kbps(&gm["down"], s.game_mode.down_kbps);
    s.game_mode.up_kbps = kbps(&gm["up"], s.game_mode.up_kbps);

    if let Some(done) = web["notifications"]["done"].as_bool() {
        s.notifications.done = done;
    }
    if let Some(error) = web["notifications"]["error"].as_bool() {
        s.notifications.error = error;
    }
    s.torrent_port = num(&web["torrentPort"]).map_or(s.torrent_port, |n| u16::try_from(n).unwrap_or(0));
    s.disk_margin_mb = num(&web["diskMarginMb"]).unwrap_or(s.disk_margin_mb);

    let r2 = &web["r2"];
    s.r2.account_id = text_of(&r2["accountId"]).unwrap_or_default();
    s.r2.bucket = text_of(&r2["bucket"]).unwrap_or_default();
    s.r2.public_url = text_of(&r2["publicUrl"]).unwrap_or_default();
    s.r2.link_expiry_days = num(&r2["linkExpiryDays"]).map_or(7, |n| n.clamp(1, 7) as u32);
    s.r2.delete_after_upload = r2["deleteAfterUpload"].as_bool().unwrap_or(true);
    s.normalize().map_err(anyhow::Error::msg)?;

    Ok(WebImport {
        settings: s,
        r2_access_key_id: secret(&r2["accessKeyId"]),
        r2_secret_access_key: secret(&r2["secretAccessKey"]),
    })
}
