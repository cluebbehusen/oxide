use super::*;

#[test]
fn traced_and_untraced_player_facing_acts_are_behaviorally_identical() {
    let scenario = Scenario::skirmish();
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_113);
    let mut direct_state = scenario.build().expect("the skirmish builds");
    let mut first_traced_state = direct_state.clone();
    let mut second_traced_state = direct_state.clone();
    let mut direct = scripted_brain(&scenario, PlayerId(0), config);
    let mut first_traced = scripted_brain(&scenario, PlayerId(0), config);
    let mut second_traced = scripted_brain(&scenario, PlayerId(0), config);
    let mut traces = 0;

    for _ in 0..600 {
        let direct_commands = direct.act(&direct_state);
        let first = first_traced.act_traced(&first_traced_state);
        let second = second_traced.act_traced(&second_traced_state);

        assert_eq!(first.commands, direct_commands);
        assert_eq!(second.commands, direct_commands);
        assert_eq!(first.trace, second.trace);
        if let Some(trace) = &first.trace {
            traces += 1;
            assert_eq!(trace.tick, direct_state.current_tick());
            assert_eq!(trace.player, PlayerId(0));
            assert_eq!(
                trace.lowering.total_commands as usize,
                direct_commands.len()
            );
            assert_eq!(
                serde_json::to_string(trace).expect("the trace serializes"),
                serde_json::to_string(second.trace.as_ref().expect("the second trace exists"))
                    .expect("the second trace serializes")
            );
            for effects in [
                &trace.channels.team_relief.effects,
                &trace.channels.connected_air.effects,
                &trace.channels.lift.effects,
                &trace.channels.raid.effects,
            ] {
                assert!(effects.unit_claims.windows(2).all(|pair| pair[0] < pair[1]));
                assert!(effects.unit_claims.len() <= direct_state.units().len());
            }
        }

        direct_state.tick(&direct_commands);
        first_traced_state.tick(&first.commands);
        second_traced_state.tick(&second.commands);
        assert_eq!(first_traced_state.hash(), direct_state.hash());
        assert_eq!(second_traced_state.hash(), direct_state.hash());
    }

    assert_eq!(
        traces,
        600 / direct.dials.cadence,
        "the trace is bounded to actual decision ticks"
    );
    assert_brain_unchanged(&direct, &first_traced);
    assert_brain_unchanged(&direct, &second_traced);
}

#[test]
fn connected_package_trace_is_deterministic_and_behaviorally_observational() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the trace scenario has one connected-air scout");
    (scout.x, scout.y) = (42, 19);
    scenario.units.extend([
        unit_spec(0, UnitKind::Bombard, 9, 17),
        unit_spec(0, UnitKind::Condor, 10, 18),
        unit_spec(0, UnitKind::Condor, 11, 18),
    ]);
    let mut direct_state = scenario
        .build()
        .expect("the connected-operation trace scenario builds");
    let mut traced_state = direct_state.clone();
    let mut direct = foundry_competition_brain(&scenario);
    let mut traced = direct.clone();

    let direct_commands = direct.act(&direct_state);
    let traced_act = traced.act_traced(&traced_state);

    assert_eq!(traced_act.commands, direct_commands);
    let trace = traced_act
        .trace
        .expect("the connected-operation admission is traced");
    assert_eq!(
        trace.connected_force.status,
        crate::trace::ConnectedForceStatus::Active
    );
    let target = trace
        .connected_force
        .target
        .expect("the connected package records its target");
    assert_eq!(target.kind, BuildingKind::Foundry);
    assert_eq!(target.evidence, crate::trace::TargetEvidenceTrace::Current);
    let package = trace
        .connected_force
        .package
        .expect("the admitted connected package is recorded");
    assert_eq!(package.admitted_at, direct_state.current_tick());
    assert_eq!(package.derived_at, direct_state.current_tick());
    assert!(package.preparation_deadline > package.derived_at);
    assert!(package.target_anchors.contains(&target.anchor));
    assert!(
        package
            .target_anchors
            .windows(2)
            .all(|pair| (pair[0].y, pair[0].x) < (pair[1].y, pair[1].x))
    );
    assert!(!package.demands.recon.is_empty());
    assert!(!package.demands.suppression.is_empty());
    assert!(!package.demands.strike.is_empty());
    assert!(package.chosen_capability.recon >= package.minimum_capability.recon);
    assert!(package.chosen_capability.suppression >= package.minimum_capability.suppression);
    assert!(package.chosen_capability.strike >= package.minimum_capability.strike);
    assert_eq!(package.observed_aa_firepower, 0);
    assert_eq!(package.suppressible_aa_firepower, 0);
    assert!(trace.connected_force.assigned.scout.is_some());
    assert!(!trace.connected_force.assigned.membership_frozen);

    direct_state.tick(&direct_commands);
    traced_state.tick(&traced_act.commands);
    assert_eq!(traced_state.hash(), direct_state.hash());
    assert_brain_unchanged(&direct, &traced);
}

#[test]
fn terminal_connected_trace_uses_target_evidence_from_the_termination_tick() {
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("Foundries are constructible")
        .cost;
    let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
    let scout = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
        .expect("the trace scenario has one connected-air scout");
    (scout.x, scout.y) = (42, 19);
    for (index, kind) in [
        UnitKind::Bombard,
        UnitKind::Avalanche,
        Role::AirGround.unit_for(scenario.players[0].faction),
        Role::Bomber.unit_for(scenario.players[0].faction),
    ]
    .into_iter()
    .cycle()
    .take(24)
    .enumerate()
    {
        scenario.units.push(unit_spec(
            0,
            kind,
            7 + i32::try_from(index % 8).expect("small fixture index"),
            16 + i32::try_from(index / 8).expect("small fixture index"),
        ));
    }
    let mut state = scenario
        .build()
        .expect("the terminal-trace scenario builds");
    let mut brain = foundry_competition_brain(&scenario);

    let mut frozen = None;
    for _ in 0..1_000 {
        let decision = brain.act_traced(&state);
        if let Some(trace) = &decision.trace
            && trace.connected_force.assigned.membership_frozen
        {
            let target = trace
                .connected_force
                .target
                .expect("the frozen connected package retains its target");
            assert_eq!(target.evidence, crate::trace::TargetEvidenceTrace::Current);
            let package = trace
                .connected_force
                .package
                .as_ref()
                .expect("the frozen connected force retains its package");
            let assigned = &trace.connected_force.assigned;
            let mut members: Vec<_> = assigned
                .scout
                .into_iter()
                .chain(assigned.suppression.iter().copied())
                .chain(assigned.strike.iter().copied())
                .collect();
            members.sort_unstable();
            members.dedup();
            assert!(!members.is_empty());
            frozen = Some((target, package.target_anchors.clone(), members));
        }
        state.tick(&decision.commands);
        if frozen.is_some() {
            break;
        }
    }
    let (target, target_anchors, members) =
        frozen.expect("the connected package reaches exact-id freeze");

    let mut lost_force = Observation::fog_honest(&state, PlayerId(0));
    lost_force
        .my_units
        .retain(|unit| members.binary_search(&unit.id).is_err());
    lost_force.tick = lost_force.tick.next_multiple_of(brain.dials.cadence);
    for building in &mut lost_force.enemy_buildings {
        if building.player == target.player && building.anchor == target.anchor {
            building.seen = false;
        }
    }
    let (width, height) = target.kind.base_stats().size;
    for y in target.anchor.y..target.anchor.y + height {
        for x in target.anchor.x..target.anchor.x + width {
            let index = (y * lost_force.map_width + x) as usize;
            lost_force.visible[index] = false;
        }
    }
    let terminal = crate::Brain::act_traced(&mut brain, &lost_force);
    let trace = terminal.trace.expect("the terminal decision is traced");
    assert_eq!(
        trace.connected_force.status,
        crate::trace::ConnectedForceStatus::Aborted
    );
    let terminal_target = trace
        .connected_force
        .target
        .expect("the terminal trace preserves the package target");
    assert_eq!(
        (
            terminal_target.player,
            terminal_target.kind,
            terminal_target.anchor
        ),
        (target.player, target.kind, target.anchor)
    );
    assert_eq!(
        terminal_target.evidence,
        crate::trace::TargetEvidenceTrace::Remembered,
        "the lost scout makes the still-live objective a ghost on the termination tick"
    );
    assert_eq!(
        trace
            .connected_force
            .package
            .expect("the terminal trace preserves the frozen package")
            .target_anchors,
        target_anchors
    );
}

#[test]
fn traced_act_marks_recovery_and_omits_non_decisions() {
    let mut scenario = Scenario::skirmish();
    scenario
        .units
        .retain(|unit| unit.player != 0 || unit.kind.stats().harvest.is_none());
    let mut state = scenario.build().expect("the stranded skirmish builds");
    let mut scripted = scripted_brain(&scenario, PlayerId(0), BotConfig::default());

    let recovery = scripted.act_traced(&state);
    let trace = recovery
        .trace
        .expect("a player-facing recovery think is traced");
    assert_eq!(trace.control_flow, DecisionControlFlow::HarvesterRecovery);
    assert_eq!(
        trace.lowering.total_commands as usize,
        recovery.commands.len()
    );
    assert!(trace.budget.is_none());
    assert_eq!(trace.channels, crate::trace::ChannelTraces::default());

    state.tick(&recovery.commands);
    assert!(
        scripted.act_traced(&state).trace.is_none(),
        "a cadence skip is not a decision record"
    );
}
