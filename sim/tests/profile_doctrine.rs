//! Named-profile opening doctrine stays deterministic, finite, and visible in
//! the same action mask native inference and the external gym consume.

use oxide_sim::bot::{
    ACTION_HEADS, Action, ActionPlan, CONSTRUCTION_ACTIONS, GymBot, PRODUCTION_ACTIONS,
    PROFILE_TEAM_ROLES, ProfileFacets, canonical_profiles,
};
use oxide_sim::scenario::{BuildingSpec, TeamRole, UnitSpec};
use oxide_sim::{BuildingKind, Faction, Order, PlayerId, Scenario, State, UnitKind};

fn facets(economy: u32, air: u32, siege: u32, support: u32) -> ProfileFacets {
    ProfileFacets {
        economy_bias: economy,
        air_bias: air,
        siege_bias: siege,
        support_bias: support,
        commitment_bias: 0,
    }
}

fn profiled(facets: ProfileFacets) -> GymBot {
    GymBot::with_profile_facets(PlayerId(0), 8, facets)
}

#[test]
fn canonical_catalog_reserves_the_commitment_ceiling_for_vanguard() {
    for profile in canonical_profiles() {
        for role in PROFILE_TEAM_ROLES {
            let commitment = profile.facets.with_role(role).commitment_bias;
            if role == TeamRole::Vanguard {
                assert_eq!(
                    commitment, 1_000,
                    "{} Vanguard must carry the exact finite-screen marker",
                    profile.name
                );
            } else {
                assert!(
                    commitment < 1_000,
                    "{} {role:?} must preserve the learned opening, got {commitment}",
                    profile.name
                );
            }
        }
    }
}

fn only_enabled(mask: &[bool], head: &[usize]) -> Vec<usize> {
    head.iter()
        .copied()
        .filter(|action| mask[*action])
        .collect()
}

fn add_harvesters(scenario: &mut Scenario, total: usize) {
    let current = scenario
        .units
        .iter()
        .filter(|unit| unit.player == 0 && unit.kind == UnitKind::Harvester)
        .count();
    for (index, (x, y)) in [(8, 5), (8, 6), (9, 5), (9, 6)]
        .into_iter()
        .take(total.saturating_sub(current))
        .enumerate()
    {
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x,
            y: y + i32::try_from(index / 4).unwrap(),
        });
    }
}

fn add_home_screen(scenario: &mut Scenario) {
    add_sentinels(scenario, 5);
}

fn add_sentinels(scenario: &mut Scenario, total: usize) {
    let current = scenario
        .units
        .iter()
        .filter(|unit| unit.player == 0 && unit.kind == UnitKind::Sentinel)
        .count();
    for (x, y) in [(10, 5), (10, 6), (10, 7), (11, 5)]
        .into_iter()
        .skip(current.saturating_sub(1))
        .take(total.saturating_sub(current))
    {
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x,
            y,
        });
    }
}

fn add_building(scenario: &mut Scenario, kind: BuildingKind, x: i32, y: i32) {
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind,
        x,
        y,
    });
}

fn advanced_state_for(faction: Faction, units: &[UnitKind]) -> State {
    let mut scenario = Scenario::skirmish();
    scenario.retint_seat(0, faction);
    scenario.players[0].scrap = 1_000;
    add_building(&mut scenario, BuildingKind::Fabricator, 9, 2);
    // The closed tree homes the sky at the Airworks, so the advanced
    // fixture stands one — air-unit training masks off without it.
    add_building(&mut scenario, BuildingKind::Airworks, 13, 2);
    for (index, kind) in units.iter().copied().enumerate() {
        scenario.units.push(UnitSpec {
            player: 0,
            kind,
            x: 10 + i32::try_from(index % 2).unwrap(),
            y: 5 + i32::try_from(index / 2).unwrap(),
        });
    }
    scenario.build().unwrap()
}

fn advanced_state(units: &[UnitKind]) -> State {
    advanced_state_for(Faction::Ferrous, units)
}

#[test]
fn zero_facets_are_mask_command_and_hash_inert() {
    let mut ordinary = GymBot::with_cadence(PlayerId(0), 8);
    let mut explicit = GymBot::with_profile_facets(PlayerId(0), 8, ProfileFacets::ZERO);
    let mut ordinary_state = Scenario::skirmish().build().unwrap();
    let mut explicit_state = ordinary_state.clone();

    for _ in 0..256 {
        let ordinary_decision = ordinary.decision(&ordinary_state);
        let explicit_decision = explicit.decision(&explicit_state);
        assert_eq!(ordinary_decision.features, explicit_decision.features);
        assert_eq!(ordinary_decision.mask, explicit_decision.mask);
        let picks = std::array::from_fn(|head| {
            ACTION_HEADS[head]
                .iter()
                .rev()
                .copied()
                .find(|action| ordinary_decision.mask[*action])
                .unwrap()
        });
        let plan = ActionPlan::from_indices(picks);
        let ordinary_commands = ordinary.step_plan(&ordinary_state, plan);
        let explicit_commands = explicit.step_plan(&explicit_state, plan);
        assert_eq!(ordinary_commands, explicit_commands);
        ordinary_state.tick(&ordinary_commands);
        explicit_state.tick(&explicit_commands);
        assert_eq!(ordinary_state.hash(), explicit_state.hash());
    }
}

#[test]
fn industry_compounds_then_fields_one_mixed_reserve_and_reclaimer() {
    let industry = facets(800, 0, 0, 0);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    let state = scenario.build().unwrap();
    let decision = profiled(industry).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainHarvester as usize]
    );
    assert!(
        decision.mask[Action::NoConstruction as usize],
        "rich nearby opening patches must not turn a Reclaimer into an opening build"
    );

    add_harvesters(&mut scenario, 5);
    let rich = scenario.build().unwrap();
    let decision = profiled(industry).decision(&rich);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainSentinel as usize],
        "the fifth worker unlocks the bounded Fabricator safety screen"
    );

    add_home_screen(&mut scenario);
    let decision = profiled(industry).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildFabricator as usize]
    );

    let mut exhausted_before_tech = scenario.clone();
    for row in &mut exhausted_before_tech.map {
        *row = row.replace(['s', 'S'], ".");
    }
    let decision = profiled(industry).decision(&exhausted_before_tech.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildReclaimer as usize],
        "an industrial retirement plan outranks delayed tech once the home field is exhausted"
    );

    add_building(&mut scenario, BuildingKind::Fabricator, 9, 2);
    let decision = profiled(industry).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainScuttler as usize],
        "the cheapest mixed-reserve body comes online first"
    );
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Scuttler,
        x: 16,
        y: 5,
    });
    let decision = profiled(industry).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAntiAir as usize]
    );
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Flakhound,
        x: 17,
        y: 5,
    });
    let decision = profiled(industry).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainLancer as usize]
    );
    scenario.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Lancer,
        x: 18,
        y: 5,
    });
    let decision = profiled(industry).decision(&scenario.build().unwrap());
    assert!(
        decision.mask[Action::TrainScuttler as usize]
            && decision.mask[Action::TrainLancer as usize]
            && decision.mask[Action::Idle as usize],
        "the production head releases after the finite mixed reserve"
    );

    let mut once = profiled(industry);
    let _ = once.decision(&scenario.build().unwrap());
    let mut lost_scout = scenario.clone();
    lost_scout
        .units
        .retain(|unit| unit.player != 0 || unit.kind != UnitKind::Scuttler);
    let decision = once.decision(&lost_scout.build().unwrap());
    assert!(
        decision.mask[Action::TrainScuttler as usize]
            && decision.mask[Action::TrainLancer as usize]
            && decision.mask[Action::Idle as usize],
        "losing the one Scuttler must not turn the opening into a standing quota"
    );

    for row in &mut scenario.map {
        *row = row.replace(['s', 'S'], ".");
    }
    let depleted = scenario.build().unwrap();
    let mut bot = profiled(industry);
    let decision = bot.decision(&depleted);
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildReclaimer as usize]
    );

    let commands = bot.step_plan(
        &depleted,
        ActionPlan {
            construction: Action::BuildReclaimer,
            ..ActionPlan::default()
        },
    );
    let mut site = depleted;
    site.tick(&commands);
    let released = bot.decision(&site);
    assert!(
        released.mask[Action::NoConstruction as usize],
        "an unfinished paid site satisfies the finite Reclaimer milestone"
    );

    let mut cupric = Scenario::skirmish();
    cupric.retint_seat(0, Faction::Cupric);
    cupric.players[0].scrap = 1_000;
    add_harvesters(&mut cupric, 5);
    add_home_screen(&mut cupric);
    add_building(&mut cupric, BuildingKind::Fabricator, 9, 2);
    cupric.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Lancer,
        x: 16,
        y: 5,
    });
    cupric.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Scuttler,
        x: 17,
        y: 5,
    });
    let decision = profiled(industry).decision(&cupric.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAntiAir as usize]
    );
    cupric.units.push(UnitSpec {
        player: 0,
        kind: UnitKind::Stinger,
        x: 18,
        y: 5,
    });
    let decision = profiled(industry).decision(&cupric.build().unwrap());
    assert!(
        decision.mask[Action::TrainScuttler as usize]
            && decision.mask[Action::TrainLancer as usize],
        "Cupric industry recognizes its Stinger as the faction AA commitment"
    );
}

#[test]
fn vanguard_commitment_fields_one_direct_ground_screen_then_releases_production() {
    let solo_aggressive = ProfileFacets {
        commitment_bias: 950,
        ..ProfileFacets::ZERO
    };
    let ordinary = profiled(solo_aggressive).decision(&Scenario::skirmish().build().unwrap());
    assert!(
        ordinary.mask[Action::Idle as usize]
            && ordinary.mask[Action::TrainHarvester as usize]
            && ordinary.mask[Action::TrainSentinel as usize],
        "solo Aggressive profiles retain the learned opening instead of inheriting a team job"
    );

    let vanguard_commitment = ProfileFacets {
        commitment_bias: 1000,
        ..ProfileFacets::ZERO
    };
    let mut bot = profiled(vanguard_commitment);
    let scenario = Scenario::skirmish();
    let decision = bot.decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainSentinel as usize],
        "the shipped one-Sentinel opening needs two more direct ground bodies"
    );

    let mut indirect_only = scenario.clone();
    for (index, kind) in [UnitKind::Bombard, UnitKind::Flakhound]
        .into_iter()
        .enumerate()
    {
        indirect_only.units.push(UnitSpec {
            player: 0,
            kind,
            x: 10 + i32::try_from(index).unwrap(),
            y: 5,
        });
    }
    let decision = bot.decision(&indirect_only.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainSentinel as usize],
        "artillery and anti-air do not satisfy a direct ground screen"
    );

    let mut committed = serde_json::to_value(scenario.build().unwrap()).unwrap();
    let foundry = committed["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "foundry")
        .unwrap();
    foundry["queue"] = serde_json::json!(["sentinel", "sentinel"]);
    let committed: State = serde_json::from_value(committed).unwrap();
    let decision = bot.decision(&committed);
    assert!(
        decision.mask[Action::Idle as usize]
            && only_enabled(&decision.mask, &PRODUCTION_ACTIONS)
                != vec![Action::TrainSentinel as usize],
        "live and queued direct fighters satisfy the finite opening while the bodies are still queued"
    );

    let decision = bot.decision(&scenario.build().unwrap());
    assert!(
        decision.mask[Action::Idle as usize]
            && decision.mask[Action::TrainHarvester as usize]
            && decision.mask[Action::TrainSentinel as usize],
        "the Vanguard screen is a one-way milestone, not a replacement quota"
    );
}

#[test]
fn team_industry_lean_keeps_the_reclaimer_without_inheriting_the_full_package() {
    let team_industry = facets(700, 0, 0, 0);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    add_harvesters(&mut scenario, 5);
    add_home_screen(&mut scenario);
    add_building(&mut scenario, BuildingKind::Fabricator, 9, 2);
    for row in &mut scenario.map {
        *row = row.replace(['s', 'S'], ".");
    }

    let decision = profiled(team_industry).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildReclaimer as usize]
    );
    assert!(
        decision.mask[Action::TrainScuttler as usize]
            && decision.mask[Action::TrainLancer as usize]
            && decision.mask[Action::Idle as usize],
        "the complementary role must not inherit the strong profile's mixed-army quota"
    );
}

#[test]
fn air_package_commits_tech_then_one_of_each_combined_arm() {
    let air = facets(0, 800, 0, 0);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 0;
    let decision = profiled(air).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainHarvester as usize],
        "advanced doctrine first meets the Fabricator's worker prerequisite"
    );

    add_harvesters(&mut scenario, 4);
    let decision = profiled(air).decision(&scenario.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainSentinel as usize],
        "advanced doctrine next meets the Fabricator's screen prerequisite"
    );

    add_sentinels(&mut scenario, 4);
    let queued_screen = scenario.build().unwrap();
    let mut queued_screen = serde_json::to_value(queued_screen).unwrap();
    let foundry = queued_screen["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "foundry")
        .unwrap();
    foundry["queue"] = serde_json::json!(["sentinel"]);
    let queued_screen: State = serde_json::from_value(queued_screen).unwrap();
    let decision = profiled(air).decision(&queued_screen);
    assert!(
        decision.mask[Action::TrainHarvester as usize]
            && decision.mask[Action::TrainSentinel as usize],
        "a queued screen unit counts toward the advanced prerequisite"
    );

    add_home_screen(&mut scenario);
    let state = scenario.build().unwrap();
    let decision = profiled(air).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildFabricator as usize],
        "advanced doctrine must establish a saved tech intention before the bank can afford it"
    );

    // On the closed tree the sky lives at the Airworks: once the
    // Fabricator stands, the air lean owes its own hangar before any
    // wing can queue.
    let mut hangar_pending = Scenario::skirmish();
    hangar_pending.players[0].scrap = 1_000;
    add_building(&mut hangar_pending, BuildingKind::Fabricator, 9, 2);
    let decision = profiled(air).decision(&hangar_pending.build().unwrap());
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildAirworks as usize],
        "an air lean with a standing Fabricator commits to the Airworks next"
    );

    let state = advanced_state(&[]);
    let decision = profiled(air).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAirGround as usize]
    );

    let mut queued = serde_json::to_value(&state).unwrap();
    let airworks = queued["buildings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|building| building["player"] == 0 && building["kind"] == "airworks")
        .unwrap();
    airworks["queue"] = serde_json::json!(["buzzard"]);
    let queued: State = serde_json::from_value(queued).unwrap();
    let decision = profiled(air).decision(&queued);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAirAir as usize],
        "a queued signature unit counts as committed"
    );

    let state = advanced_state(&[UnitKind::Buzzard, UnitKind::Talon]);
    let decision = profiled(air).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainLancer as usize]
    );

    let state = advanced_state(&[UnitKind::Buzzard, UnitKind::Talon, UnitKind::Lancer]);
    let decision = profiled(air).decision(&state);
    assert!(
        decision.mask[Action::TrainScuttler as usize]
            && decision.mask[Action::TrainAirGround as usize]
            && decision.mask[Action::Idle as usize],
        "the production head releases once all three finite milestones stand"
    );

    let cupric = advanced_state_for(Faction::Cupric, &[]);
    let decision = profiled(air).decision(&cupric);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAirGround as usize]
    );
    let cupric = advanced_state_for(Faction::Cupric, &[UnitKind::Darter]);
    let decision = profiled(air).decision(&cupric);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainAirAir as usize],
        "Cupric air doctrine maps roles to Darter then Wisp"
    );
}

#[test]
fn siege_and_support_commit_once_then_release_their_heads() {
    let siege = facets(0, 0, 800, 0);
    let state = advanced_state(&[]);
    let decision = profiled(siege).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &PRODUCTION_ACTIONS),
        vec![Action::TrainBombard as usize]
    );
    let state = advanced_state(&[UnitKind::Bombard]);
    let decision = profiled(siege).decision(&state);
    assert!(decision.mask[Action::TrainSentinel as usize]);

    let support = facets(0, 0, 0, 800);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    let state = scenario.build().unwrap();
    let decision = profiled(support).decision(&state);
    assert_eq!(
        only_enabled(&decision.mask, &CONSTRUCTION_ACTIONS),
        vec![Action::BuildTurret as usize]
    );
    add_building(&mut scenario, BuildingKind::Turret, 10, 5);
    let state = scenario.build().unwrap();
    let decision = profiled(support).decision(&state);
    assert!(decision.mask[Action::NoConstruction as usize]);
}

#[test]
fn completed_opening_commitments_do_not_relock_after_losses() {
    let all = facets(800, 800, 800, 800);
    let mut achieved = Scenario::skirmish();
    achieved.players[0].scrap = 1_000;
    add_harvesters(&mut achieved, 5);
    add_home_screen(&mut achieved);
    add_building(&mut achieved, BuildingKind::Fabricator, 9, 2);
    // The air lean's closed-tree opening includes its Airworks; without
    // one the milestone would legitimately still be owed after losses.
    add_building(&mut achieved, BuildingKind::Airworks, 13, 2);
    add_building(&mut achieved, BuildingKind::Reclaimer, 16, 2);
    add_building(&mut achieved, BuildingKind::Turret, 20, 2);
    for (index, kind) in [
        UnitKind::Buzzard,
        UnitKind::Talon,
        UnitKind::Lancer,
        UnitKind::Flakhound,
        UnitKind::Scuttler,
        UnitKind::Bombard,
    ]
    .into_iter()
    .enumerate()
    {
        achieved.units.push(UnitSpec {
            player: 0,
            kind,
            x: 16 + i32::try_from(index).unwrap(),
            y: 5,
        });
    }
    let mut bot = profiled(all);
    let _ = bot.decision(&achieved.build().unwrap());

    let mut after_losses = Scenario::skirmish();
    after_losses.players[0].scrap = 1_000;
    for row in &mut after_losses.map {
        *row = row.replace(['s', 'S'], ".");
    }
    let decision = bot.decision(&after_losses.build().unwrap());
    assert!(
        decision.mask[Action::Idle as usize]
            && decision.mask[Action::TrainHarvester as usize]
            && decision.mask[Action::TrainSentinel as usize],
        "losing the opening roster must not restore a standing production quota"
    );
    assert!(
        decision.mask[Action::NoConstruction as usize]
            && decision.mask[Action::BuildTurret as usize],
        "destroyed opening structures must not restore a standing capital quota"
    );
}

#[test]
fn saved_capital_and_tactical_reconciliation_outrank_profile_openings() {
    let air = facets(0, 800, 0, 0);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 100;
    add_harvesters(&mut scenario, 5);
    add_home_screen(&mut scenario);
    let state = scenario.build().unwrap();
    let mut bot = profiled(air);
    let _ = bot.step_plan(
        &state,
        ActionPlan {
            construction: Action::BuildArray,
            ..ActionPlan::default()
        },
    );
    let decision = bot.decision(&state);
    assert!(
        decision.mask[Action::BuildArray as usize]
            && decision.mask[Action::BuildFabricator as usize]
            && decision.mask[Action::NoConstruction as usize],
        "a profile must not replace or freeze an existing saved capital plan"
    );
    assert!(
        decision.mask[Action::TrainSentinel as usize]
            && decision.mask[Action::TrainHarvester as usize],
        "an existing saved capital plan suppresses profile production narrowing too"
    );

    let support = facets(0, 0, 0, 800);
    let mut threatened = Scenario::skirmish();
    threatened.players[0].scrap = 1_000;
    let enemy = threatened
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .unwrap();
    (enemy.x, enemy.y) = (10, 7);
    let decision = profiled(support).decision(&threatened.build().unwrap());
    assert!(decision.mask[Action::FormArmy as usize]);
    assert!(
        decision.mask[Action::NoConstruction as usize],
        "home-defense reconciliation leaves no simultaneous profile commitment"
    );
}

#[test]
fn deferred_building_commitment_prevents_a_duplicate_profile_goal() {
    let industry = facets(800, 0, 0, 0);
    let mut scenario = Scenario::skirmish();
    scenario.players[0].scrap = 1_000;
    add_harvesters(&mut scenario, 5);
    for row in &mut scenario.map {
        *row = row.replace(['s', 'S'], ".");
    }
    let state = scenario.build().unwrap();
    let mut value = serde_json::to_value(state).unwrap();
    value["units"][0]["order"] = serde_json::to_value(Order::Found {
        kind: BuildingKind::Reclaimer,
        anchor: chassis::grid::TilePos::new(15, 4),
    })
    .unwrap();
    let state: State = serde_json::from_value(value).unwrap();
    let decision = profiled(industry).decision(&state);
    assert!(
        !decision.mask[Action::BuildReclaimer as usize],
        "the unpaid walking claim is already the profile's committed Reclaimer"
    );
    assert!(decision.mask[Action::NoConstruction as usize]);
}
