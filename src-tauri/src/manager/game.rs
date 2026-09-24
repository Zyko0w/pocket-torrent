//! Mode jeu : limites réduites ou pause quand un jeu est lancé.

use super::*;

impl Manager {
    fn effective_limits(&self) -> SpeedLimits {
        let settings = self.settings.read();
        let game = self.game.lock();
        let gm = &settings.game_mode;
        // planificateur (§5) : remplace les limites normales pendant sa plage horaire
        let base = if settings.schedule.active_now() {
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
            down_kbps: lowest(
                base.down_kbps,
                if game.active && gm.action == GameAction::Limit { gm.down_kbps } else { 0 },
            ),
            up_kbps: lowest(base.up_kbps, if game.active { gm.up_kbps } else { 0 }),
        }
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
