//! Mode jeu : limites réduites ou pause quand un jeu est lancé.

use super::*;

/// Limites appliquées : le planificateur (§5) remplace les limites normales pendant sa plage
/// horaire, puis le mode jeu abaisse le tout (la plus basse l'emporte, 0 = illimitée). En mode
/// « pause », la réception est arrêtée ailleurs : seul l'envoi est limité ici.
fn combined_limits(settings: &Settings, schedule_active: bool, game_active: bool) -> SpeedLimits {
    let gm = &settings.game_mode;
    let base = if schedule_active {
        SpeedLimits { down_kbps: settings.schedule.down_kbps, up_kbps: settings.schedule.up_kbps }
    } else {
        settings.limits
    };
    let lowest = |a: u32, b: u32| match (a, b) {
        (0, b) => b,
        (a, 0) => a,
        (a, b) => a.min(b),
    };
    SpeedLimits {
        down_kbps: lowest(base.down_kbps, if game_active && gm.action == GameAction::Limit { gm.down_kbps } else { 0 }),
        up_kbps: lowest(base.up_kbps, if game_active { gm.up_kbps } else { 0 }),
    }
}

impl Manager {
    pub(super) fn effective_limits(&self) -> SpeedLimits {
        let settings = self.settings.read();
        combined_limits(&settings, settings.schedule.active_now(), self.game.lock().active)
    }

    pub(super) fn apply_limits(&self) {
        self.engine.set_limits(&self.effective_limits());
    }

    pub(super) async fn check_game(self: &Arc<Self>) {
        let gm = self.settings.read().game_mode.clone();
        let (active, process) = match gm.mode {
            GameModeSwitch::On => (true, None),
            GameModeSwitch::Off => (false, None),
            GameModeSwitch::Auto if gm.processes.is_empty() => (false, None),
            GameModeSwitch::Auto => {
                let running = self.processes.lock().running();
                let found = gm.processes.iter().find(|p| running.contains(&system::normalize_exe(p))).cloned();
                (found.is_some(), found)
            }
        };
        let previous = {
            let mut game = self.game.lock();
            let previous = game.clone();
            *game = GameState { active, process: process.clone(), checked: true };
            previous
        };
        if previous.checked && previous.active == active && previous.process == process {
            return;
        }
        self.apply_limits();

        if active && gm.action == GameAction::Pause {
            let (running, _) = self.slots(&self.engine.torrents());
            for slot in running {
                self.hold(&slot.key, Hold::Game).await;
            }
        } else {
            // fin du jeu (ou simple limitation) : ce que le mode jeu avait arrêté repart via la file
            self.release_held(Hold::Game);
        }
        if previous.checked || active {
            tracing::info!(active, ?process, "mode jeu");
        }
        self.mark_dirty();
    }

    pub async fn set_game_mode(self: &Arc<Self>, mode: GameModeSwitch) -> anyhow::Result<()> {
        let settings = {
            let mut settings = self.settings.write();
            settings.game_mode.mode = mode;
            settings.clone()
        };
        settings.save(&self.settings_path)?;
        self.game.lock().checked = false;
        self.check_game().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_game_mode_combine_to_the_lowest_limit() {
        let mut settings = Settings { limits: SpeedLimits { down_kbps: 5000, up_kbps: 0 }, ..Default::default() };
        settings.schedule.down_kbps = 0;
        settings.schedule.up_kbps = 300;
        settings.game_mode.down_kbps = 1000;
        settings.game_mode.up_kbps = 100;
        settings.game_mode.action = GameAction::Limit;
        let limits = |settings: &Settings, schedule, game| {
            let l = combined_limits(settings, schedule, game);
            (l.down_kbps, l.up_kbps)
        };
        assert_eq!(limits(&settings, false, false), (5000, 0), "limites normales");
        assert_eq!(limits(&settings, true, false), (0, 300), "planificateur : remplace, 0 = illimitée");
        assert_eq!(limits(&settings, false, true), (1000, 100), "jeu : la plus basse");
        assert_eq!(limits(&settings, true, true), (1000, 100));
        settings.game_mode.action = GameAction::Pause;
        assert_eq!(
            limits(&settings, false, true),
            (5000, 100),
            "pause : l'envoi reste limité, la réception est arrêtée par la file"
        );
    }
}
