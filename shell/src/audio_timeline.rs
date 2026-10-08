//! Shared missile clocks and projectile identity across the impact tick.

use crate::game::SoundKind;

pub(crate) fn missile_ejection_ticks(total: f32) -> f32 {
    3.0_f32.min(total * 0.25)
}

/// Tick reports are consumed after the launch tick has completed.
pub(crate) fn projectile_elapsed_ticks(now: f32, arrival: u64, total: f32) -> f32 {
    total - (arrival as f32 + 1.0 - now)
}

#[derive(Default)]
pub(crate) struct AudioTimeline {
    arrivals: Vec<oxide_sim::state::Shell>,
}

impl AudioTimeline {
    pub(crate) fn remember_arrivals(&mut self, state: &oxide_sim::State) {
        self.arrivals = state
            .shells()
            .iter()
            .filter(|shell| shell.arrival <= state.current_tick())
            .cloned()
            .collect();
    }

    pub(crate) fn landed(
        &mut self,
        player: oxide_sim::PlayerId,
        at: chassis::fx::Vec2Fx,
    ) -> SoundKind {
        let Some(index) = self
            .arrivals
            .iter()
            .position(|shell| shell.player == player && shell.impact == at)
        else {
            return SoundKind::Artillery;
        };
        if self.arrivals.remove(index).kind == oxide_sim::ProjectileKind::Missile {
            SoundKind::RocketImpact
        } else {
            SoundKind::Artillery
        }
    }

    pub(crate) fn clear(&mut self) {
        self.arrivals.clear();
    }
}

#[cfg(test)]
mod tests;
