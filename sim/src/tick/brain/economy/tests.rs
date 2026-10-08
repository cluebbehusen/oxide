use crate::state::PROGRESS_ENVELOPE;

#[test]
fn the_live_meter_saturates_below_the_progress_envelope() {
    // The step math's overflow guard: whatever a torch has lived
    // through, the meter it bills from stays under the ceiling the
    // ramp products are proven to fit (state.rs pins the products).
    assert_eq!(super::metered(0), 0);
    assert_eq!(super::metered(PROGRESS_ENVELOPE - 1), PROGRESS_ENVELOPE - 1);
    assert_eq!(super::metered(PROGRESS_ENVELOPE), PROGRESS_ENVELOPE - 1);
    assert_eq!(super::metered(u32::MAX), PROGRESS_ENVELOPE - 1);
}
