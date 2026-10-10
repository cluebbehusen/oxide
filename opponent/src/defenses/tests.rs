use super::*;

#[test]
fn a_gun_holds_off_what_its_firepower_and_health_match_not_its_price() {
    let [spread, clumped] = SPLASH_TARGETS;
    let gun = |kind: BuildingKind, tier: u8, targets: u64| strength(kind.tier_stats(tier), targets);
    let turret = gun(BuildingKind::Turret, 0, spread);
    assert_eq!(
        turret,
        gun(BuildingKind::Turret, 0, clumped),
        "one hit a shot"
    );
    assert!(turret < gun(BuildingKind::Turret, 1, spread));
    assert!(gun(BuildingKind::Turret, 1, spread) < gun(BuildingKind::Turret, 2, spread));
    // Per scrap, a lone Bastion holds off fewer spread Sentinels than a
    // Turret, and a clump lifts it.
    let per_scrap = |strength: u64, kind: BuildingKind| strength * 1_000 / price(kind.base_stats());
    let turret = per_scrap(turret, BuildingKind::Turret);
    let bastion = |targets| {
        per_scrap(
            gun(BuildingKind::Bastion, 0, targets),
            BuildingKind::Bastion,
        )
    };
    assert!(
        bastion(spread) < turret,
        "{} against {turret}",
        bastion(spread)
    );
    assert!(bastion(clumped) > bastion(spread) * 3 / 2);
}

#[test]
fn a_gun_answers_only_enemies_it_can_fire_back_at() {
    let gun = |kind: BuildingKind| Cover::of(kind, 0, TilePos::new(10, 10)).unwrap();
    let answers = |kind: BuildingKind, enemy: UnitKind| gun(kind).answers(reach(enemy));
    assert!(answers(BuildingKind::Turret, UnitKind::Sentinel));
    assert!(
        !answers(BuildingKind::Turret, UnitKind::Lancer),
        "outranged"
    );
    assert!(!answers(BuildingKind::Turret, UnitKind::Bombard));
    assert!(answers(BuildingKind::Bastion, UnitKind::Sentinel));
    assert!(answers(BuildingKind::Bastion, UnitKind::Lancer));
    assert!(
        answers(BuildingKind::Bastion, UnitKind::Bombard),
        "even at a corner"
    );
    assert!(
        !answers(BuildingKind::Bastion, UnitKind::Scuttler),
        "a Scuttler bites from inside the minimum range"
    );
}

#[test]
fn an_upgrade_adds_its_whole_strength_where_the_gun_did_not_reach() {
    let anchor = TilePos::new(10, 10);
    let turret = Cover::of(BuildingKind::Turret, 0, anchor).unwrap();
    let heavy = Cover::of(BuildingKind::Turret, 1, anchor).unwrap();
    let centre = footprint_centre(BuildingKind::Turret, anchor);
    // Two, five and a half and eight tiles out, in doubled coordinates.
    let near = (centre.0 + 4, centre.1);
    let edge = (centre.0 + 11, centre.1);
    let far = (centre.0 + 16, centre.1);
    assert!(turret.covers(Domain::Ground, near) && !turret.covers(Domain::Ground, edge));
    assert!(heavy.covers(Domain::Ground, edge) && !heavy.covers(Domain::Ground, far));
    let raised = |point| raises(turret, heavy, Domain::Ground, point, |cover| cover.value[0]);
    assert_eq!(raised(near), heavy.value[0] - turret.value[0]);
    assert_eq!(raised(edge), heavy.value[0]);
    assert_eq!(raised(far), 0);
}

#[test]
fn a_repair_bay_reaches_by_straight_distance_from_its_edges() {
    let bay = Span::of((2, 2), TilePos::new(10, 10));
    let unit = |x: i32, y: i32| {
        let (x, y) = doubled(TilePos::new(x, y));
        Span {
            x: (x, x),
            y: (y, y),
        }
    };
    assert!(
        bay.aura(unit(15, 10)),
        "three and a half tiles straight out"
    );
    assert!(!bay.aura(unit(16, 10)), "four and a half tiles out");
    assert!(
        !bay.aura(unit(15, 15)),
        "three tiles clear on both axes is over four tiles away"
    );
    let building = Span::of((2, 2), TilePos::new(16, 10));
    assert!(bay.aura(building), "edges four tiles apart");
    assert!(!bay.aura(Span::of((2, 2), TilePos::new(15, 15))));
}

#[test]
fn a_defense_faces_the_domain_its_guns_can_hit() {
    assert_eq!(domain(BuildingKind::FlakTurret), Domain::Air);
    for kind in [
        BuildingKind::Turret,
        BuildingKind::Bastion,
        BuildingKind::Array,
    ] {
        assert_eq!(domain(kind), Domain::Ground, "{kind:?}");
    }
}
