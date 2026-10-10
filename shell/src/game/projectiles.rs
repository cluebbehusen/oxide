//! Launch facts retained only through their authoritative payload arrival.

use macroquad::prelude::{Vec2, vec2};
use oxide_sim::state::Shell;
use oxide_sim::{Event, ProjectileKind, State, Target, UnitKind};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct ProjectileReleases {
    releases: HashMap<(Target, u64), Vec<Flight>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LaunchPose {
    pub(crate) heading: Vec2,
    pub(crate) kind: UnitKind,
    pub(crate) slot: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Flight {
    pub(crate) target: Option<Target>,
    pub(crate) ticks: u64,
    pose: Option<LaunchPose>,
}

impl ProjectileReleases {
    pub(crate) fn observe(&mut self, state: &State, events: &[Event]) {
        // The landing report is read after the arrival tick has completed.
        self.releases
            .retain(|(_, arrival), _| *arrival >= state.current_tick().saturating_sub(1));
        let mut slots = HashMap::<Target, usize>::new();
        for event in events {
            if let Event::ShellLaunched {
                shooter,
                target,
                unit_pose,
                flight,
                ..
            } = event
            {
                let releases = self
                    .releases
                    .entry((*shooter, state.current_tick() - 1 + flight))
                    .or_default();
                let slot = slots.entry(*shooter).or_default();
                let pose = unit_pose.map(|pose| {
                    let heading = chassis::compass::dir(pose.heading);
                    LaunchPose {
                        heading: vec2(heading.x.to_num::<f32>(), heading.y.to_num::<f32>()),
                        kind: pose.kind,
                        slot: *slot,
                    }
                });
                releases.push(Flight {
                    target: *target,
                    ticks: *flight,
                    pose,
                });
                *slot += 1;
            }
        }
    }

    pub(crate) fn flight(&self, shells: &[Shell], index: usize) -> Option<Flight> {
        let shell = shells.get(index)?;
        let releases = self.releases.get(&(shell.shooter, shell.arrival))?;
        // Map-edge clamping can give several bombs the same arrival tick.
        // They retain launch order and are removed together.
        let occurrence = shells[..index]
            .iter()
            .filter(|earlier| earlier.shooter == shell.shooter && earlier.arrival == shell.arrival)
            .count();
        releases.get(occurrence).copied()
    }

    pub(crate) fn artillery_heading(&self, shell: &Shell) -> Option<Vec2> {
        self.releases
            .get(&(shell.shooter, shell.arrival))?
            .first()?
            .pose
            .filter(|pose| crate::look::fires(pose.kind, ProjectileKind::Shell))
            .map(|pose| pose.heading)
    }

    pub(crate) fn release(&self, shells: &[Shell], index: usize) -> Option<LaunchPose> {
        (shells.get(index)?.kind == ProjectileKind::Bomb)
            .then(|| self.flight(shells, index)?.pose)?
            .filter(|pose| crate::look::fires(pose.kind, ProjectileKind::Bomb))
    }
}

#[cfg(test)]
mod tests;
