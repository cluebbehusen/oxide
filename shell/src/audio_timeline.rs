//! Shared missile clocks and projectile identity across the impact tick.

use crate::game::SoundKind;

pub(crate) fn missile_ejection_ticks(total: f32) -> f32 {
    3.0_f32.min(total * 0.25)
}

/// Tick reports are consumed after the launch tick has completed.
pub(crate) fn missile_elapsed_ticks(now: f32, arrival: u64, total: f32) -> f32 {
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
mod tests {
    use super::*;

    #[test]
    fn coincident_impacts_keep_projectile_order_and_player_identity() {
        use chassis::fx::Vec2Fx;
        use oxide_sim::{PlayerId, ProjectileKind, Target, UnitId};

        let shell = |kind, player| oxide_sim::state::Shell {
            kind,
            shooter: Target::Unit(UnitId(42)),
            player,
            launch: Vec2Fx::ZERO,
            impact: Vec2Fx::ZERO,
            arrival: 10,
            damage: 1,
            targets: oxide_sim::stats::DomainMask::GROUND,
            splash: None,
        };
        let mut timeline = AudioTimeline {
            arrivals: vec![
                shell(ProjectileKind::Missile, PlayerId(1)),
                shell(ProjectileKind::Shell, PlayerId(0)),
                shell(ProjectileKind::Missile, PlayerId(0)),
            ],
        };
        assert_eq!(
            timeline.landed(PlayerId(0), Vec2Fx::ZERO),
            SoundKind::Artillery
        );
        assert_eq!(
            timeline.landed(PlayerId(0), Vec2Fx::ZERO),
            SoundKind::RocketImpact
        );
        assert_eq!(
            timeline.landed(PlayerId(1), Vec2Fx::ZERO),
            SoundKind::RocketImpact
        );
        assert_eq!(
            timeline.landed(PlayerId(0), Vec2Fx::ZERO),
            SoundKind::Artillery
        );
    }

    #[test]
    fn missile_pose_and_cues_share_the_post_tick_launch_origin() {
        assert_eq!(missile_elapsed_ticks(11.0, 30, 20.0), 0.0);
        assert_eq!(
            missile_elapsed_ticks(14.0, 30, 20.0),
            missile_ejection_ticks(20.0)
        );
        assert_eq!(missile_elapsed_ticks(31.0, 30, 20.0), 20.0);
        for total in [1.0, 4.0, 20.0] {
            assert!(missile_ejection_ticks(total) < total);
        }
    }
}
