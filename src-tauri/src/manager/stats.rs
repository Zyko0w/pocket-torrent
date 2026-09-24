//! Statistiques cumulées : octets reçus et envoyés depuis la première utilisation (ou la
//! dernière remise à zéro), et depuis le lancement de l'appli.

use super::*;

/// Dernières valeurs lues des compteurs, pour n'additionner que la différence.
#[derive(Default)]
pub(super) struct TrafficSample {
    engine_fetched: u64,
    engine_uploaded: u64,
    direct_received: u64,
    session_downloaded: u64,
    session_uploaded: u64,
    /// Dernier enregistrement dû aux statistiques : au plus une écriture de jobs.json par minute.
    saved_at: Option<Instant>,
}

/// Octets apparus depuis la lecture précédente. Les compteurs du moteur repartent de zéro
/// quand il redémarre (port, VPN…) : une valeur plus petite qu'avant est alors toute nouvelle.
fn delta(now: u64, before: u64) -> u64 {
    if now >= before { now - before } else { now }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficStats {
    pub downloaded: u64,
    pub uploaded: u64,
    /// Début du décompte (ms depuis 1970), 0 si rien n'a encore été compté.
    pub since: u64,
    pub session_downloaded: u64,
    pub session_uploaded: u64,
}

impl Manager {
    /// Appelé chaque seconde par la boucle.
    pub(super) fn record_traffic(&self) {
        let (fetched, uploaded) = self.engine.traffic_counters();
        let received = direct::RECEIVED_TOTAL.load(Ordering::Relaxed);
        let (down, up, save) = {
            let mut sample = self.traffic.lock();
            let down = delta(fetched, sample.engine_fetched) + delta(received, sample.direct_received);
            let up = delta(uploaded, sample.engine_uploaded);
            sample.engine_fetched = fetched;
            sample.engine_uploaded = uploaded;
            sample.direct_received = received;
            sample.session_downloaded += down;
            sample.session_uploaded += up;
            let save = sample.saved_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(60));
            if save && (down > 0 || up > 0) {
                sample.saved_at = Some(Instant::now());
            }
            (down, up, save)
        };
        if down == 0 && up == 0 {
            return;
        }
        {
            let mut data = self.data.lock();
            let totals = &mut data.totals;
            if totals.since == 0 {
                totals.since = now_ms();
            }
            totals.downloaded += down;
            totals.uploaded += up;
        }
        if save {
            self.mark_dirty();
        }
    }

    pub fn traffic_stats(&self) -> TrafficStats {
        let totals = self.data.lock().totals.clone();
        let sample = self.traffic.lock();
        TrafficStats {
            downloaded: totals.downloaded,
            uploaded: totals.uploaded,
            since: totals.since,
            session_downloaded: sample.session_downloaded,
            session_uploaded: sample.session_uploaded,
        }
    }

    pub fn reset_traffic_stats(&self) -> TrafficStats {
        self.data.lock().totals = Default::default();
        self.save_now();
        self.traffic_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::delta;

    #[test]
    fn counter_reset_counts_new_bytes_only() {
        assert_eq!(delta(150, 100), 50);
        assert_eq!(delta(100, 100), 0);
        // moteur redémarré : le compteur est reparti de zéro puis a compté 30 octets
        assert_eq!(delta(30, 5_000), 30);
    }
}
