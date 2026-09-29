use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use parking_lot::Mutex;

/// Fenêtre de mesure habituelle : 1 s, le rythme de rafraîchissement de l'interface (à la
/// tolérance du minuteur près, pour un estimateur alimenté une fois par seconde).
const MIN_WINDOW: Duration = Duration::from_millis(950);

/// En dessous de 4 blocs (4 × 16 Kio) sur la fenêtre, la mesure est élargie : un bloc isolé
/// divisé par une fenêtre courte donnerait un pic (16 Kio sur 0,4 s = 40 Kio/s) suivi d'un 0.
const MIN_WINDOW_BYTES: u64 = 4 * 16 * 1024;

#[derive(Clone, Copy)]
struct ProgressSnapshot {
    progress_bytes: u64,
    instant: Instant,
}

/// Vitesse de téléchargement/envoi sur une fenêtre glissante adaptée au débit (Pocket Torrent) :
/// 1 s dès qu'assez de données arrivent, élargie jusqu'à `window_seconds` quand le débit est
/// trop faible pour qu'une seconde soit représentative (démarrage, pair très lent).
///
/// À l'origine, la fenêtre comptait `window_seconds` relevés et non des secondes : le moteur
/// relève chaque torrent toutes les 100 ms, la vitesse était donc mesurée sur 0,4 s.
pub struct SpeedEstimator {
    snapshots: Mutex<VecDeque<ProgressSnapshot>>,
    max_window: Duration,
    bytes_per_second: AtomicU64,
    time_remaining_millis: AtomicU64,
}

impl Default for SpeedEstimator {
    fn default() -> Self {
        Self::new(5)
    }
}

impl SpeedEstimator {
    pub fn new(window_seconds: usize) -> Self {
        assert!(window_seconds > 1);
        Self {
            snapshots: Mutex::new(VecDeque::new()),
            max_window: Duration::from_secs(window_seconds as u64),
            bytes_per_second: Default::default(),
            time_remaining_millis: Default::default(),
        }
    }

    pub fn time_remaining(&self) -> Option<Duration> {
        let tr = self.time_remaining_millis.load(Ordering::Relaxed);
        if tr == 0 {
            return None;
        }
        Some(Duration::from_millis(tr))
    }

    pub fn bps(&self) -> u64 {
        self.bytes_per_second.load(Ordering::Relaxed)
    }

    pub fn mbps(&self) -> f64 {
        self.bps() as f64 / 1024f64 / 1024f64
    }

    pub fn add_snapshot(
        &self,
        progress_bytes: u64,
        remaining_bytes: Option<u64>,
        instant: Instant,
    ) {
        let (downloaded_bytes_diff, elapsed) = {
            let mut g = self.snapshots.lock();

            let is_first = g.is_empty();
            g.push_back(ProgressSnapshot {
                progress_bytes,
                instant,
            });
            if is_first {
                return;
            }
            // garde un relevé au moins aussi vieux que la fenêtre maximale, pas plus
            while g.len() > 2 && instant.saturating_duration_since(g[1].instant) >= self.max_window
            {
                g.pop_front();
            }

            // le relevé le plus récent qui couvre au moins 1 s et assez d'octets, sinon le plus
            // ancien (toute la fenêtre maximale)
            let base = g
                .iter()
                .rev()
                .skip(1)
                .find(|s| {
                    instant.saturating_duration_since(s.instant) >= MIN_WINDOW
                        && progress_bytes.saturating_sub(s.progress_bytes) >= MIN_WINDOW_BYTES
                })
                .or(g.front())
                .copied()
                .unwrap();
            (
                progress_bytes.saturating_sub(base.progress_bytes),
                instant.saturating_duration_since(base.instant),
            )
        };

        // jamais divisé par moins d'une seconde : au démarrage, l'historique est plus court
        let bps = downloaded_bytes_diff as f64 / elapsed.max(MIN_WINDOW).as_secs_f64();

        let time_remaining_millis_rounded: u64 = if downloaded_bytes_diff > 0 {
            let time_remaining_secs = remaining_bytes.unwrap_or_default() as f64 / bps;
            (time_remaining_secs * 1000f64) as u64
        } else {
            0
        };
        self.time_remaining_millis
            .store(time_remaining_millis_rounded, Ordering::Relaxed);
        self.bytes_per_second.store(bps as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;

    /// Alimente l'estimateur toutes les `tick` pendant `duration`, avec `bytes_at(t)` octets
    /// reçus au total à l'instant t ; renvoie la vitesse relevée après chaque tick.
    fn run(
        estimator: &SpeedEstimator,
        start: Instant,
        from: Duration,
        duration: Duration,
        tick: Duration,
        bytes_at: impl Fn(Duration) -> u64,
    ) -> Vec<(Duration, u64)> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= from + duration {
            estimator.add_snapshot(bytes_at(t), None, start + t);
            out.push((t, estimator.bps()));
            t += tick;
        }
        out
    }

    #[test]
    fn isolated_blocks_neither_spike_nor_drop_to_zero() {
        // démarrage au compte-gouttes : un bloc de 16 Kio toutes les 1,5 s, relevés toutes les
        // 100 ms (comme le moteur). Avant : 40 Kio/s puis 0, en alternance.
        let e = SpeedEstimator::default();
        let start = Instant::now();
        let first_block = Duration::from_millis(50);
        let samples = run(
            &e,
            start,
            Duration::ZERO,
            Duration::from_secs(12),
            Duration::from_millis(100),
            |t| {
                if t < first_block {
                    0
                } else {
                    (1 + (t - first_block).as_millis() as u64 / 1500) * 16 * KIB
                }
            },
        );
        for (t, bps) in samples {
            // débit réel ~11 Kio/s ; 2 blocs à 1,6 s du début donnent au plus 20 Kio/s
            assert!(bps <= 21 * KIB, "pic à {bps} o/s à {t:?}");
            if t > first_block {
                assert!(bps > 0, "retombé à 0 à {t:?}");
            }
        }
    }

    #[test]
    fn steady_download_is_measured_over_one_second() {
        // 1 Mio/s pendant 4 s, puis 4 Mio/s : une seconde plus tard, la nouvelle vitesse est
        // affichée (une moyenne sur 5 s donnerait encore ~1,6 Mio/s)
        let e = SpeedEstimator::default();
        let start = Instant::now();
        let bytes_at = |t: Duration| {
            let s = t.as_secs_f64();
            if s <= 4.0 {
                (s * MIB as f64) as u64
            } else {
                (4.0 * MIB as f64 + (s - 4.0) * 4.0 * MIB as f64) as u64
            }
        };
        let samples = run(
            &e,
            start,
            Duration::ZERO,
            Duration::from_millis(5100),
            Duration::from_millis(100),
            bytes_at,
        );
        let (_, at_4s) = samples[40];
        assert!(at_4s.abs_diff(MIB) < MIB / 20, "{at_4s}");
        let (_, last) = *samples.last().unwrap();
        assert!(last.abs_diff(4 * MIB) < MIB / 5, "{last}");
    }

    #[test]
    fn one_second_ticks_with_timer_jitter() {
        // estimateur de la session : un relevé par seconde, parfois un peu en avance
        let e = SpeedEstimator::new(5);
        let start = Instant::now();
        let mut t = Duration::ZERO;
        for _ in 0..10 {
            e.add_snapshot(
                (t.as_secs_f64() * 100.0 * KIB as f64) as u64,
                None,
                start + t,
            );
            t += Duration::from_millis(998);
        }
        assert!(e.bps().abs_diff(100 * KIB) < KIB, "{}", e.bps());
    }

    #[test]
    fn stopped_download_reaches_zero() {
        let e = SpeedEstimator::default();
        let start = Instant::now();
        let stop = Duration::from_secs(3);
        let bytes_at = |t: Duration| (t.min(stop).as_secs_f64() * MIB as f64) as u64;
        let samples = run(
            &e,
            start,
            Duration::ZERO,
            Duration::from_secs(10),
            Duration::from_millis(100),
            bytes_at,
        );
        let (_, last) = *samples.last().unwrap();
        assert_eq!(last, 0);
        // et la vitesse ne remonte jamais une fois arrêté
        let after: Vec<u64> = samples
            .iter()
            .filter(|(t, _)| *t > stop)
            .map(|(_, b)| *b)
            .collect();
        assert!(after.windows(2).all(|w| w[1] <= w[0]), "{after:?}");
    }

    #[test]
    fn time_remaining_follows_the_measured_speed() {
        let e = SpeedEstimator::default();
        let start = Instant::now();
        for i in 0..=20u64 {
            e.add_snapshot(
                i * 100 * KIB,
                Some(10 * MIB),
                start + Duration::from_millis(i * 100),
            );
        }
        // 1 Mio/s environ, 10 Mio restants
        let secs = e.time_remaining().unwrap().as_secs_f64();
        assert!((secs - 10.0).abs() < 0.5, "{secs}");
    }
}
