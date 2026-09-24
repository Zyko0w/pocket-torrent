//! Étranglement des pairs (choking, BEP 3), ajouté pour Torrent Cloud : librqbit débloquait
//! tous les pairs dès la connexion, l'envoi était donc éparpillé entre tous au lieu d'aller en
//! priorité à ceux qui nous envoient le plus en retour.
//!
//! Toutes les 10 s, pour chaque torrent :
//! - en téléchargement, les pairs intéressés qui nous ont le plus envoyé sont débloqués
//!   (réciprocité, « tit-for-tat ») ; en partage, ceux qui reçoivent le plus vite ;
//! - une place tourne toutes les 30 s au hasard (« optimistic unchoke »), pour laisser leur
//!   chance aux nouveaux pairs et découvrir de meilleurs partenaires ;
//! - garde-fou : s'il n'y a pas plus de pairs intéressés que de places, tous sont débloqués
//!   (exactement l'ancien comportement).
//!
//! Un pair qui devient intéressé est débloqué tout de suite s'il reste une place libre, sans
//! attendre le prochain tour.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use peer_binary_protocol::Message;
use tracing::trace;

use crate::peer_connection::WriterRequest;

use super::TorrentStateLive;

/// Intervalle entre deux décisions (raccourci dans les tests d'essaim local).
#[cfg(not(test))]
const ROUND: Duration = Duration::from_secs(10);
#[cfg(test)]
const ROUND: Duration = Duration::from_millis(300);
/// La place tournante change tous les N tours (30 s).
const OPTIMISTIC_EVERY_ROUNDS: u64 = 3;

/// Ce que le choker sait d'un pair connecté pour un tour.
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub addr: SocketAddr,
    pub interested: bool,
    pub unchoked: bool,
    /// Octets échangés pendant le dernier tour (reçus de lui en téléchargement, envoyés à lui
    /// en partage).
    pub rate: u64,
}

/// Pairs à débloquer pour ce tour. `optimistic` : pair choisi pour la place tournante, s'il
/// est encore intéressé et pas déjà retenu par ailleurs.
pub(crate) fn choose_unchoked(
    candidates: &[Candidate],
    slots: usize,
    optimistic: Option<SocketAddr>,
) -> HashSet<SocketAddr> {
    let slots = slots.max(1);
    let mut interested: Vec<&Candidate> = candidates.iter().filter(|c| c.interested).collect();
    if interested.len() <= slots {
        return interested.iter().map(|c| c.addr).collect();
    }
    // les plus rapides d'abord ; à égalité, on garde ceux déjà débloqués (moins d'allers-retours)
    interested.sort_by(|a, b| b.rate.cmp(&a.rate).then(b.unchoked.cmp(&a.unchoked)));
    let mut chosen: HashSet<SocketAddr> =
        interested.iter().take(slots - 1).map(|c| c.addr).collect();
    let optimistic = optimistic
        .filter(|addr| !chosen.contains(addr) && interested.iter().any(|c| c.addr == *addr))
        .or_else(|| {
            interested
                .iter()
                .map(|c| c.addr)
                .find(|addr| !chosen.contains(addr))
        });
    chosen.extend(optimistic);
    chosen
}

/// Choisit une nouvelle place tournante parmi les pairs intéressés pas encore débloqués.
fn pick_optimistic(
    candidates: &[Candidate],
    exclude: &HashSet<SocketAddr>,
    round: u64,
) -> Option<SocketAddr> {
    let pool: Vec<SocketAddr> = candidates
        .iter()
        .filter(|c| c.interested && !exclude.contains(&c.addr))
        .map(|c| c.addr)
        .collect();
    if pool.is_empty() {
        return None;
    }
    // pas besoin d'un vrai aléa : il suffit de ne pas toujours retomber sur le même pair
    let mixed = round.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (pool.len() as u64).rotate_left(17);
    let len = u64::try_from(pool.len()).ok()?;
    pool.get(usize::try_from(mixed % len).ok()?).copied()
}

impl TorrentStateLive {
    pub(crate) fn upload_slots(&self) -> Option<usize> {
        self.shared.options.upload_slots
    }

    /// Envoie Choke/Unchoke au pair si son état change.
    fn set_unchoked(&self, addr: SocketAddr, unchoked: bool) {
        self.peers
            .with_live_mut(addr, "choker_set_unchoked", |live| {
                if live.unchoked != unchoked {
                    live.unchoked = unchoked;
                    let message = if unchoked {
                        Message::Unchoke
                    } else {
                        Message::Choke
                    };
                    let _ = live.tx.send(WriterRequest::Message(message));
                    trace!(peer = %addr, unchoked, "choker");
                }
            });
    }

    /// Pairs débloqués, intéressés ou non : chacun occupe une place.
    fn unchoked_count(&self) -> usize {
        self.peers
            .states
            .iter()
            .filter(|p| p.value().get_live().is_some_and(|l| l.unchoked))
            .count()
    }

    /// Pour les tests : (pairs intéressés, pairs débloqués).
    #[cfg(test)]
    pub(crate) fn choke_counts(&self) -> (usize, usize) {
        let interested = self
            .peers
            .states
            .iter()
            .filter(|p| p.value().get_live().is_some_and(|l| l.peer_interested))
            .count();
        (interested, self.unchoked_count())
    }

    /// Pair devenu intéressé : débloqué tout de suite s'il reste une place.
    pub(crate) fn unchoke_if_slot_free(&self, addr: SocketAddr) {
        let Some(slots) = self.upload_slots() else {
            return;
        };
        let _decision = self.choker_lock.lock();
        if self.unchoked_count() < slots {
            self.set_unchoked(addr, true);
        }
    }

    /// Tâche de fond du choker (lancée seulement si `upload_slots` est défini).
    pub(crate) async fn task_choker(self: Arc<Self>, slots: usize) -> crate::Result<()> {
        let mut previous: HashMap<SocketAddr, (u64, u64)> = HashMap::new();
        let mut optimistic: Option<SocketAddr> = None;
        let mut round: u64 = 0;
        loop {
            tokio::time::sleep(ROUND).await;
            round += 1;
            let seeding = self.is_finished();
            let mut counters = HashMap::new();
            let candidates: Vec<Candidate> = self
                .peers
                .states
                .iter()
                .filter_map(|entry| {
                    let peer = entry.value();
                    let live = peer.get_live()?;
                    let fetched = peer.stats.counters.fetched_bytes.load(Ordering::Relaxed);
                    let uploaded = peer.stats.counters.uploaded_bytes.load(Ordering::Relaxed);
                    let (prev_fetched, prev_uploaded) = previous
                        .get(entry.key())
                        .copied()
                        .unwrap_or((fetched, uploaded));
                    counters.insert(*entry.key(), (fetched, uploaded));
                    let rate = if seeding {
                        uploaded.saturating_sub(prev_uploaded)
                    } else {
                        fetched.saturating_sub(prev_fetched)
                    };
                    Some(Candidate {
                        addr: *entry.key(),
                        interested: live.peer_interested,
                        unchoked: live.unchoked,
                        rate,
                    })
                })
                .collect();
            previous = counters;

            if round % OPTIMISTIC_EVERY_ROUNDS == 1 || optimistic.is_none() {
                let regular = choose_unchoked(&candidates, slots, None);
                optimistic = pick_optimistic(&candidates, &regular, round);
            }
            let chosen = choose_unchoked(&candidates, slots, optimistic);
            let _decision = self.choker_lock.lock();
            // d'abord libérer les places, puis les attribuer : jamais plus de `slots` débloqués
            for candidate in candidates.iter().filter(|c| !chosen.contains(&c.addr)) {
                self.set_unchoked(candidate.addr, false);
            }
            for addr in &chosen {
                self.set_unchoked(*addr, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u16, interested: bool, unchoked: bool, rate: u64) -> Candidate {
        Candidate {
            addr: SocketAddr::from(([10, 0, 0, 1], n)),
            interested,
            unchoked,
            rate,
        }
    }

    #[test]
    fn few_interested_peers_are_all_unchoked() {
        let peers = [
            peer(1, true, false, 0),
            peer(2, true, false, 0),
            peer(3, false, true, 99),
        ];
        let chosen = choose_unchoked(&peers, 4, None);
        assert_eq!(chosen.len(), 2);
        assert!(
            !chosen.contains(&peers[2].addr),
            "un pair pas intéressé n'occupe pas de place"
        );
    }

    #[test]
    fn fastest_peers_win_plus_one_optimistic() {
        let peers: Vec<Candidate> = (1..=10)
            .map(|n| peer(n, true, false, u64::from(n) * 100))
            .collect();
        let chosen = choose_unchoked(&peers, 4, Some(peers[0].addr));
        assert_eq!(chosen.len(), 4);
        for fast in [10, 9, 8] {
            assert!(
                chosen.contains(&peer(fast, true, false, 0).addr),
                "le pair {fast} est parmi les plus rapides"
            );
        }
        assert!(
            chosen.contains(&peers[0].addr),
            "le plus lent a la place tournante"
        );
    }

    #[test]
    fn optimistic_is_replaced_when_no_longer_interested() {
        let mut peers: Vec<Candidate> = (1..=6)
            .map(|n| peer(n, true, false, u64::from(n)))
            .collect();
        peers[0].interested = false;
        let chosen = choose_unchoked(&peers, 3, Some(peers[0].addr));
        assert_eq!(chosen.len(), 3);
        assert!(!chosen.contains(&peers[0].addr));
    }

    #[test]
    fn optimistic_pick_avoids_regular_peers() {
        let peers: Vec<Candidate> = (1..=5)
            .map(|n| peer(n, true, false, u64::from(n)))
            .collect();
        let regular = choose_unchoked(&peers, 3, None);
        for round in 0..20 {
            let pick = pick_optimistic(&peers, &regular, round).unwrap();
            assert!(!regular.contains(&pick));
        }
    }
}
