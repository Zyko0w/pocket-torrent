//! Pocket Torrent : état de chaque tracker, par torrent, pour l'affichage (volet « Trackers »).
//!
//! Registre global indexé par empreinte : il n'y a rien à faire passer par la session. Chaque
//! flux d'annonces (`TrackerComms::start`) a son propre identifiant ; ses entrées disparaissent
//! quand il s'arrête (torrent en pause, supprimé, ou aperçu terminé).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use librqbit_core::hash_id::Id20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TrackerState {
    /// Première annonce en cours.
    #[default]
    Connecting,
    /// Dernière annonce réussie.
    Working,
    /// Dernière annonce en échec (voir `error`).
    Failing,
    /// Jamais contacté : mode HTTPS seul, ou type de tracker non pris en charge (voir `error`).
    Skipped,
}

#[derive(Clone, Debug, Default)]
pub struct TrackerStatus {
    pub url: String,
    pub state: TrackerState,
    /// Dernière annonce réussie, et nombre de pairs qu'elle a renvoyés.
    pub last_success: Option<SystemTime>,
    pub last_peers: usize,
    pub error: Option<String>,
    pub next_announce: Option<SystemTime>,
}

type Registry = HashMap<Id20, HashMap<String, (u64, TrackerStatus)>>;

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = REGISTRY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// État des trackers annoncés pour ce torrent, triés par adresse.
pub fn tracker_statuses(info_hash: &Id20) -> Vec<TrackerStatus> {
    let mut list: Vec<TrackerStatus> = with_registry(|r| {
        r.get(info_hash).map(|m| m.values().map(|(_, s)| s.clone()).collect()).unwrap_or_default()
    });
    list.sort_by(|a, b| a.url.cmp(&b.url));
    list
}

/// Entrées d'un flux d'annonces ; retirées du registre quand il est abandonné.
pub(crate) struct StatusOwner {
    id: u64,
    info_hash: Id20,
}

impl StatusOwner {
    pub(crate) fn new(info_hash: Id20) -> Self {
        Self { id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed), info_hash }
    }

    fn update(&self, url: &str, f: impl FnOnce(&mut TrackerStatus)) {
        with_registry(|r| {
            let entry = r
                .entry(self.info_hash)
                .or_default()
                .entry(url.to_owned())
                .or_insert_with(|| (self.id, TrackerStatus { url: url.to_owned(), ..Default::default() }));
            // le dernier flux démarré pour ce torrent prend la main sur l'entrée
            entry.0 = self.id;
            f(&mut entry.1);
        });
    }

    pub(crate) fn connecting(&self, url: &str) {
        self.update(url, |s| {
            s.state = TrackerState::Connecting;
            s.error = None;
        });
    }

    pub(crate) fn skipped(&self, url: &str, reason: &str) {
        self.update(url, |s| {
            s.state = TrackerState::Skipped;
            s.error = Some(reason.to_owned());
            s.next_announce = None;
        });
    }

    pub(crate) fn success(&self, url: &str, peers: usize, next_in: Duration) {
        self.update(url, |s| {
            s.state = TrackerState::Working;
            s.last_success = Some(SystemTime::now());
            s.last_peers = peers;
            s.error = None;
            s.next_announce = Some(SystemTime::now() + next_in);
        });
    }

    pub(crate) fn failure(&self, url: &str, error: &str, retry_in: Option<Duration>) {
        self.update(url, |s| {
            s.state = TrackerState::Failing;
            s.error = Some(error.to_owned());
            s.next_announce = retry_in.map(|d| SystemTime::now() + d);
        });
    }
}

impl Drop for StatusOwner {
    fn drop(&mut self) {
        with_registry(|r| {
            if let Some(map) = r.get_mut(&self.info_hash) {
                map.retain(|_, (owner, _)| *owner != self.id);
                if map.is_empty() {
                    r.remove(&self.info_hash);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_follow_their_owner() {
        let hash = Id20::new([7; 20]);
        let first = StatusOwner::new(hash);
        first.success("udp://a", 12, Duration::from_secs(60));
        first.failure("udp://b", "timeout", None);
        let list = tracker_statuses(&hash);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].state, TrackerState::Working);
        assert_eq!(list[0].last_peers, 12);
        assert_eq!(list[1].error.as_deref(), Some("timeout"));

        // un second flux reprend « a » : l'abandon du premier ne retire que « b »
        let second = StatusOwner::new(hash);
        second.connecting("udp://a");
        drop(first);
        let list = tracker_statuses(&hash);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, TrackerState::Connecting);
        drop(second);
        assert!(tracker_statuses(&hash).is_empty());
    }
}
