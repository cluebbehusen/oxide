use super::*;

const SOUND_NAMES: [&str; 40] = [
    "ack",
    "alert",
    "artillery_boom",
    "artillery_launch",
    "attack_bastion",
    "attack_bombard",
    "attack_breaker",
    "attack_buzzard",
    "attack_darter",
    "attack_flak_turret",
    "attack_flakhound",
    "attack_lancer",
    "attack_scuttler",
    "attack_sentinel",
    "attack_stinger",
    "attack_talon",
    "attack_warden",
    "attack_wisp",
    "avalanche_launch",
    "avalanche_motor",
    "bomb_release",
    "building_boom",
    "click",
    "defeat",
    "demolition_boom",
    "denied",
    "deposit",
    "laser",
    "laser2",
    "music_calm",
    "music_combat",
    "music_defeat",
    "music_menu",
    "music_result",
    "music_victory",
    "rocket_impact",
    "train_done",
    "unit_death",
    "upgrade_done",
    "victory",
];

/// Each defense rung's hull and mount stems.
fn defense_rungs() -> Vec<(String, String)> {
    BuildingKind::ALL
        .into_iter()
        .filter_map(|kind| crate::look::defense(kind).map(|look| (kind, look)))
        .flat_map(|(kind, look)| {
            (0..kind.tiers().len()).map(move |tier| {
                (
                    rung_stem(building_stem(kind), tier),
                    rung_stem(look.mount, tier),
                )
            })
        })
        .collect()
}

fn manifest() -> Manifest {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/sprites/atlas.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_str(&text).expect("atlas.json maps names to [x, y, w, h]")
}

#[test]
fn array_layers_require_both_tiers_and_every_allegiance_mask() {
    let mut atlas = Manifest::default();
    assert!(array_rig(&atlas).unwrap().is_none());
    for tier in 0..2 {
        for (part_index, part) in ["base", "rotor"].into_iter().enumerate() {
            for (faction_index, faction) in ["ferrous", "cupric", "accent"].into_iter().enumerate()
            {
                atlas.insert(
                    format!("rig_array_t{tier}_{part}_{faction}"),
                    [
                        tier as f32,
                        part_index as f32,
                        faction_index as f32 + 128.0,
                        128.0,
                    ],
                );
            }
        }
    }
    let rig = array_rig(&atlas).unwrap().unwrap();
    let layers = rig.layers(1, Faction::Cupric);
    assert_eq!(layers[0].0, Rect::new(1.0, 0.0, 129.0, 128.0));
    assert_eq!(layers[1].1, Rect::new(1.0, 1.0, 130.0, 128.0));
    for key in atlas.keys() {
        let mut incomplete = atlas.clone();
        incomplete.remove(key);
        assert!(array_rig(&incomplete).is_err(), "missing {key}");
    }
}

#[test]
fn articulated_unit_bank_is_optional_but_must_be_complete() {
    for (stem, actions) in [
        ("sentinel", 4),
        ("warden", 4),
        ("lancer", 6),
        ("buzzard", 4),
        ("wisp", 4),
        ("skyhook", 4),
    ] {
        assert!(unit_rig(&manifest(), stem, actions).unwrap().is_some());
        let mut atlas = Manifest::default();
        assert!(unit_rig(&atlas, stem, actions).unwrap().is_none());
        atlas.insert(format!("rig_{stem}_hull_ferrous"), [0.0, 0.0, 128.0, 128.0]);
        assert!(unit_rig(&atlas, stem, actions).is_err());
        for faction in ["ferrous", "cupric", "accent"] {
            for suffix in ["", "_move1", "_move2"] {
                atlas.insert(
                    format!("rig_{stem}_hull_{faction}{suffix}"),
                    [8.0, 16.0, 128.0, 128.0],
                );
            }
            for action in 0..=actions {
                let suffix = if action == 0 {
                    String::new()
                } else {
                    format!("_action{action}")
                };
                atlas.insert(
                    format!("rig_{stem}_mount_{faction}{suffix}"),
                    [action as f32, 16.0, 128.0, 128.0],
                );
            }
        }
        let rig = unit_rig(&atlas, stem, actions).unwrap().unwrap();
        assert_eq!(
            rig.hull(Faction::Cupric, 2).0,
            Rect::new(8.0, 16.0, 128.0, 128.0)
        );
        assert_eq!(
            rig.mount(Faction::Ferrous, Some(actions - 1)).1,
            Rect::new(actions as f32, 16.0, 128.0, 128.0)
        );
        atlas.remove(&format!("rig_{stem}_mount_accent_action{actions}"));
        assert!(unit_rig(&atlas, stem, actions).is_err());
    }
}

#[test]
fn harvester_body_layers_require_every_cargo_and_faction_mask() {
    let mut atlas = Manifest::new();
    assert!(harvester_body_rows(&atlas).unwrap().is_none());
    for cargo in 0..6 {
        for key in variant_keys("rig_harvester_body", &format!("_cargo{cargo}")) {
            atlas.insert(key, [1., 1., 128., 128.]);
        }
    }
    assert!(harvester_body_rows(&atlas).unwrap().is_some());
    atlas.remove("rig_harvester_body_accent_cargo5");
    assert!(harvester_body_rows(&atlas).is_err());
}

#[test]
fn quarry_dressing_is_optional_but_partial_banks_are_rejected() {
    let mut atlas = Manifest::new();
    assert!(quarry_dressing_rows(&atlas).unwrap().is_none());
    atlas.insert("quarry_dressing_0".into(), [0.0, 0.0, 64.0, 64.0]);
    assert!(quarry_dressing_rows(&atlas).is_err());
    for index in 1..12 {
        atlas.insert(
            format!("quarry_dressing_{index}"),
            [index as f32 * 66.0, 0.0, 64.0, 64.0],
        );
    }
    assert_eq!(quarry_dressing_rows(&atlas).unwrap().unwrap().len(), 12);
}

#[test]
fn optional_bombard_spades_require_every_deployment_pose() {
    let mut atlas = Manifest::new();
    assert!(bombard_spade_rows(&atlas).unwrap().is_none());
    atlas.insert("bombard_spades_0".into(), [0.0, 0.0, 128.0, 128.0]);
    assert!(bombard_spade_rows(&atlas).is_err());
    for phase in 1..5 {
        atlas.insert(
            format!("bombard_spades_{phase}"),
            [phase as f32, 0.0, 128.0, 128.0],
        );
    }
    assert_eq!(bombard_spade_rows(&atlas).unwrap().unwrap()[4].x, 4.0);
}

#[test]
fn generated_sound_bank_is_complete_and_has_valid_pcm_metadata() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/sounds");
    let mut actual: Vec<String> = std::fs::read_dir(&dir)
        .expect("assets/sounds exists")
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("wav"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    actual.sort();
    let expected: Vec<String> = SOUND_NAMES
        .iter()
        .map(|name| format!("{name}.wav"))
        .collect();
    assert_eq!(
        actual, expected,
        "tools/gen_sounds.py and the checked-in bank must be a bijection"
    );

    for name in SOUND_NAMES {
        let path = dir.join(format!("{name}.wav"));
        let bytes =
            std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        assert!(
            bytes.len() >= 44,
            "{} is shorter than a WAV header",
            path.display()
        );
        assert_eq!(&bytes[0..4], b"RIFF", "{} is not RIFF", path.display());
        assert_eq!(&bytes[8..12], b"WAVE", "{} is not WAVE", path.display());
        assert_eq!(
            u16::from_le_bytes([bytes[22], bytes[23]]),
            1,
            "{} must stay mono",
            path.display()
        );
        let expected_rate = if name.starts_with("music_") {
            22_050
        } else {
            44_100
        };
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            expected_rate,
            "{} changed sample rate",
            path.display()
        );
        assert_eq!(
            u16::from_le_bytes([bytes[34], bytes[35]]),
            16,
            "{} must stay 16-bit PCM",
            path.display()
        );
        if name.starts_with("music_") {
            let data_bytes =
                u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
            assert_eq!(
                data_bytes,
                12 * 22_050 * 2,
                "{} is not the authored twelve-second loop",
                path.display()
            );
        }
    }
}

#[test]
fn the_shell_and_the_atlas_name_the_same_sprites() {
    let atlas = manifest();
    let named = atlas_keys(&atlas);
    let mut missing: Vec<&String> = named.iter().filter(|k| !atlas.contains_key(*k)).collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "the shell asks for sprites the atlas does not ship: {missing:?} \
             (regenerate with tools/gen_sprites.py)"
    );
    // The other direction: every atlas row needs a reader.
    // gen_sprites.py and the shell ship together, so an unread row is
    // an incomplete change.
    let mut orphans: Vec<&String> = atlas.keys().filter(|k| !named.contains(k)).collect();
    orphans.sort();
    assert!(
        orphans.is_empty(),
        "the atlas ships sprites the shell never draws: {orphans:?}"
    );
    // Containment both ways still allows the shell to name one
    // region twice; a bijection does not.
    assert_eq!(named.len(), atlas.len(), "a sprite is named twice");
}

#[test]
fn the_building_loader_finds_every_kind_in_the_shipped_atlas() {
    let art = building_art(&manifest()).expect("every building bank is in the atlas");
    assert_eq!(art.len(), BuildingKind::ALL.len());
}

#[test]
fn the_unit_loader_finds_every_kind_in_the_shipped_atlas() {
    let art = unit_art(&manifest()).expect("every unit bank is in the atlas");
    for kind in UnitKind::ALL {
        assert_eq!(
            art[kind as usize].action.len(),
            unit_action_frames(kind),
            "{kind:?}"
        );
    }
}

#[test]
fn atlas_regions_fit_a_portable_texture_with_extrusion_room() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/sprites");
    let atlas = manifest();
    let page_count = atlas
        .values()
        .map(|row| numeric::to_usize(row[1]) / 4096 + 1)
        .max()
        .unwrap();
    let pages: Vec<_> = (0..page_count)
        .map(|page| {
            let name = if page == 0 {
                "atlas.png".to_owned()
            } else {
                format!("atlas_{page}.png")
            };
            let bytes = std::fs::read(root.join(name)).unwrap();
            let image = macroquad::prelude::Image::from_file_with_format(
                &bytes,
                Some(macroquad::prelude::ImageFormat::Png),
            )
            .unwrap();
            assert!(
                image.width <= 4096 && image.height <= 4096,
                "atlas page exceeds the 4096px texture limit"
            );
            image
        })
        .collect();
    for (name, [x, y, w, h]) in atlas {
        let (page, local) = atlas_page(Rect::new(x, y, w, h), 4096.0);
        let image = &pages[page];
        assert!(w > 0.0 && h > 0.0, "{name} has an empty atlas region");
        assert!(
            local.x >= 1.0
                && local.y >= 1.0
                && local.x + w < f32::from(image.width)
                && local.y + h < f32::from(image.height),
            "{name} leaves no room for its one-pixel edge extrusion"
        );
    }
}

#[test]
fn atlas_page_coordinates_preserve_sprite_canvas_and_legacy_banks() {
    let source = Rect::new(17.0, 4098.0, 128.0, 64.0);
    assert_eq!(
        atlas_page(source, 4096.0),
        (1, Rect::new(17.0, 2.0, 128.0, 64.0))
    );
    assert_eq!(atlas_page(source, 5680.0), (0, source));
    let first = Rect::new(17.0, 3964.0, 128.0, 128.0);
    assert_eq!(atlas_page(first, 4096.0), (0, first));
}

#[test]
fn defense_mounts_match_their_footprints_and_rotate_about_square_canvases() {
    let atlas = manifest();
    for (base_stem, mount) in defense_rungs() {
        let base = &atlas[&variant_keys(&base_stem, "")[0]];
        let actions = numbered_suffixes(&atlas, &mount, "action").unwrap();
        assert!(!actions.is_empty(), "{mount} ships its firing frames");
        for suffix in std::iter::once("").chain(actions.iter().map(String::as_str)) {
            for key in variant_keys(&mount, suffix) {
                let rect = &atlas[&key];
                assert_eq!(
                    rect[2..],
                    base[2..],
                    "{key} must share the defense footprint"
                );
                assert_eq!(rect[2], rect[3], "{key} needs a centered rotation pivot");
            }
        }
    }
    for suffix in numbered_suffixes(&atlas, "bastion", "action").unwrap() {
        for key in variant_keys("bastion", &suffix) {
            assert_eq!(
                atlas[&key][2..],
                atlas[&variant_keys("bastion", "")[0]][2..],
                "{key} must stay synchronized with the Bastion mount footprint"
            );
        }
    }
}

fn sprite_image(name: &str) -> macroquad::prelude::Image {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../assets/sprites")
        .join(format!("{name}.png"));
    let bytes =
        std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    macroquad::prelude::Image::from_file_with_format(
        &bytes,
        Some(macroquad::prelude::ImageFormat::Png),
    )
    .unwrap_or_else(|err| panic!("decoding {}: {err}", path.display()))
}

fn alpha_bytes(image: &macroquad::prelude::Image) -> impl Iterator<Item = u8> + '_ {
    image.bytes.as_chunks::<4>().0.iter().map(|pixel| pixel[3])
}

fn opaque_bounds(image: &macroquad::prelude::Image) -> (usize, usize, usize, usize) {
    let width = usize::from(image.width);
    let height = usize::from(image.height);
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for (index, pixel) in image.bytes.as_chunks::<4>().0.iter().enumerate() {
        if pixel[3] < 64 {
            continue;
        }
        let x = index % width;
        let y = index / width;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        found = true;
    }
    assert!(found, "sprite has no opaque art");
    (min_x, min_y, max_x, max_y)
}

fn opaque_span(image: &macroquad::prelude::Image) -> (usize, usize) {
    let (min_x, min_y, max_x, max_y) = opaque_bounds(image);
    (max_x - min_x + 1, max_y - min_y + 1)
}

fn assert_animation_variant(stem: &str, suffix: &str) {
    let ferrous = sprite_image(&format!("{stem}_ferrous{suffix}"));
    let cupric = sprite_image(&format!("{stem}_cupric{suffix}"));
    let accent = sprite_image(&format!("{stem}_accent{suffix}"));
    assert_eq!(
        (ferrous.width, ferrous.height),
        (cupric.width, cupric.height),
        "{stem}{suffix} faction frames must share a footprint"
    );
    assert_eq!(
        (ferrous.width, ferrous.height),
        (accent.width, accent.height),
        "{stem}{suffix} accent must share the frame footprint"
    );
    assert_eq!(
        alpha_bytes(&ferrous).collect::<Vec<_>>(),
        alpha_bytes(&cupric).collect::<Vec<_>>(),
        "{stem}{suffix} variants must share a silhouette"
    );
    assert!(
        alpha_bytes(&accent).any(|alpha| alpha > 0),
        "{stem}{suffix} needs a non-empty allegiance mask"
    );
}

#[test]
fn authored_animation_families_are_complete_distinct_and_faction_safe() {
    for (stem, suffixes) in [
        ("harvester", TREAD_SUFFIXES),
        ("sentinel", MOVE_SUFFIXES),
        ("scuttler", MOVE_SUFFIXES),
        ("lancer", MOVE_SUFFIXES),
        ("bombard", MOVE_SUFFIXES),
        ("flakhound", TREAD_SUFFIXES),
        ("stinger", MOVE_SUFFIXES),
        ("buzzard", MOVE_SUFFIXES),
        ("darter", MOVE_SUFFIXES),
        ("talon", MOVE_SUFFIXES),
        ("wisp", MOVE_SUFFIXES),
    ] {
        let mut ferrous_seen = vec![sprite_image(&format!("{stem}_ferrous")).bytes];
        let mut cupric_seen = vec![sprite_image(&format!("{stem}_cupric")).bytes];
        for suffix in suffixes {
            assert_animation_variant(stem, suffix);
            let ferrous = sprite_image(&format!("{stem}_ferrous{suffix}")).bytes;
            let cupric = sprite_image(&format!("{stem}_cupric{suffix}")).bytes;
            assert_ne!(
                ferrous_seen.last().unwrap(),
                &ferrous,
                "{stem}{suffix} must advance its locomotion cycle"
            );
            assert_ne!(
                cupric_seen.last().unwrap(),
                &cupric,
                "{stem}{suffix} must advance its locomotion cycle"
            );
            ferrous_seen.push(ferrous);
            cupric_seen.push(cupric);
        }
    }

    let harvester = sprite_image("harvester_ferrous");
    let base_alpha = alpha_bytes(&harvester).collect::<Vec<_>>();
    for suffix in TREAD_SUFFIXES {
        let tread = sprite_image(&format!("harvester_ferrous{suffix}"));
        assert_eq!(
            alpha_bytes(&tread).collect::<Vec<_>>(),
            base_alpha,
            "harvester{suffix} cleats must stay inside the resting silhouette"
        );
    }

    let working = BuildingKind::ALL
        .into_iter()
        .map(|kind| {
            let work = numbered_suffixes(&manifest(), building_stem(kind), "work").unwrap();
            (kind, work)
        })
        .filter(|(_, work)| !work.is_empty());
    for (kind, work) in working {
        let stem = building_stem(kind);
        let base = sprite_image(&format!("{stem}_ferrous"));
        let mut changed = false;
        for suffix in &work {
            assert_animation_variant(stem, suffix);
            changed |= sprite_image(&format!("{stem}_ferrous{suffix}")).bytes != base.bytes;
        }
        assert!(changed, "{stem} needs at least one visible work pose");
    }
    let refinery = sprite_image("reclaimer_t1_ferrous");
    let mut changed = false;
    for suffix in numbered_suffixes(&manifest(), "reclaimer_t1", "work").unwrap() {
        assert_animation_variant("reclaimer_t1", &suffix);
        changed |= sprite_image(&format!("reclaimer_t1_ferrous{suffix}")).bytes != refinery.bytes;
    }
    assert!(changed, "reclaimer_t1 needs at least one visible work pose");
    let deep_array = sprite_image("array_t1_ferrous");
    let mut changed = false;
    for suffix in numbered_suffixes(&manifest(), "array_t1", "work").unwrap() {
        assert_animation_variant("array_t1", &suffix);
        changed |= sprite_image(&format!("array_t1_ferrous{suffix}")).bytes != deep_array.bytes;
    }
    assert!(changed, "array_t1 needs at least one visible work pose");
    for kind in BuildingKind::ALL {
        let stem = building_stem(kind);
        let base = sprite_image(&format!("{stem}_ferrous"));
        for stage in 0..SITE_STAGES {
            let mut phases = Vec::new();
            for phase in 0..SITE_PHASES {
                let suffix = format!("_site{stage}_{phase}");
                assert_animation_variant(stem, &suffix);
                let frame = sprite_image(&format!("{stem}_ferrous{suffix}"));
                assert_eq!(
                    (frame.width, frame.height),
                    (base.width, base.height),
                    "{stem}{suffix} must fill the final footprint"
                );
                phases.push(frame.bytes);
            }
            assert_ne!(
                phases[0], phases[1],
                "{stem} site stage {stage} must move its authored machinery"
            );
        }
    }
}

#[test]
fn production_action_and_cargo_rows_match_the_runtime_contract() {
    for kind in UnitKind::ALL {
        let stem = unit_stem(kind);
        for suffix in numbered_suffixes(&manifest(), stem, "action").unwrap() {
            assert_animation_variant(stem, &suffix);
        }
    }

    for level in 0..HARVESTER_CARGO_LEVELS {
        for suffix in std::iter::once(format!("_cargo{level}")).chain(
            TREAD_SUFFIXES
                .into_iter()
                .chain(SCOOP_SUFFIXES)
                .map(|pose| format!("_cargo{level}{pose}")),
        ) {
            assert_animation_variant("harvester", &suffix);
        }
    }
    let atlas = manifest();
    for level in 0..EXCAVATOR_CARGO_LEVELS {
        assert!(
            atlas.contains_key(&format!("excavator_cargo{level}")),
            "missing Excavator cargo level {level}"
        );
    }

    let charge_racks = BuildingKind::ALL
        .into_iter()
        .filter(|kind| crate::look::defense(*kind).is_some_and(|look| look.charge_rack))
        .map(|kind| building_stem(kind).to_owned());
    for stem in defense_rungs()
        .into_iter()
        .map(|(_, mount)| mount)
        .chain(charge_racks)
    {
        for suffix in numbered_suffixes(&manifest(), &stem, "action").unwrap() {
            assert_animation_variant(&stem, &suffix);
        }
    }
}

#[test]
fn defense_mount_art_covers_its_pivot_and_carries_an_allegiance_mask() {
    for (_, stem) in defense_rungs() {
        let ferrous = sprite_image(&format!("{stem}_ferrous"));
        let cupric = sprite_image(&format!("{stem}_cupric"));
        let accent = sprite_image(&format!("{stem}_accent"));

        assert_eq!(
            (ferrous.width, ferrous.height),
            (cupric.width, cupric.height)
        );
        assert_eq!(
            (ferrous.width, ferrous.height),
            (accent.width, accent.height)
        );
        assert_eq!(
            alpha_bytes(&ferrous).collect::<Vec<_>>(),
            alpha_bytes(&cupric).collect::<Vec<_>>(),
            "{stem} variants must rotate as one silhouette"
        );

        let (min_x, min_y, max_x, max_y) = opaque_bounds(&ferrous);
        let center_x = usize::from(ferrous.width / 2);
        let center_y = usize::from(ferrous.height / 2);
        assert!(
            min_x <= center_x && center_x <= max_x && min_y <= center_y && center_y <= max_y,
            "{stem} opaque bounds must straddle its centered rotation pivot"
        );
        assert!(
            alpha_bytes(&accent).any(|alpha| alpha > 0),
            "{stem} needs a non-empty allegiance mask"
        );
    }
}

#[test]
fn defense_mounts_preserve_separate_banks_and_a_compact_siege_carriage() {
    let flak = sprite_image("flak_mount_ferrous");
    let upgraded_flak = sprite_image("flak_mount_t1_ferrous");
    for image in [&flak, &upgraded_flak] {
        let width = usize::from(image.width);
        let row = &image.bytes.as_chunks::<4>().0[25 * width..26 * width];
        let bank_starts: Vec<_> = row
            .iter()
            .enumerate()
            .filter(|(x, pixel)| pixel[3] >= 128 && (*x == 0 || row[x - 1][3] < 128))
            .map(|(x, _)| x)
            .collect();
        assert_eq!(
            bank_starts.len(),
            2,
            "the two Flak barrel banks must remain separate"
        );
        assert!(bank_starts[0] < width / 2 && bank_starts[1] > width / 2);
        assert_eq!(row[width / 2][3], 0, "the center gap must remain open");
    }
    assert!(opaque_span(&upgraded_flak).0 > opaque_span(&flak).0);

    let bastion = sprite_image("bastion_mount_ferrous");
    let (width, height) = opaque_span(&bastion);
    assert!(
        width * 2 < height,
        "the low-carriage gun must keep its narrow traverse fork"
    );
    assert!(height * 4 >= usize::from(bastion.height) * 3);
}

#[test]
fn every_theme_prop_row_is_tile_sized_and_ordered_like_the_lookup() {
    let atlas = manifest();
    for (row, (theme, stem)) in [
        ("rusted-yard", "rusted_yard"),
        ("cold-circuitry", "cold_circuitry"),
        ("quarry-dust", "quarry_dust"),
        ("basalt", "basalt"),
        ("slag", "slag"),
        ("verdigris", "verdigris"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(theme_prop_row(theme), Some(row));
        for variant in 0..6 {
            let key = format!("theme_{stem}_{variant}");
            assert_eq!(THEME_PROP_KEYS[row * 6 + variant], key);
            assert_eq!(atlas[&key][2..], [64.0, 64.0]);
        }
    }
    assert_eq!(theme_prop_row("unknown"), None);
}

#[test]
fn finalized_environment_regions_keep_their_authored_footprints() {
    let atlas = manifest();
    for key in FIELD_DEBRIS_KEYS {
        let [_, _, width, height] = atlas[key];
        assert!(matches!((width, height), (32.0, 32.0) | (64.0, 64.0)));
    }
    for (key, footprint) in GROUND_BLOCKER_KEYS.into_iter().zip([
        (2, 2),
        (2, 1),
        (3, 2),
        (2, 2),
        (3, 2),
        (3, 2),
        (3, 1),
        (2, 2),
        (2, 2),
    ]) {
        assert_eq!(
            atlas[key][2..],
            [footprint.0 as f32 * 64.0, footprint.1 as f32 * 64.0]
        );
    }
    for (key, footprint) in ROCK_KEYS.into_iter().zip(
        std::iter::repeat_n((1, 1), 14)
            .chain(std::iter::repeat_n((2, 1), 5))
            .chain(std::iter::repeat_n((3, 1), 4)),
    ) {
        assert_eq!(
            atlas[key][2..],
            [footprint.0 as f32 * 64.0, footprint.1 as f32 * 64.0]
        );
    }
}

#[test]
fn every_peak_neighbor_mask_has_two_full_tile_barriers() {
    let atlas = manifest();
    for mask in 0..16 {
        for variant in 0..2 {
            let key = format!("peak_barrier_{mask:02x}_{variant}");
            assert_eq!(PEAK_BARRIER_KEYS[mask * 2 + variant], key);
            assert_eq!(atlas[&key][2..], [64.0, 64.0]);
        }
    }
}
