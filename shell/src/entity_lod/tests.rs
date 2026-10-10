use super::*;
#[test]
fn live_body_frames_have_cached_production_contact_sources() {
    let manifest: HashMap<String, [f32; 4]> =
        serde_json::from_str(include_str!("../../../assets/sprites/atlas.json")).unwrap();
    let sources = contact_sources(&manifest);
    let loaded_sources = entity_sources(&manifest);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/sprites");
    let bytes = std::fs::read(root.join("atlas.png")).unwrap();
    let first = Image::from_file_with_format(&bytes, Some(ImageFormat::Png)).unwrap();
    let page_height = u32::from(first.height);
    let mut pages = vec![first];
    for name in [
        "rig_harvester_body_cargo5",
        "rig_excavator_body_move2",
        "rig_tender_body_move1",
        "rig_scuttler_body_move2",
        "rig_buzzard_hull_move2",
        "bombard_action2",
    ] {
        let key = manifest[name].map(numeric::to_u32);
        assert!(sources.contains(&key), "uncached live body frame: {name}");
        assert!(
            loaded_sources.contains(&key),
            "body frame omitted from load: {name}"
        );
        let page = (key[1] / page_height) as usize;
        while pages.len() <= page {
            let bytes = std::fs::read(root.join(format!("atlas_{}.png", pages.len()))).unwrap();
            pages.push(Image::from_file_with_format(&bytes, Some(ImageFormat::Png)).unwrap());
        }
        let mask = crate::sprite_contact::SpriteContact::capture(
            &pages[page],
            [key[0], key[1] % page_height, key[2], key[3]],
        );
        assert!(
            mask.contact(vec2(0.0, -2.0), Vec2::ZERO, Vec2::splat(-0.5), Vec2::ONE)
                .is_some(),
            "empty body mask: {name}"
        );
    }
}

#[test]
fn layered_units_omit_unused_full_poses_but_keep_portraits_and_fallbacks() {
    let mut manifest = HashMap::from([
        ("buzzard".to_owned(), [0.0, 0.0, 128.0, 128.0]),
        ("buzzard_move1".to_owned(), [128.0, 0.0, 128.0, 128.0]),
        ("buzzard_action2".to_owned(), [256.0, 0.0, 128.0, 128.0]),
        (
            "buzzard_accent_move1".to_owned(),
            [640.0, 0.0, 128.0, 128.0],
        ),
    ]);
    assert_eq!(entity_sources(&manifest).len(), 4);
    manifest.insert("rig_buzzard_hull".to_owned(), [384.0, 0.0, 128.0, 128.0]);
    manifest.insert(
        "rig_buzzard_mount_action2".to_owned(),
        [512.0, 0.0, 128.0, 128.0],
    );
    assert_eq!(
        entity_sources(&manifest),
        BTreeSet::from([[0, 0, 128, 128], [384, 0, 128, 128], [512, 0, 128, 128],])
    );
}

#[test]
fn production_mips_fit_seven_pages_without_discarding_levels() {
    let manifest: HashMap<String, [f32; 4]> =
        serde_json::from_str(include_str!("../../../assets/sprites/atlas.json")).unwrap();
    let sources = entity_sources(&manifest);
    let order = packing_order(&sources);
    assert_eq!(order.len(), sources.len() * LEVELS.len());
    let mut packer = Packer::new();
    for (key, level) in order {
        let factor = LEVELS[level].fit::<u32>();
        let region = packer.insert(&Image::gen_image_color(
            (key[2] / factor).fit::<u16>(),
            (key[3] / factor).fit::<u16>(),
            WHITE,
        ));
        assert!(region.rect.right() < PAGE as f32);
        assert!(region.rect.bottom() < PAGE as f32);
    }
    assert!(
        packer.images.len() <= 7,
        "{} mip pages exceed the seven-page budget",
        packer.images.len()
    );
}

#[test]
fn mip_vertex_data_preserves_flip_pivot_rotation_and_tint() {
    let params = DrawTextureParams {
        dest_size: Some(vec2(40.0, 20.0)),
        flip_x: true,
        flip_y: true,
        pivot: Some(vec2(10.0, 20.0)),
        rotation: std::f32::consts::FRAC_PI_2,
        ..Default::default()
    };
    let a = Rect::new(128.0, 256.0, 64.0, 32.0);
    let b = Rect::new(64.0, 512.0, 32.0, 16.0);
    let tint = Color::new(0.3, 0.6, 0.9, 0.4);
    let vertices = sprite_vertices(vec2(10.0, 20.0), tint, &params, a, b, 0.6);
    for (vertex, expected) in vertices.iter().zip([
        vec2(-10.0, 60.0),
        vec2(-10.0, 20.0),
        vec2(10.0, 20.0),
        vec2(10.0, 60.0),
    ]) {
        assert!(vertex.position.truncate().distance(expected) < 0.0001);
        assert_eq!(vertex.color, <[u8; 4]>::from(tint));
        assert_eq!(vertex.normal.z, 0.6);
        assert_eq!(vertex.normal.w, 1.0);
    }
    assert_eq!(vertices[0].uv, vec2(128.0, 256.0) / PAGE as f32);
    assert_eq!(vertices[2].uv, vec2(192.0, 288.0) / PAGE as f32);
    assert_eq!(
        vertices[0].normal.truncate().truncate(),
        vec2(64.0, 512.0) / PAGE as f32
    );
    assert_eq!(
        vertices[2].normal.truncate().truncate(),
        vec2(96.0, 528.0) / PAGE as f32
    );
}

#[test]
fn mip_quad_rotates_about_the_destination_center_by_default() {
    let params = DrawTextureParams {
        dest_size: Some(vec2(40.0, 20.0)),
        rotation: std::f32::consts::PI,
        ..Default::default()
    };
    let source = Rect::new(0.0, 0.0, 64.0, 64.0);
    let vertices = sprite_vertices(vec2(10.0, 20.0), WHITE, &params, source, source, 0.0);
    assert!(vertices[0].position.truncate().distance(vec2(50.0, 40.0)) < 0.0001);
    assert!(vertices[2].position.truncate().distance(vec2(10.0, 20.0)) < 0.0001);
}

#[test]
fn reduction_keeps_color_premultiplied_through_transparent_edges() {
    let image = Image {
        width: 2,
        height: 2,
        bytes: vec![255, 80, 10, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0],
    };
    assert_eq!(reduce(&image, [0, 0, 2, 2], 2).bytes, [63, 20, 2, 63]);
    let full = reduce(&image, [0, 0, 2, 2], 1);
    assert_eq!(&full.bytes[4..8], &[0, 0, 0, 0]);
}
#[test]
fn physical_size_and_both_axes_select_detail_biased_levels() {
    let (a, b, blend) = lod_mix(vec2(128.0, 128.0), vec2(32.0, 32.0));
    assert_eq!((a, b), (1, 2));
    assert!((blend - 0.6).abs() < 0.0001);
    assert_eq!(lod_mix(vec2(128.0, 64.0), vec2(128.0, 16.0)), (a, b, blend),);
    let (a, b, blend) = lod_mix(vec2(128.0, 128.0), vec2(64.0, 64.0));
    assert_eq!((a, b), (0, 1));
    assert!((blend - 0.6).abs() < 0.0001);
    assert_eq!(lod_mix(vec2(128.0, 128.0), vec2(256.0, 256.0)), (0, 0, 0.0));
}
#[test]
fn levels_change_continuously_at_every_boundary() {
    for width in 8000..129_000 {
        let weights = |w| {
            let (a, b, t) = lod_mix(vec2(128.0, 128.0), vec2(w, w));
            let mut out = [0.0; 4];
            out[a] += 1.0 - t;
            out[b] += t;
            out
        };
        let a = weights(width as f32 / 1000.0);
        let b = weights((width + 1) as f32 / 1000.0);
        for (a, b) in a.into_iter().zip(b) {
            assert!((a - b).abs() < 0.001);
        }
    }
}
#[test]
fn building_layers_and_resources_enter_the_bank_but_terrain_does_not() {
    for key in [
        "foundry",
        "flak_turret",
        "flak_mount_accent",
        "rig_array_t1_rotor",
        "repair_bay",
        "scuttle_charge_accent",
        "scrap_full",
        "wreck_pile",
        "harvester_cargo0",
    ] {
        assert!(is_entity_source(key), "{key}");
    }
    for key in ["ground_0", "peak_barrier_1", "scorch"] {
        assert!(!is_entity_source(key));
    }
}
#[test]
fn packed_borders_do_not_import_adjacent_sprites() {
    let mut packer = Packer::new();
    let red = packer.insert(&Image::gen_image_color(2, 2, RED));
    packer.insert(&Image::gen_image_color(2, 2, BLUE));
    for y in numeric::to_u32(red.rect.y) - 1..=numeric::to_u32(red.rect.y) + 2 {
        for x in numeric::to_u32(red.rect.x) - 1..=numeric::to_u32(red.rect.x) + 2 {
            assert_eq!(
                packer.images[red.page].get_pixel(x, y),
                packer.images[red.page].get_pixel(2, 2)
            );
        }
    }
    packer.x = PAGE - 2;
    packer.y = PAGE - 2;
    assert_eq!(packer.insert(&Image::gen_image_color(2, 2, GREEN)).page, 1);
}
