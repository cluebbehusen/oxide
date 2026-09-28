//! Order programs as the staged commands will leave them.
//!
//! The orders dock and the waypoint chain show the selection's programs with
//! every staged command applied, so a chip clicked while paused, or while a
//! networked order is still on its way, names an order in the program that
//! command will actually edit.

use oxide_sim::{Building, BuildingId, Order, PlayerCommand, State, Tick, Unit, UnitId};

/// One unit's order program, active order first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Program {
    /// The active order, then the queue.
    pub(crate) orders: Vec<Order>,
    /// Whether finished orders rotate to the back (a patrol).
    pub(crate) looping: bool,
}

impl Program {
    fn of(unit: &Unit) -> Self {
        Self {
            orders: std::iter::once(&unit.order)
                .chain(&unit.queue)
                .copied()
                .collect(),
            looping: unit.looping,
        }
    }

    /// Whether the unit is idle with nothing queued.
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.orders[..], [Order::Idle])
    }
}

/// What a projection was captured from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Captured {
    tick: Tick,
    pending: usize,
    units: Vec<UnitId>,
}

/// The programs of the selection's decorated units after the staged
/// commands, with the sites their Build orders name. It is captured once per
/// tick, staged-command count and decorated selection: commands are only
/// ever appended to the staged batch or drained from it along with a tick.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    captured: Option<Captured>,
    programs: Vec<(UnitId, Program)>,
    sites: Vec<Building>,
}

impl Projection {
    pub(super) fn is_current(&self, tick: Tick, pending: usize, units: &[UnitId]) -> bool {
        self.captured.as_ref().is_some_and(|captured| {
            captured.tick == tick && captured.pending == pending && captured.units == units
        })
    }

    /// Projects `units` through `pending`. Only a staged batch needs the
    /// command phase; otherwise the programs are the world's own.
    pub(super) fn capture(state: &State, pending: &[PlayerCommand], units: Vec<UnitId>) -> Self {
        let (programs, sites) = if pending.is_empty() {
            (programs(&units, |id| state.unit(id)), Vec::new())
        } else {
            state.inspect_command_phase(pending, |view| {
                let programs = programs(&units, |id| view.unit(id));
                let sites = view
                    .buildings()
                    .iter()
                    .filter(|site| {
                        programs.iter().any(|(_, program)| {
                            program.orders.contains(&Order::Build { site: site.id })
                        })
                    })
                    .cloned()
                    .collect();
                (programs, sites)
            })
        };
        Self {
            captured: Some(Captured {
                tick: state.current_tick(),
                pending: pending.len(),
                units,
            }),
            programs,
            sites,
        }
    }

    /// A decorated unit's projected program.
    pub(crate) fn program(&self, id: UnitId) -> Option<&Program> {
        self.programs
            .iter()
            .find(|(unit, _)| *unit == id)
            .map(|(_, program)| program)
    }

    /// A building as the staged commands leave it, so a site a staged Build
    /// claims is already there.
    pub(crate) fn building<'a>(&'a self, state: &'a State, id: BuildingId) -> Option<&'a Building> {
        self.sites
            .iter()
            .find(|site| site.id == id)
            .or_else(|| state.building(id))
    }
}

fn programs<'u>(
    ids: &[UnitId],
    unit: impl Fn(UnitId) -> Option<&'u Unit>,
) -> Vec<(UnitId, Program)> {
    ids.iter()
        .filter_map(|&id| Some((id, Program::of(unit(id)?))))
        .collect()
}
