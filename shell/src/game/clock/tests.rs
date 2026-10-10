use super::*;

#[test]
fn a_paused_picture_shows_the_last_tick_whole() {
    let mut clock = Clock {
        accum: TICK_DT * 0.25,
        ..Clock::default()
    };
    assert!((clock.render_alpha() - 0.25).abs() < 1e-6);
    clock.paused = true;
    assert!((clock.render_alpha() - 1.0).abs() < f32::EPSILON);
    assert!(
        (clock.tick_fraction() - 0.25).abs() < 1e-6,
        "the debt waits"
    );
}

#[test]
fn owed_ticks_follow_speed_and_stop_at_a_frames_cap() {
    let mut clock = Clock {
        speed: 2.0,
        ..Clock::default()
    };
    assert_eq!(clock.due_ticks(TICK_DT * 1.5), 3);
    assert!(clock.accum < TICK_DT, "only the fraction stays owed");
    let mut hitch = Clock::default();
    assert_eq!(
        hitch.due_ticks(TICK_DT * 100.0),
        u64::from(MAX_TICKS_PER_FRAME)
    );
    assert!(hitch.accum < TICK_DT, "debt past the cap is dropped");
}
