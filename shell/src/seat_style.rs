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

#[derive(Clone, Copy)]
pub(crate) struct SeatStyle {
    pub cue: AllegianceCue,
    pub color: Color,
}

#[derive(Clone, Copy)]
pub(crate) struct SeatStyles([SeatStyle; MAX_PLAYERS]);

impl SeatStyles {
    pub fn new(state: &State, viewer: PlayerId, colorblind: bool) -> Self {
        let mut allies = 0;
        let mut hostiles = 0;
        let mut styles = [SeatStyle {
            cue: AllegianceCue::Mine,
            color: WHITE,
        }; MAX_PLAYERS];
        for (seat, style) in styles.iter_mut().enumerate().take(state.players().len()) {
            let owner = PlayerId::from_index(seat);
            let cue = AllegianceCue::of(state, viewer, owner);
            let color = match cue {
                AllegianceCue::Mine => roster_accent(colorblind),
                AllegianceCue::Ally => {
                    let color = identity_color(cue, allies, colorblind);
                    allies += 1;
                    color
                }
                AllegianceCue::Hostile => {
                    let color = identity_color(cue, hostiles, colorblind);
                    hostiles += 1;
                    color
                }
            };
            *style = SeatStyle { cue, color };
        }
        Self(styles)
    }

    pub fn get(&self, owner: PlayerId) -> SeatStyle {
        self.0[usize::from(owner.0)]
    }
}

/// The accent the roster's art wears, which own seats keep.
pub(crate) fn roster_accent(colorblind: bool) -> Color {
    if colorblind {
        color_u8!(230, 120, 30, 255)
    } else {
        color_u8!(196, 87, 59, 255)
    }
}

/// Stable hues by rank within the ally or the hostile family.
fn identity_color(cue: AllegianceCue, rank: usize, colorblind: bool) -> Color {
    let allies = if colorblind {
        [
            color_u8!(238, 234, 222, 255),
            color_u8!(112, 184, 238, 255),
            color_u8!(181, 158, 232, 255),
            color_u8!(145, 207, 190, 255),
            color_u8!(107, 148, 224, 255),
            color_u8!(137, 207, 229, 255),
            color_u8!(200, 181, 239, 255),
            color_u8!(111, 188, 174, 255),
        ]
    } else {
        [
            color_u8!(100, 160, 245, 255),
            color_u8!(68, 190, 205, 255),
            color_u8!(165, 139, 235, 255),
            color_u8!(132, 201, 170, 255),
            color_u8!(64, 119, 221, 255),
            color_u8!(113, 203, 239, 255),
            color_u8!(196, 162, 242, 255),
            color_u8!(72, 174, 157, 255),
        ]
    };
    let hostiles = if colorblind {
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
    };
    let palette = match cue {
        AllegianceCue::Ally => allies,
        AllegianceCue::Hostile => hostiles,
        AllegianceCue::Mine => unreachable!("own seats use the roster accent"),
    };
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
