//! Répartition de la bande passante de réception selon la priorité : quand plusieurs torrents de
//! priorités différentes téléchargent en même temps, chacun reçoit une part de la capacité
//! proportionnelle à son poids (haute 4, normale 2, basse 1).
//!
//! La capacité est la limite globale si elle est réglée, sinon le débit réel de la connexion,
//! mesuré au fil de l'eau. Rien n'est gaspillé : la part qu'un torrent n'utilise pas (peu de
//! pairs) revient aux autres (partage max-min pondéré). Sans torrent concurrent de priorité
//! différente, aucune limite par torrent n'est posée.

use super::*;

/// Débit minimal laissé à chaque torrent, pour qu'il continue de mesurer ce qu'il pourrait faire.
const FLOOR_BPS: u64 = 16 * 1024;
/// Plancher de la capacité estimée (connexion inconnue ou très lente).
const MIN_CAPACITY_BPS: u64 = 256 * 1024;

pub(super) fn weight(priority: Priority) -> u64 {
    match priority {
        Priority::High => 4,
        Priority::Normal => 2,
        Priority::Low => 1,
    }
}

/// Un torrent qui télécharge : son poids, son débit mesuré, et la limite posée au tour précédent.
#[derive(Clone, Copy, Debug)]
pub(super) struct Flow {
    pub weight: u64,
    pub speed: u64,
    pub previous_cap: Option<u64>,
}

impl Flow {
    /// Ce que le torrent demande : sans limite atteinte, son débit avec une marge pour grandir ;
    /// s'il plafonne à sa limite, on ne sait pas jusqu'où il irait (demande illimitée).
    fn demand(&self) -> u64 {
        match self.previous_cap {
            Some(cap) if self.speed >= cap.saturating_mul(9) / 10 => u64::MAX,
            _ => self.speed.saturating_mul(5) / 4 + FLOOR_BPS,
        }
    }
}

/// Partage max-min pondéré de `capacity` entre les flux : chacun reçoit sa part selon son poids,
/// sans dépasser sa demande ; le reste est redistribué aux autres.
pub(super) fn allocate(capacity: u64, flows: &[Flow]) -> Vec<u64> {
    let mut caps = vec![0u64; flows.len()];
    let mut open: Vec<usize> = (0..flows.len()).collect();
    let mut remaining = capacity;
    while !open.is_empty() {
        let total_weight: u64 = open.iter().map(|&i| flows[i].weight).sum();
        let share = |i: usize| (remaining as u128 * flows[i].weight as u128 / total_weight.max(1) as u128) as u64;
        // les flux qui demandent moins que leur part sont servis entièrement
        let satisfied: Vec<usize> = open.iter().copied().filter(|&i| flows[i].demand() <= share(i)).collect();
        if satisfied.is_empty() {
            for &i in &open {
                caps[i] = share(i);
            }
            break;
        }
        for &i in &satisfied {
            caps[i] = flows[i].demand();
            remaining = remaining.saturating_sub(caps[i]);
        }
        open.retain(|i| !satisfied.contains(i));
    }
    caps.into_iter().map(|cap| cap.max(FLOOR_BPS)).collect()
}

/// Estimation de la capacité de la connexion, sans limite globale : pic de débit récent qui
/// décroît lentement, et relevé de 10 % quand on s'en approche (pour ne jamais brider une
/// connexion plus rapide que ce qu'on a vu).
#[derive(Debug, Default)]
pub(super) struct Capacity {
    estimate: u64,
}

impl Capacity {
    pub fn update(&mut self, total_speed: u64) -> u64 {
        let decayed = self.estimate.saturating_mul(199) / 200;
        let mut estimate = decayed.max(total_speed);
        if total_speed >= estimate.saturating_mul(9) / 10 {
            estimate = estimate.saturating_mul(11) / 10;
        }
        self.estimate = estimate.max(MIN_CAPACITY_BPS);
        self.estimate
    }
}

#[derive(Default)]
pub(super) struct BandwidthState {
    capacity: Capacity,
    /// Limites posées au tour précédent, par empreinte.
    caps: HashMap<String, u64>,
}

impl Manager {
    /// Recalcule chaque seconde les limites de réception par torrent (voir en tête du module).
    pub(super) fn balance_bandwidth(&self, views: &[TorrentView]) {
        // cas courant (au plus un torrent qui télécharge, aucune limite posée) : rien à calculer,
        // juste suivre le débit de la connexion
        let live = views.iter().filter(|v| v.state == EngineState::Live && !v.finished);
        if live.clone().count() < 2 && self.bandwidth.lock().caps.is_empty() {
            self.bandwidth.lock().capacity.update(live.map(|v| v.down_speed).sum());
            return;
        }
        let priorities: HashMap<String, Priority> =
            self.data.lock().torrents.iter().map(|(hash, e)| (hash.clone(), e.common.priority)).collect();
        let downloading: Vec<(&TorrentView, Priority)> = views
            .iter()
            .filter(|v| v.state == EngineState::Live && !v.finished)
            .filter_map(|v| Some((v, *priorities.get(&v.info_hash)?)))
            .collect();

        let global = u64::from(self.effective_limits().down_kbps) * 1024;
        let total: u64 = downloading.iter().map(|(v, _)| v.down_speed).sum();
        let mut state = self.bandwidth.lock();
        let measured = state.capacity.update(total);
        let mixed = downloading.windows(2).any(|pair| pair[0].1 != pair[1].1);

        if downloading.len() < 2 || !mixed {
            // rien à départager : aucune limite par torrent
            for hash in std::mem::take(&mut state.caps).into_keys() {
                self.engine.set_torrent_download_limit(&hash, None);
            }
            return;
        }

        let capacity = if global > 0 { global } else { measured };
        let flows: Vec<Flow> = downloading
            .iter()
            .map(|(v, priority)| Flow {
                weight: weight(*priority),
                speed: v.down_speed,
                previous_cap: state.caps.get(&v.info_hash).copied(),
            })
            .collect();
        let caps = allocate(capacity, &flows);

        let mut next = HashMap::new();
        for ((view, _), cap) in downloading.iter().zip(caps) {
            self.engine.set_torrent_download_limit(&view.info_hash, Some(cap));
            next.insert(view.info_hash.clone(), cap);
        }
        // torrents qui ne téléchargent plus : limite retirée
        for hash in state.caps.keys().filter(|h| !next.contains_key(*h)) {
            self.engine.set_torrent_download_limit(hash, None);
        }
        state.caps = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn flow(weight: u64, speed: u64, previous_cap: Option<u64>) -> Flow {
        Flow { weight, speed, previous_cap }
    }

    #[test]
    fn saturated_torrents_share_by_weight() {
        // trois torrents qui plafonnent tous à leur limite : parts 4/2/1 de 7 Mio/s
        let caps = allocate(7 * MIB, &[flow(4, MIB, Some(MIB)), flow(2, MIB, Some(MIB)), flow(1, MIB, Some(MIB))]);
        assert_eq!(caps, vec![4 * MIB, 2 * MIB, MIB]);
    }

    #[test]
    fn unused_share_goes_to_the_others() {
        // le torrent prioritaire n'a presque pas de sources : sa part inutilisée revient à l'autre
        let slow_high = flow(4, 100 * 1024, None);
        let busy_low = flow(1, 5 * MIB, Some(5 * MIB));
        let caps = allocate(10 * MIB, &[slow_high, busy_low]);
        let high_demand = 100 * 1024 * 5 / 4 + FLOOR_BPS;
        assert_eq!(caps[0], high_demand);
        assert_eq!(caps[1], 10 * MIB - high_demand);
    }

    #[test]
    fn every_torrent_keeps_a_floor() {
        let caps = allocate(20 * 1024, &[flow(4, MIB, Some(MIB)), flow(1, MIB, Some(MIB))]);
        assert!(caps.iter().all(|&c| c >= FLOOR_BPS));
    }

    #[test]
    fn capacity_follows_the_connection() {
        let mut capacity = Capacity::default();
        assert_eq!(capacity.update(0), MIN_CAPACITY_BPS);
        // à fond : l'estimation dépasse toujours un peu le débit vu (la connexion peut aller plus vite)
        let seen = capacity.update(10 * MIB);
        assert!(seen > 10 * MIB);
        // débit qui retombe : l'estimation baisse lentement, sans s'effondrer
        let after = capacity.update(MIB);
        assert!(after < seen && after > 9 * MIB);
    }
}
