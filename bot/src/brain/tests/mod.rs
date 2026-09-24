use super::*;
use crate::SeatBot;
use crate::Specialty;
use crate::lift::LiftPhase;
use crate::observation::{BuildingObs, UnitObs};
use crate::test_support::{
    assert_commands_accepted, building_spec, home_foundry, player_spec, unit_spec,
};
use chassis::grid::TilePos;
use oxide_sim::State;
use oxide_sim::ids::{PlayerId, Target};
use oxide_sim::scenario::{BotDifficulty, BotStance, Scenario};
use oxide_sim::state::Faction;
use oxide_sim::stats::{BuildingKind, Role, UnitKind};

use crate::allocation::prior_planner_claims;
use crate::executive::ArmyState;
use crate::lift::{LiftAdmission, LiftAirSupport};
use crate::resources::ResourceSnapshot;
use crate::strategy::{AirOperationOutcome, AirRecoveryReason};
use crate::test_support::operations::*;
use crate::{
    executive::Intent,
    strategy::{AirOperationPhase, StrategicCoordination, StrategicThinkContext},
    utility::combat_core_status,
};
use crate::{
    team::TeamReliefAdmission,
    trace::{ChannelPhase, ChannelState},
};
use oxide_sim::command::Command;
use oxide_sim::ids::UnitId;
mod fixtures;
use fixtures::*;
mod contracts;
mod core_protection;
mod expansion;
mod operations;
mod production;
mod tracing;
