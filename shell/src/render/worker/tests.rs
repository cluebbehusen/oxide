use super::*;

#[test]
fn overlapping_wrecks_never_pull_the_tool_behind_its_mount() {
    for bearing in [0., 0.7, 1.5, 3., 4.5] {
        let forward = Vec2::from_angle(bearing);
        let root = vec2(120., 80.);
        for offset in [Vec2::ZERO, -forward * 40., vec2(100., -100.)] {
            let tip = forward_work_point(root + offset, root, forward, 1.);
            assert!((tip - root).dot(forward) >= 19.99);
            assert!(tip.distance(root) < 34.);
        }
    }
}

#[test]
fn stowed_welder_nests_against_the_body_without_entering_the_treads() {
    for target in [vec2(64., 0.), vec2(110., 10.), vec2(45., 35.)] {
        let joints = arm_joints(target, 0.);
        assert_eq!(joints[0], vec2(86., 66.));
        for point in joints {
            assert!((83. ..91.).contains(&point.x));
            assert!((59. ..102.).contains(&point.y));
        }
    }
}

#[test]
fn welder_links_keep_their_length_while_folding_to_the_side() {
    let target = vec2(64., 19.);
    for step in 0..=20 {
        let [mount, elbow, tip] = arm_joints(target, step as f32 / 20.);
        assert!((elbow.distance(mount) - 34.).abs() < 0.001);
        assert!((tip.distance(elbow) - 38.).abs() < 0.001);
    }
    let [mount, elbow, tip] = arm_joints(target, 1.);
    assert_eq!(mount, vec2(86., 66.));
    assert!(elbow.x > 90., "elbow should clear the side of the roller");
    assert!(tip.distance(target) < 0.001);
}
