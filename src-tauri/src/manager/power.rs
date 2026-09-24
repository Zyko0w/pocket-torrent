//! « Éteindre le PC quand tout est terminé » : option valable pour la session en cours
//! seulement (jamais mémorisée, pour ne pas éteindre le PC par surprise au lancement suivant).
//! L'arrêt passe par `shutdown.exe` avec un délai : Windows affiche son propre avertissement,
//! et l'interface propose d'annuler pendant ce temps.

use super::*;

/// Délai entre la fin des téléchargements et l'arrêt du PC.
const SHUTDOWN_DELAY_SECS: u64 = 60;

#[derive(Default)]
pub(super) struct PowerState {
    /// Option activée par l'utilisateur.
    armed: bool,
    /// Il y avait encore quelque chose en cours à la vérification précédente.
    was_busy: bool,
    /// Activation (ms depuis 1970) : l'arrêt n'a lieu que si un téléchargement s'est terminé
    /// depuis (tout mettre en pause à la main, ou vider la liste, n'éteint pas le PC).
    armed_at: u64,
    /// Arrêt programmé : heure prévue (ms depuis 1970).
    shutdown_at: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerView {
    pub shutdown_when_done: bool,
    pub shutdown_at: Option<u64>,
}

/// Téléchargement pas encore terminé : en cours, en attente d'une place ou d'une condition
/// (mode jeu, VPN, disque) qui le relancera tout seul. Une pause manuelle ne compte pas.
fn is_busy(job: &JobView) -> bool {
    match job.status {
        "metadata" | "checking" | "connecting" | "queued" | "downloading" | "uploading" => true,
        "paused" => matches!(job.hold, Some(Hold::Game | Hold::Vpn | Hold::Disk | Hold::Queue)),
        _ => false,
    }
}

impl Manager {
    pub fn power_view(&self) -> PowerView {
        let power = self.power.lock();
        PowerView { shutdown_when_done: power.armed, shutdown_at: power.shutdown_at }
    }

    pub fn set_shutdown_when_done(&self, enabled: bool) -> anyhow::Result<()> {
        let jobs = self.overview().jobs;
        let busy = jobs.iter().any(is_busy);
        if enabled && !busy {
            bail!("Aucun téléchargement en cours : rien à attendre avant d'éteindre.");
        }
        let mut power = self.power.lock();
        power.armed = enabled;
        power.was_busy = busy;
        power.armed_at = now_ms();
        tracing::info!("arrêt du PC à la fin des téléchargements : {}", if enabled { "activé" } else { "désactivé" });
        Ok(())
    }

    /// Appelé chaque seconde par la boucle : programme l'arrêt quand plus rien n'est en cours.
    pub(super) fn check_shutdown_when_done(&self) {
        if !self.power.lock().armed {
            return;
        }
        let jobs = self.overview().jobs;
        let busy = jobs.iter().any(is_busy);
        {
            let mut power = self.power.lock();
            let completed_since = jobs.iter().any(|j| j.finished_at.is_some_and(|t| t >= power.armed_at));
            let finished = power.was_busy && !busy && completed_since;
            power.was_busy = busy;
            if !power.armed || !finished {
                return;
            }
            power.armed = false;
        }
        self.save_now();
        match system::schedule_shutdown(SHUTDOWN_DELAY_SECS) {
            Ok(()) => {
                self.power.lock().shutdown_at = Some(now_ms() + SHUTDOWN_DELAY_SECS * 1000);
                tracing::info!("téléchargements terminés : arrêt du PC dans {SHUTDOWN_DELAY_SECS} s");
                self.notify(
                    Notify::Done,
                    "Téléchargements terminés",
                    "Le PC s'éteint dans une minute. Tu peux annuler depuis Pocket Torrent.",
                );
            }
            Err(err) => tracing::error!("arrêt du PC impossible : {err:#}"),
        }
    }

    pub fn cancel_shutdown(&self) -> anyhow::Result<()> {
        system::cancel_shutdown()?;
        self.power.lock().shutdown_at = None;
        tracing::info!("arrêt du PC annulé");
        Ok(())
    }
}
