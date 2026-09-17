//! Building-placement rules and fog-honest placement queries.

use super::{Order, PlaceRefusal, State};
use crate::ids::{PlayerId, UnitId};
use crate::stats::BuildingKind;
use chassis::grid::TilePos;

impl State {
    /// Whether a currently visible site can be claimed from the issuer's
    /// knowledge. Concealed enemy charges do not refuse a claim; detection
    /// cancels unstarted work, and construction trips an undiscovered charge.
    pub fn can_place(&self, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> bool {
        self.place_refusal(player, kind, anchor).is_none()
    }

    /// The first known placement blocker, requiring current footprint sight.
    pub fn place_refusal(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<PlaceRefusal> {
        if !self.prerequisites_met(player, kind) {
            return Some(PlaceRefusal::Prerequisite);
        }
        self.place_refusal_except(player, kind, anchor, None)
    }

    pub(crate) fn place_refusal_except(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
        ignored: Option<crate::BuildingId>,
    ) -> Option<PlaceRefusal> {
        if kind.base_stats().construction.is_none() {
            return Some(PlaceRefusal::NotConstructible);
        }
        let (w, h) = kind.base_stats().size;
        // Sight answers before the authored-frame rules. Otherwise the
        // distinct FrameRequired / FrameBlocked reasons turn placement
        // into a probe for derelict frames hidden in unexplored ground.
        for dy in 0..h {
            for dx in 0..w {
                if !self.vision(player).visible(anchor.offset(dx, dy)) {
                    return Some(PlaceRefusal::Fog);
                }
            }
        }
        if kind == BuildingKind::Extractor {
            // The machine exists only where the old rush left its frame.
            if !self.map.is_extractor_frame(anchor) {
                return Some(PlaceRefusal::FrameRequired);
            }
        } else {
            // Nothing else may pave over a frame: the ground under a
            // derelict Extractor stays contestable forever.
            let (w, h) = kind.base_stats().size;
            for dy in 0..h {
                for dx in 0..w {
                    if self.map.tile_in_extractor_frame(anchor.offset(dx, dy)) {
                        return Some(PlaceRefusal::FrameBlocked);
                    }
                }
            }
        }
        for dy in 0..h {
            for dx in 0..w {
                let t = anchor.offset(dx, dy);
                if !self.map.terrain_passable(t) {
                    return Some(PlaceRefusal::Terrain);
                }
                if self.known_building_at_except(player, t, ignored) {
                    return Some(PlaceRefusal::Building);
                }
            }
        }
        // Hostile machines hold their ground — standing on a tile
        // denies it to the enemy's foundations. Friendly machines
        // (allies included) never block: they walk off as the site
        // claims the ground (only a routeless body is dealt to the
        // perimeter instantly). A flyer passing overhead blocks
        // nothing either way.
        let hostile_in_footprint = self.units.iter().any(|u| {
            u.hp > 0
                && self.hostile(player, u.player)
                && u.domain() == crate::stats::Domain::Ground
                && {
                    let t = u.tile();
                    t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h
                }
        });
        hostile_in_footprint.then_some(PlaceRefusal::Unit)
    }

    fn known_building_at_except(
        &self,
        player: PlayerId,
        tile: TilePos,
        ignored: Option<crate::BuildingId>,
    ) -> bool {
        self.buildings_at(tile)
            .any(|b| Some(b.id) != ignored && self.building_apparent(player, b))
            || self
                .vision(player)
                .ghosts()
                .iter()
                .any(|ghost| ghost.kind.is_stealthy() && ghost.footprint().any(|t| t == tile))
    }

    pub(crate) fn known_charge_at(&self, player: PlayerId, tile: TilePos) -> bool {
        self.vision(player)
            .ghosts()
            .iter()
            .any(|ghost| ghost.kind.is_stealthy() && ghost.footprint().any(|t| t == tile))
            || (self.vision(player).visible(tile)
                && self.buildings_at(tile).any(|b| {
                    b.hp > 0
                        && b.kind.is_stealthy()
                        && self.hostile(player, b.player)
                        && self.building_apparent(player, b)
                }))
    }

    /// Whether `player` owns a completed building of every kind that
    /// `kind`'s construction requires — the tech tree's construction
    /// gate, shared verbatim by command validation, the armed placement
    /// ghost, and every bot. An unconstructible kind trivially passes
    /// (its own refusal arm answers first).
    pub fn prerequisites_met(&self, player: PlayerId, kind: BuildingKind) -> bool {
        kind.base_stats().construction.is_none_or(|construction| {
            construction.requires.iter().all(|required| {
                self.buildings.iter().any(|building| {
                    building.player == player
                        && building.hp > 0
                        && building.built
                        && building.kind == *required
                })
            })
        })
    }

    /// Converts a clicked footprint tile into the build anchor the player
    /// actually knows. Extractors snap anywhere inside a discovered 2x2
    /// derelict frame; every other building keeps the clicked tile.
    ///
    /// An undiscovered frame is deliberately ignored. The shell uses this
    /// one result for its ghost and command so cursor movement cannot reveal
    /// map-authored information through a hidden snap.
    pub fn canonical_build_anchor(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        clicked: TilePos,
    ) -> TilePos {
        if kind != BuildingKind::Extractor {
            return clicked;
        }
        self.map
            .extractor_frames()
            .iter()
            .copied()
            .find(|frame| {
                clicked.x >= frame.x
                    && clicked.x < frame.x + 2
                    && clicked.y >= frame.y
                    && clicked.y < frame.y + 2
                    && (0..2).any(|dy| {
                        (0..2).any(|dx| self.vision(player).explored(frame.offset(dx, dy)))
                    })
            })
            .unwrap_or(clicked)
    }

    /// Whether the player knows a derelict frame is covered by a live or
    /// remembered claim. Own and allied works are shared facts; a hostile
    /// claim counts only while apparent and visible or retained as a building ghost.
    pub fn extractor_frame_claim_known(&self, player: PlayerId, frame: TilePos) -> bool {
        let vision = self.vision(player);
        self.buildings.iter().any(|building| {
            building.hp > 0
                && building.anchor == frame
                && self.building_apparent(player, building)
                && (!self.hostile(player, building.player)
                    || building.tiles().any(|tile| vision.visible(tile)))
        }) || vision
            .ghosts()
            .iter()
            .any(|ghost| ghost.hp > 0 && ghost.anchor == frame)
    }

    /// Whether `player` may *intend* to build `kind` at `anchor` — the
    /// deferred sibling of [`State::place_refusal`], serving
    /// [`crate::Command::Build`]'s `defer` mode and the shell's ghost
    /// on remembered ground. Per footprint tile: a currently visible
    /// tile takes the strict predicate's live checks verbatim; an
    /// explored-but-unseen tile is judged ONLY on what the issuer
    /// knows — static terrain (immutable after parse), remembered
    /// scrap (conservative: nodes only shrink), remembered enemy
    /// buildings, live own/allied buildings (team-internal facts),
    /// and the issuer's own pending [`Order::Found`] claims; a
    /// never-explored tile refuses as [`PlaceRefusal::Fog`]. Live
    /// hostile units and unremembered enemy buildings on unseen ground
    /// are deliberately unreadable here — two states differing only in
    /// what fog hides return identical verdicts, so the amber ghost
    /// can never be a hidden-enemy detector. The arrival re-check
    /// through the strict predicate is what catches the collisions
    /// memory cannot (an allied scaffold on unseen ground included).
    pub fn place_intent_refusal(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<PlaceRefusal> {
        self.place_intent_refusal_replacing(player, kind, anchor, &[])
    }

    /// The intent verdict for a non-queued build that replaces the programs
    /// of `units`. Claims belonging to live own harvesters in that selection
    /// leave with those programs and therefore do not reserve ground against
    /// the replacement. Claims from every other unit remain blockers.
    pub fn place_intent_refusal_replacing(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
        units: &[UnitId],
    ) -> Option<PlaceRefusal> {
        let released = crate::tick::construction::replaced_sites(self, player, units);
        if kind.base_stats().construction.is_none() {
            return Some(PlaceRefusal::NotConstructible);
        }
        if !self.prerequisites_met(player, kind) {
            return Some(PlaceRefusal::Prerequisite);
        }
        let vision = self.vision(player);
        let (w, h) = kind.base_stats().size;
        // Never distinguish a hidden authored frame from ordinary unknown
        // ground. Explored-but-unseen tiles remain eligible for a deferred
        // intent and are checked from memory below.
        for dy in 0..h {
            for dx in 0..w {
                let tile = anchor.offset(dx, dy);
                if !vision.visible(tile) && !vision.explored(tile) {
                    return Some(PlaceRefusal::Fog);
                }
            }
        }
        if kind == BuildingKind::Extractor {
            // The machine exists only where the old rush left its frame.
            if !self.map.is_extractor_frame(anchor) {
                return Some(PlaceRefusal::FrameRequired);
            }
        } else {
            // Nothing else may pave over a frame: the ground under a
            // derelict Extractor stays contestable forever.
            let (w, h) = kind.base_stats().size;
            for dy in 0..h {
                for dx in 0..w {
                    if self.map.tile_in_extractor_frame(anchor.offset(dx, dy)) {
                        return Some(PlaceRefusal::FrameBlocked);
                    }
                }
            }
        }
        let my_team = self.players[player.0 as usize].team;
        let covers = |a: TilePos, size: (i32, i32), t: TilePos| {
            t.x >= a.x && t.x < a.x + size.0 && t.y >= a.y && t.y < a.y + size.1
        };
        for dy in 0..h {
            for dx in 0..w {
                let t = anchor.offset(dx, dy);
                if vision.visible(t) {
                    if !self.map.terrain_passable(t) {
                        return Some(PlaceRefusal::Terrain);
                    }
                    if self
                        .buildings_at(t)
                        .any(|b| !released.contains(&b.id) && self.building_apparent(player, b))
                        || self.known_charge_at(player, t)
                    {
                        return Some(PlaceRefusal::Building);
                    }
                    continue;
                }
                if !vision.explored(t) {
                    return Some(PlaceRefusal::Fog);
                }
                let terrain = self.map.tile(t).map(|tile| tile.terrain);
                if terrain != Some(crate::map::Terrain::Ground) || vision.remembered_scrap(t) > 0 {
                    return Some(PlaceRefusal::Terrain);
                }
                let ghosted = vision
                    .ghosts()
                    .iter()
                    .any(|g| covers(g.anchor, g.kind.base_stats().size, t));
                let allied_building = self.buildings.iter().any(|b| {
                    self.players[b.player.0 as usize].team == my_team
                        && !released.contains(&b.id)
                        && covers(b.anchor, b.stats().size, t)
                });
                if ghosted || allied_building {
                    return Some(PlaceRefusal::Building);
                }
            }
        }
        // The issuer's own outstanding claims: two deferred founds may
        // not promise the same ground (checked over the whole footprint
        // so a visible/unseen mix cannot slip a double claim through).
        let claimed = self.units.iter().any(|u| {
            u.player == player
                && u.hp > 0
                && !(u.kind.stats().harvest.is_some() && units.contains(&u.id))
                && std::iter::once(&u.order).chain(u.queue.iter()).any(|o| {
                    matches!(o, Order::Found { kind: k, anchor: a }
                    if (0..h).any(|dy| (0..w).any(|dx| {
                        covers(*a, k.base_stats().size, anchor.offset(dx, dy))
                    })))
                })
        });
        if claimed {
            return Some(PlaceRefusal::Building);
        }
        // Hostile machines deny only ground the issuer can SEE them
        // holding — exactly the strict rule, restricted to visible
        // footprint tiles.
        let hostile_in_sight = self.units.iter().any(|u| {
            u.hp > 0
                && self.hostile(player, u.player)
                && u.domain() == crate::stats::Domain::Ground
                && {
                    let t = u.tile();
                    covers(anchor, (w, h), t) && vision.visible(t)
                }
        });
        hostile_in_sight.then_some(PlaceRefusal::Unit)
    }
}
