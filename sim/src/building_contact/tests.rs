use super::*;

/// The pruned queries against a plain walk over every edge, around every
/// outline, from points inside, beside, and well clear of it.
#[test]
fn pruned_surface_queries_match_a_walk_over_every_edge() {
    let mut rng = chassis::rng::Pcg32::new(41, 7);
    let mut near = |anchor: i32, span: i32| {
        Fx::from_num(anchor - 3)
            + Fx::from_bits(
                i64::from(rng.next_u32() % ((u32::try_from(span).unwrap() + 6) << 10)) << 22,
            )
    };
    let (mut blocked, mut open) = (0, 0);
    for kind in BuildingKind::ALL {
        let size = kind.size();
        let surface = Surface {
            kind,
            anchor: TilePos::new(10, 20),
            size,
        };
        let center = Vec2Fx::new(Fx::from_num(10), Fx::from_num(20))
            + Vec2Fx::new(Fx::from_num(size.0), Fx::from_num(size.1)) / Fx::from_num(2);
        for _ in 0..400 {
            let from = Vec2Fx::new(near(10, size.0), near(20, size.1));
            let to =
                from + (Vec2Fx::new(near(10, size.0), near(20, size.1)) - from) / Fx::from_num(4);
            let radius = near(0, 0) / 8 + Fx::lit("0.2");
            let closest = surface
                .edges()
                .map(|(a, b)| closest_on_segment(from, a, b))
                .min_by_key(|p| (from.dist_sq(*p), cross(from - center, *p - center)))
                .expect("nonempty outline");
            assert_eq!(surface.closest(from), closest, "{kind:?} {from:?}");
            let clear = !surface.contains(from)
                && !surface.contains(to)
                && surface.edges().all(|(a, b)| {
                    !segments_cross(from, to, a, b)
                        && [
                            from.dist_sq(closest_on_segment(from, a, b)),
                            to.dist_sq(closest_on_segment(to, a, b)),
                            a.dist_sq(closest_on_segment(a, from, to)),
                            b.dist_sq(closest_on_segment(b, from, to)),
                        ]
                        .into_iter()
                        .all(|d| d >= radius * radius)
                });
            assert_eq!(
                surface.clear(from, to, radius),
                clear,
                "{kind:?} {from:?} {to:?}"
            );
            blocked += usize::from(!clear);
            open += usize::from(clear);
        }
    }
    assert!(
        blocked > 200 && open > 200,
        "{blocked} blocked, {open} open"
    );
}
