use chassis::grid::TilePos;

/// A positive rectangular footprint with a top-left anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SiteFootprint {
    /// Top-left tile.
    anchor: TilePos,
    /// Width and height in tiles.
    size: (i32, i32),
}

impl SiteFootprint {
    /// Creates a footprint, rejecting zero or negative dimensions.
    pub(crate) const fn new(anchor: TilePos, size: (i32, i32)) -> Option<Self> {
        if size.0 > 0 && size.1 > 0 {
            Some(Self { anchor, size })
        } else {
            None
        }
    }

    /// Top-left tile retained by this exact construction claim.
    pub(crate) const fn anchor(self) -> TilePos {
        self.anchor
    }

    /// Width and height retained by this exact construction claim.
    pub(crate) const fn size(self) -> (i32, i32) {
        self.size
    }

    pub(crate) fn overlaps(self, other: Self) -> bool {
        let self_left = i64::from(self.anchor.x);
        let self_top = i64::from(self.anchor.y);
        let self_right = self_left + i64::from(self.size.0);
        let self_bottom = self_top + i64::from(self.size.1);
        let other_left = i64::from(other.anchor.x);
        let other_top = i64::from(other.anchor.y);
        let other_right = other_left + i64::from(other.size.0);
        let other_bottom = other_top + i64::from(other.size.1);
        self_left < other_right
            && other_left < self_right
            && self_top < other_bottom
            && other_top < self_bottom
    }

    fn row_major_key(self) -> (i32, i32, i32, i32) {
        (self.anchor.y, self.anchor.x, self.size.1, self.size.0)
    }
}

impl Ord for SiteFootprint {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.row_major_key().cmp(&other.row_major_key())
    }
}

impl PartialOrd for SiteFootprint {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
