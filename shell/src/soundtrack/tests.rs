use super::*;

fn settle(score: &mut Soundtrack, scene: Scene, volumes: Volumes) -> Mix {
    let mut mix = Mix::default();
    for _ in 0..100 {
        mix = score.update(scene, false, 0.1, volumes);
    }
    mix
}

#[test]
fn menu_and_match_crossfade_without_a_restart_or_jump() {
    let volumes = Volumes::default();
    let mut score = Soundtrack::default();
    let menu = settle(&mut score, Scene::Menu, volumes);
    assert_eq!(menu.menu, 0.16);
    assert_eq!(menu.calm, 0.0);

    let first_match = score.update(Scene::Match, false, 0.1, volumes);
    assert!(
        first_match.menu > 0.0 && first_match.menu < menu.menu,
        "the old bed fades instead of stopping"
    );
    assert!(
        first_match.calm > 0.0 && first_match.calm < 0.16,
        "the new bed fades in instead of restarting loudly"
    );
    let calm = settle(&mut score, Scene::Match, volumes);
    assert_eq!(calm.menu, 0.0);
    assert_eq!(calm.calm, 0.16);
}

#[test]
fn a_combat_impulse_layers_pressure_then_returns_to_calm() {
    let volumes = Volumes::default();
    let mut score = Soundtrack::default();
    settle(&mut score, Scene::Match, volumes);
    score.update(Scene::Match, true, 0.1, volumes);
    let mut battle = Mix::default();
    for _ in 0..10 {
        battle = score.update(Scene::Match, false, 0.1, volumes);
    }
    assert!(battle.combat > 0.1, "one volley has a useful musical tail");
    assert!(
        battle.calm < 0.16,
        "the pressure layer makes room for itself"
    );

    let calm = settle(&mut score, Scene::Match, volumes);
    assert_eq!(calm.combat, 0.0);
    assert_eq!(calm.calm, 0.16);
}

#[test]
fn pause_ducks_the_match_bed_without_silencing_it() {
    let volumes = Volumes::default();
    let mut score = Soundtrack::default();
    settle(&mut score, Scene::Match, volumes);
    let paused = settle(&mut score, Scene::Pause, volumes);
    assert_eq!(paused.calm, 0.05);
    assert_eq!(paused.combat, 0.0);
}

#[test]
fn neutral_victory_and_defeat_results_have_distinct_beds() {
    for (scene, field) in [(Scene::Result, 3), (Scene::Victory, 4), (Scene::Defeat, 5)] {
        let mut score = Soundtrack::default();
        let mix = settle(&mut score, scene, Volumes::default());
        let values = [
            mix.menu,
            mix.calm,
            mix.combat,
            mix.result,
            mix.victory,
            mix.defeat,
        ];
        assert!(values[field] > 0.14);
        assert!(
            values
                .iter()
                .enumerate()
                .all(|(index, value)| index == field || *value == 0.0),
            "{scene:?} owns exactly one result bed"
        );
    }
}

#[test]
fn mute_and_volume_edits_fade_and_scale_the_music_bus() {
    let mut score = Soundtrack::default();
    let full = settle(&mut score, Scene::Menu, Volumes::default());
    let muted = Volumes {
        music: 0.0,
        ..Volumes::default()
    };
    let first_muted = score.update(Scene::Menu, false, 0.1, muted);
    assert!(
        first_muted.menu > 0.0 && first_muted.menu < full.menu,
        "mute is a short fade, not a click"
    );
    assert_eq!(settle(&mut score, Scene::Menu, muted), Mix::default());

    let half = Volumes {
        master: 0.5,
        music: 0.5,
        ..Volumes::default()
    };
    let mix = settle(&mut score, Scene::Menu, half);
    assert!((mix.menu - 0.04).abs() < 1.0e-6);
}
