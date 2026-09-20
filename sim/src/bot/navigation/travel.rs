//! Free-flow travel time over a scalar route cost.

use crate::UnitKind;

/// Ticks to cover `route_cost` at the kind's full speed. Route costs use ten
/// for an axial tile and fourteen for a diagonal tile.
///
/// The ceiling is taken once over exact integers. Dividing in fixed point
/// truncates twice and can land a tick short.
pub(in crate::bot) fn travel_ticks(kind: UnitKind, route_cost: u32) -> u64 {
    let speed = kind.stats().speed.to_bits();
    if speed <= 0 {
        return u64::MAX;
    }
    let distance = u128::from(route_cost) << 32;
    let ticks = distance.div_ceil((speed as u128).saturating_mul(10));
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travel_is_the_exact_ceiling_of_distance_over_speed() {
        for kind in UnitKind::ALL {
            let step = u128::try_from(kind.stats().speed.to_bits()).unwrap() * 10;
            for cost in [0, 10, 14, 24, 50, 134, 917_490, u32::MAX] {
                let ticks = u128::from(travel_ticks(kind, cost));
                let distance = u128::from(cost) << 32;
                assert!(ticks * step >= distance, "{kind:?} {cost}");
                assert!(
                    ticks == 0 || (ticks - 1) * step < distance,
                    "{kind:?} {cost}"
                );
            }
        }
        assert_eq!(travel_ticks(UnitKind::Harvester, 10), 8);
    }
}
