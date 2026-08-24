//! Argument value-parsers for the live-client CLI: tiles, points,
//! buttons, keys, and the kind-name enums clap surfaces as choices.

use anyhow::{Context, Result, bail};
use oxide_protocol::{Key, MouseButton};
use oxide_sim::UnitKind;

pub(crate) fn parse_tile(s: &str) -> Result<chassis::grid::TilePos> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    Ok(chassis::grid::TilePos::new(
        x.trim().parse()?,
        y.trim().parse()?,
    ))
}

pub(crate) fn parse_point(s: &str) -> Result<(f32, f32)> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    let point = (x.trim().parse::<f32>()?, y.trim().parse::<f32>()?);
    if point.0.is_finite() && point.1.is_finite() {
        Ok(point)
    } else {
        bail!("point coordinates must be finite")
    }
}

pub(crate) fn parse_mouse_button(s: &str) -> Result<MouseButton> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => bail!("unknown button {other:?}"),
    })
}

/// Clap-native unit kinds — typos die in argument parsing with the full
/// list of choices, before anything touches the socket.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum UnitKindArg {
    Harvester,
    Sentinel,
    Scuttler,
    Lancer,
    Bombard,
    Flakhound,
    Stinger,
    Buzzard,
    Darter,
    Talon,
    Wisp,
    Warden,
    Tender,
    Excavator,
    Kestrel,
    Gnat,
    Shrike,
    Sylph,
    Condor,
    Moth,
    Breaker,
    Avalanche,
    Skyhook,
    Sapper,
}

impl From<UnitKindArg> for UnitKind {
    fn from(k: UnitKindArg) -> Self {
        match k {
            UnitKindArg::Harvester => UnitKind::Harvester,
            UnitKindArg::Sentinel => UnitKind::Sentinel,
            UnitKindArg::Scuttler => UnitKind::Scuttler,
            UnitKindArg::Lancer => UnitKind::Lancer,
            UnitKindArg::Bombard => UnitKind::Bombard,
            UnitKindArg::Flakhound => UnitKind::Flakhound,
            UnitKindArg::Stinger => UnitKind::Stinger,
            UnitKindArg::Buzzard => UnitKind::Buzzard,
            UnitKindArg::Darter => UnitKind::Darter,
            UnitKindArg::Talon => UnitKind::Talon,
            UnitKindArg::Wisp => UnitKind::Wisp,
            UnitKindArg::Warden => UnitKind::Warden,
            UnitKindArg::Tender => UnitKind::Tender,
            UnitKindArg::Excavator => UnitKind::Excavator,
            UnitKindArg::Kestrel => UnitKind::Kestrel,
            UnitKindArg::Gnat => UnitKind::Gnat,
            UnitKindArg::Shrike => UnitKind::Shrike,
            UnitKindArg::Sylph => UnitKind::Sylph,
            UnitKindArg::Condor => UnitKind::Condor,
            UnitKindArg::Moth => UnitKind::Moth,
            UnitKindArg::Breaker => UnitKind::Breaker,
            UnitKindArg::Avalanche => UnitKind::Avalanche,
            UnitKindArg::Skyhook => UnitKind::Skyhook,
            UnitKindArg::Sapper => UnitKind::Sapper,
        }
    }
}

/// Every player-buildable kind. The simulation's placement rules remain
/// the authority on where each may stand.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum BuildingKindArg {
    Turret,
    Fabricator,
    FlakTurret,
    Bastion,
    Array,
    Reclaimer,
    RepairBay,
    Airworks,
    Crucible,
    Foundry,
    Extractor,
    Barricade,
    ScuttleCharge,
}

impl From<BuildingKindArg> for oxide_sim::BuildingKind {
    fn from(k: BuildingKindArg) -> Self {
        match k {
            BuildingKindArg::Turret => oxide_sim::BuildingKind::Turret,
            BuildingKindArg::Fabricator => oxide_sim::BuildingKind::Fabricator,
            BuildingKindArg::FlakTurret => oxide_sim::BuildingKind::FlakTurret,
            BuildingKindArg::Bastion => oxide_sim::BuildingKind::Bastion,
            BuildingKindArg::Array => oxide_sim::BuildingKind::Array,
            BuildingKindArg::Reclaimer => oxide_sim::BuildingKind::Reclaimer,
            BuildingKindArg::RepairBay => oxide_sim::BuildingKind::RepairBay,
            BuildingKindArg::Airworks => oxide_sim::BuildingKind::Airworks,
            BuildingKindArg::Crucible => oxide_sim::BuildingKind::Crucible,
            BuildingKindArg::Foundry => oxide_sim::BuildingKind::Foundry,
            BuildingKindArg::Extractor => oxide_sim::BuildingKind::Extractor,
            BuildingKindArg::Barricade => oxide_sim::BuildingKind::Barricade,
            BuildingKindArg::ScuttleCharge => oxide_sim::BuildingKind::ScuttleCharge,
        }
    }
}

pub(crate) fn parse_key(s: &str) -> Result<Key> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "h" => Key::H,
        "s" => Key::S,
        "a" => Key::A,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "o" => Key::O,
        "q" => Key::Q,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "y" => Key::Y,
        "z" => Key::Z,
        "p" => Key::P,
        "r" => Key::R,
        "b" => Key::B,
        "n" => Key::N,
        "x" => Key::X,
        "enter" | "return" => Key::Enter,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "space" => Key::Space,
        "f1" => Key::F1,
        "shift" => Key::Shift,
        "ctrl" => Key::Ctrl,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        other => bail!("unknown key {other:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum as _;

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
    fn cli_kind_catalogs_cover_every_sim_kind_exactly_once() {
        let mut cli_units: Vec<_> = UnitKindArg::value_variants()
            .iter()
            .copied()
            .map(UnitKind::from)
            .collect();
        let mut sim_units = UnitKind::ALL.to_vec();
        cli_units.sort_unstable();
        sim_units.sort_unstable();
        assert_eq!(cli_units, sim_units);

        let mut cli_buildings: Vec<_> = BuildingKindArg::value_variants()
            .iter()
            .copied()
            .map(oxide_sim::BuildingKind::from)
            .collect();
        let mut sim_buildings = oxide_sim::BuildingKind::ALL.to_vec();
        cli_buildings.sort_unstable();
        sim_buildings.sort_unstable();
        assert_eq!(cli_buildings, sim_buildings);
    }
}
