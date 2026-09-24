//! Historique des débits (graphique de l'interface) : un point par seconde sur les 5 dernières
//! minutes, pour l'ensemble et pour chaque téléchargement. En mémoire seulement.

use std::collections::VecDeque;

use super::*;

/// 5 minutes à un point par seconde.
pub(super) const POINTS: usize = 300;

#[derive(Default)]
pub(super) struct Series {
    samples: VecDeque<(u64, u64)>,
}

impl Series {
    pub fn push(&mut self, down: u64, up: u64) {
        if self.samples.len() == POINTS {
            self.samples.pop_front();
        }
        self.samples.push_back((down, up));
    }

    /// Du plus ancien au plus récent ; complété par des zéros au début (courbe toujours sur 5 min).
    fn view(&self) -> SpeedHistory {
        let missing = POINTS - self.samples.len();
        let pad = std::iter::repeat_n(0, missing);
        SpeedHistory {
            down: pad.clone().chain(self.samples.iter().map(|s| s.0)).collect(),
            up: pad.chain(self.samples.iter().map(|s| s.1)).collect(),
        }
    }
}

#[derive(Default)]
pub(super) struct History {
    global: Series,
    jobs: HashMap<String, Series>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedHistory {
    /// Octets/s, un point par seconde, du plus ancien au plus récent.
    pub down: Vec<u64>,
    pub up: Vec<u64>,
}

impl History {
    /// Un point par téléchargement présent ; ceux qui ont disparu sont oubliés.
    pub fn record(&mut self, jobs: &[(String, u64, u64)]) {
        let (down, up) = jobs.iter().fold((0, 0), |(d, u), (_, jd, ju)| (d + jd, u + ju));
        self.global.push(down, up);
        self.jobs.retain(|id, _| jobs.iter().any(|(j, _, _)| j == id));
        for (id, down, up) in jobs {
            self.jobs.entry(id.clone()).or_default().push(*down, *up);
        }
    }

    pub fn view(&self, id: Option<&str>) -> SpeedHistory {
        match id {
            None => self.global.view(),
            Some(id) => self.jobs.get(id).map(Series::view).unwrap_or_else(|| Series::default().view()),
        }
    }
}

impl Manager {
    /// Appelé chaque seconde par la boucle principale.
    pub(super) fn record_speeds(&self, views: &[TorrentView]) {
        let mut jobs: Vec<(String, u64, u64)> =
            views.iter().map(|v| (format!("t:{}", v.info_hash), v.down_speed, v.up_speed)).collect();
        jobs.extend(self.directs.lock().iter().map(|(id, runtime)| (format!("d:{id}"), runtime.speed, 0)));
        self.history.lock().record(&jobs);
    }

    /// Historique d'un téléchargement (`id`), ou de l'ensemble.
    pub fn speed_history(&self, id: Option<&str>) -> SpeedHistory {
        self.history.lock().view(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_five_minutes_and_forgets_removed_downloads() {
        let mut history = History::default();
        for second in 0..400u64 {
            history.record(&[("t:a".into(), second, 1), ("d:b".into(), 10, 0)]);
        }
        let global = history.view(None);
        assert_eq!(global.down.len(), POINTS);
        assert_eq!(*global.down.last().unwrap(), 399 + 10, "somme des téléchargements");
        assert_eq!(global.down[0], 100 + 10, "les 100 premières secondes sont sorties de la fenêtre");
        assert_eq!(history.view(Some("t:a")).up, vec![1; POINTS]);

        history.record(&[("t:a".into(), 5, 0)]);
        assert!(history.view(Some("d:b")).down.iter().all(|&v| v == 0), "téléchargement retiré : oublié");
    }

    #[test]
    fn a_new_download_starts_with_an_empty_curve() {
        let mut history = History::default();
        history.record(&[("t:a".into(), 7, 3)]);
        let view = history.view(Some("t:a"));
        assert_eq!(view.down.len(), POINTS);
        assert_eq!(&view.down[POINTS - 2..], &[0, 7]);
    }
}
