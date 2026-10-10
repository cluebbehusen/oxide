//! Viewer-relative presentation identity, prepared once for a live or replay view.
use macroquad::prelude::{Color, WHITE, color_u8};
use oxide_sim::scenario::MAX_PLAYERS;
use oxide_sim::{PlayerId, State};

/// Relationship to the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllegianceCue {
    Mine,
    Ally,
    Hostile,
}

impl AllegianceCue {
    /// How `viewer` sees `owner`.
    pub fn of(state: &State, viewer: PlayerId, owner: PlayerId) -> Self {
        if owner == viewer {
            Self::Mine
        } else if state.hostile(viewer, owner) {
            Self::Hostile
        } else {
            Self::Ally
        }
    }
}

/// Every seat's ownership color as one viewer sees it.
#[derive(Clone, Copy)]
pub(crate) struct SeatStyles([Color; MAX_PLAYERS]);

impl SeatStyles {
    pub fn new(state: &State, viewer: PlayerId, colorblind: bool) -> Self {
        Self::from_cues(
            (0..state.players().len())
                .map(|seat| AllegianceCue::of(state, viewer, PlayerId::from_index(seat))),
            colorblind,
        )
    }

    /// Styles for seats in seat order whose relationship to the viewer is
    /// already known. Allies and hostiles each rank by seat order within
    /// their own family.
    pub fn from_cues(cues: impl IntoIterator<Item = AllegianceCue>, colorblind: bool) -> Self {
        let mut allies = 0;
        let mut hostiles = 0;
        let mut styles = [WHITE; MAX_PLAYERS];
        for (style, cue) in styles.iter_mut().zip(cues) {
            *style = match cue {
                AllegianceCue::Mine => self_color(colorblind),
                AllegianceCue::Ally => {
                    allies += 1;
                    family_color(&ally_palette(colorblind), allies - 1)
                }
                AllegianceCue::Hostile => {
                    hostiles += 1;
                    family_color(&hostile_palette(colorblind), hostiles - 1)
                }
            };
        }
        Self(styles)
    }

    pub fn get(&self, owner: PlayerId) -> Color {
        self.0[usize::from(owner.0)]
    }
}

/// Your own seat's color: a hue neither the ally nor the hostile family
/// uses, so "mine" never reads as a friend or a foe.
pub(crate) fn self_color(colorblind: bool) -> Color {
    // Near-white holds apart from both families under deutan, protan,
    // and tritan vision, where every green collapses toward a warm hue.
    if colorblind {
        color_u8!(245, 245, 240, 255)
    } else {
        color_u8!(40, 190, 110, 255)
    }
}

/// Cool blues and violets, clear of the self green.
fn ally_palette(colorblind: bool) -> [Color; 8] {
    if colorblind {
        [
            color_u8!(120, 190, 255, 255),
            color_u8!(150, 160, 255, 255),
            color_u8!(100, 215, 255, 255),
            color_u8!(185, 165, 255, 255),
            color_u8!(90, 160, 245, 255),
            color_u8!(170, 205, 255, 255),
            color_u8!(130, 135, 240, 255),
            color_u8!(150, 225, 255, 255),
        ]
    } else {
        [
            color_u8!(100, 160, 245, 255),
            color_u8!(165, 139, 235, 255),
            color_u8!(113, 203, 239, 255),
            color_u8!(64, 119, 221, 255),
            color_u8!(196, 162, 242, 255),
            color_u8!(140, 150, 250, 255),
            color_u8!(120, 96, 214, 255),
            color_u8!(160, 200, 250, 255),
        ]
    }
}

/// Warm reds, oranges, and magentas.
fn hostile_palette(colorblind: bool) -> [Color; 8] {
    if colorblind {
        [
            color_u8!(211, 65, 60, 255),
            color_u8!(232, 128, 35, 255),
            color_u8!(173, 72, 125, 255),
            color_u8!(151, 87, 61, 255),
            color_u8!(242, 84, 31, 255),
            color_u8!(207, 99, 104, 255),
            color_u8!(190, 136, 43, 255),
            color_u8!(139, 49, 82, 255),
        ]
    } else {
        [
            color_u8!(228, 44, 58, 255),
            color_u8!(232, 105, 42, 255),
            color_u8!(199, 66, 132, 255),
            color_u8!(172, 83, 57, 255),
            color_u8!(246, 73, 24, 255),
            color_u8!(210, 85, 111, 255),
            color_u8!(196, 124, 37, 255),
            color_u8!(154, 50, 85, 255),
        ]
    }
}

/// The `rank`th seat's color within a family; a family larger than its
/// palette reuses it lightened.
fn family_color(palette: &[Color; 8], rank: usize) -> Color {
    let base = palette[rank % palette.len()];
    if rank < palette.len() {
        base
    } else {
        Color::new(
            base.r * 0.7 + 0.3,
            base.g * 0.7 + 0.3,
            base.b * 0.7 + 0.3,
            1.0,
        )
    }
}

#[cfg(test)]
mod tests;
