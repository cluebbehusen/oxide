use super::*;
use macroquad::prelude::vec2;

fn drive(screen: &mut CodexScreen, key: Key) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    screen.update(
        &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        &mut mouse,
        &mut sounds,
    )
}

#[test]
fn every_kind_has_exactly_one_page() {
    let screen = CodexScreen::open();
    let mut units = 0;
    let mut buildings = 0;
    for entry in screen.entries.iter().flatten() {
        match entry {
            Entry::Unit(_) => units += 1,
            Entry::Building(_) => buildings += 1,
        }
    }
    assert_eq!(units, oxide_sim::stats::UnitKind::ALL.len());
    assert_eq!(buildings, oxide_sim::stats::BuildingKind::ALL.len());
    // No machine is listed under two factories.
    let mut seen = std::collections::BTreeSet::new();
    for entry in screen.entries.iter().flatten() {
        assert!(seen.insert(format!("{entry:?}")), "{entry:?} listed twice");
    }
}

#[test]
fn opens_on_a_page_and_escape_leaves() {
    let mut screen = CodexScreen::open();
    assert_eq!(screen.mode_name(), "codex");
    assert_eq!(
        screen.selected_entry(),
        Some(Entry::Unit(UnitKind::Harvester)),
        "the first real row is the first Foundry machine"
    );
    assert_eq!(drive(&mut screen, Key::Down), Out::Stay);
    assert_eq!(
        screen.selected_entry(),
        Some(Entry::Unit(UnitKind::Sentinel))
    );
    assert_eq!(drive(&mut screen, Key::Escape), Out::Leave);
}

#[test]
fn the_back_button_leaves_and_no_row_does() {
    let mut screen = CodexScreen::open();
    assert!(!screen.menu.items.iter().any(|item| item == "Back"));
    let last = screen.menu.items.len() - 1;
    screen.menu.select(last);
    assert!(screen.selected_entry().is_some(), "the last row is a page");
    assert_eq!(drive(&mut screen, Key::Enter), Out::Stay);
    for touch in [false, true] {
        let mut mouse = vec2(0.0, 0.0);
        let out = screen.update(
            &crate::button::press_back(touch),
            &mut mouse,
            &mut Vec::new(),
        );
        assert_eq!(out, Out::Leave);
    }
}

#[test]
fn activating_a_page_row_stays() {
    let mut screen = CodexScreen::open();
    assert_eq!(drive(&mut screen, Key::Enter), Out::Stay);
}

#[test]
fn notes_read_from_the_stats_table() {
    let harvester = unit_notes(UnitKind::Harvester);
    assert!(harvester.iter().any(|l| l.starts_with("Hauls 10 scrap")));
    assert!(
        harvester
            .iter()
            .any(|l| l.contains("Trained at the Foundry"))
    );
    let skyhook = unit_notes(UnitKind::Skyhook);
    assert!(skyhook.iter().any(|l| l.starts_with("Lifts 4 sling")));
    let turret = building_notes(BuildingKind::Turret);
    assert_eq!(turret.len(), 2, "two upgrade rungs: {turret:?}");
    assert!(turret[0].contains("Heavy Turret"));
    assert!(turret[1].contains("Bulwark") && turret[1].contains("Crucible"));

    let extractor = building_notes(BuildingKind::Extractor);
    assert!(
        extractor
            .iter()
            .any(|line| line.contains("120 scrap/min remote"))
    );
    assert!(
        extractor
            .iter()
            .any(|line| line.contains("180 scrap/min with non-stacking support"))
    );
    assert!(
        extractor
            .iter()
            .any(|line| line.contains("8 footprint tiles"))
    );

    let foundry = building_notes(BuildingKind::Foundry);
    assert!(foundry.iter().any(|line| {
        line.contains("Supports own completed Extractors")
            && line.contains("120 to 180 scrap/min")
            && line.contains("do not stack")
    }));
}
