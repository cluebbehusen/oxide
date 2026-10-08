use super::*;

#[test]
fn coordinate_parsers_trim_input_and_reject_invalid_shapes() {
    assert_eq!(
        parse_tile(" -3, 17 ").unwrap(),
        chassis::grid::TilePos::new(-3, 17)
    );
    assert_eq!(parse_point(" 1.25, -2.5 ").unwrap(), (1.25, -2.5));
    assert!(parse_tile("3").is_err());
    assert!(parse_tile("3,4,5").is_err());
    assert!(
        parse_point("NaN,1")
            .unwrap_err()
            .to_string()
            .contains("finite")
    );
    assert!(
        parse_point("1,inf")
            .unwrap_err()
            .to_string()
            .contains("finite")
    );
}

#[test]
fn pointer_and_key_aliases_are_case_insensitive_but_fail_closed() {
    assert_eq!(parse_mouse_button("LEFT").unwrap(), MouseButton::Left);
    assert_eq!(parse_mouse_button("right").unwrap(), MouseButton::Right);
    assert_eq!(parse_mouse_button("Middle").unwrap(), MouseButton::Middle);
    assert!(parse_mouse_button("primary").is_err());

    assert_eq!(parse_key("RETURN").unwrap(), Key::Enter);
    assert_eq!(parse_key("esc").unwrap(), Key::Escape);
    assert_eq!(parse_key("7").unwrap(), Key::Num7);
    assert!(parse_key("delete").is_err());
}

#[test]
fn kinds_parse_by_sim_name_across_separator_styles() {
    for kind in UnitKind::ALL {
        assert_eq!(parse_unit_kind(kind.name()).unwrap(), kind);
    }
    for kind in BuildingKind::ALL {
        assert_eq!(parse_building_kind(kind.name()).unwrap(), kind);
    }
    for spelling in ["flak-turret", "flak_turret", "Flak Turret", "FlakTurret"] {
        assert_eq!(
            parse_building_kind(spelling).unwrap(),
            BuildingKind::FlakTurret
        );
    }
    let error = parse_unit_kind("sentinal").unwrap_err().to_string();
    assert!(error.contains("sentinel"), "{error}");
}
