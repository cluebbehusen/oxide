//! Independently filtered unit sprites; atlas neighbors never enter a reduction.

use anyhow::{Context, Result};
use macroquad::prelude::*;
use std::collections::{BTreeSet, HashMap};

const PAGE: usize = 2048;
const LEVELS: [usize; 3] = [2, 4, 8];
type Source = [u32; 4];

#[derive(Debug, Clone, Copy)]
struct Region {
    page: usize,
    rect: Rect,
}

pub(crate) struct UnitLod {
    pages: Vec<Texture2D>,
    materials: Vec<Material>,
    sprites: HashMap<Source, [Region; 3]>,
}

fn source_key(rect: Rect) -> Source {
    [rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32]
}

fn is_unit_source(name: &str) -> bool {
    let stem = name
        .strip_prefix("rig_")
        .unwrap_or(name)
        .split('_')
        .next()
        .unwrap_or("");
    name == "scout_radar"
        || oxide_sim::UnitKind::ALL
            .iter()
            .any(|&kind| crate::assets::unit_stem(kind) == stem)
}

// Narrow, continuous blends retain the approved nearest-level appearance away
// from boundaries. Level zero is the original atlas.
fn lod_mix(source_width: f32, physical_width: f32, zoom: f32) -> (usize, usize, f32) {
    let lod = (source_width / physical_width.max(1.0))
        .log2()
        .clamp(0.0, 3.0)
        * ((32.0 - zoom) / 4.0).clamp(0.0, 1.0);
    let level = lod.round() as usize;
    let boundary = (lod.floor() + 0.5).min(2.5);
    let (low, high, blend) = if (lod - boundary).abs() < 0.12 {
        (
            boundary.floor() as usize,
            boundary.ceil() as usize,
            (lod - boundary + 0.12) / 0.24,
        )
    } else {
        (level, level, 0.0)
    };
    (low, high, blend)
}

impl UnitLod {
    pub(crate) async fn load(
        manifest: &HashMap<String, [f32; 4]>,
        page_height: f32,
    ) -> Result<Self> {
        let sources: BTreeSet<Source> = manifest
            .iter()
            .filter(|(name, _)| is_unit_source(name))
            .map(|(_, row)| row.map(|v| v as u32))
            .collect();
        let count = sources
            .iter()
            .map(|key| key[1] as usize / page_height as usize + 1)
            .max()
            .unwrap_or(0);
        let mut originals = Vec::new();
        for index in 0..count {
            let name = if index == 0 {
                "atlas.png".to_owned()
            } else {
                format!("atlas_{index}.png")
            };
            originals.push(
                load_image(&crate::assets::resource(&format!("assets/sprites/{name}")))
                    .await
                    .context("loading unit reduction source")?,
            );
        }
        let mut packer = Packer::new();
        let mut sprites = HashMap::new();
        for key in sources {
            let page = key[1] as usize / page_height as usize;
            let local = [key[0], key[1] % page_height as u32, key[2], key[3]];
            anyhow::ensure!(
                key[2] >= 8 && key[3] >= 8 && key[2] % 8 == 0 && key[3] % 8 == 0,
                "unit sprite dimensions must be multiples of eight"
            );
            anyhow::ensure!(
                key[2] as usize / 2 + 4 <= PAGE && key[3] as usize / 2 + 4 <= PAGE,
                "unit reduction exceeds texture page"
            );
            anyhow::ensure!(
                local[0] + local[2] <= u32::from(originals[page].width)
                    && local[1] + local[3] <= u32::from(originals[page].height),
                "unit sprite extends outside its atlas page"
            );
            let levels =
                LEVELS.map(|factor| packer.insert(&reduce(&originals[page], local, factor)));
            sprites.insert(key, levels);
        }
        let pages: Vec<_> = packer
            .images
            .iter()
            .map(|image| {
                let texture = Texture2D::from_image(image);
                texture.set_filter(FilterMode::Linear);
                texture
            })
            .collect();
        let materials = pages.iter().map(blend_material).collect::<Result<_>>()?;
        Ok(Self {
            pages,
            materials,
            sprites,
        })
    }

    pub(crate) fn draw(
        &self,
        position: Vec2,
        tint: Color,
        params: &DrawTextureParams,
        zoom: f32,
        originals: &[Texture2D],
        page_height: f32,
    ) -> bool {
        let (Some(source), Some(dest)) = (params.source, params.dest_size) else {
            return false;
        };
        let Some(levels) = self.sprites.get(&source_key(source)) else {
            return false;
        };
        let (low, high, blend) = lod_mix(source.w, dest.x * screen_dpi_scale(), zoom);
        if low == 0 && high == 0 {
            return false;
        }
        let region = |level: usize| {
            if level == 0 {
                let page = (source.y / page_height) as usize;
                (
                    &originals[page],
                    Rect::new(
                        source.x,
                        source.y - page as f32 * page_height,
                        source.w,
                        source.h,
                    ),
                )
            } else {
                let region = levels[level - 1];
                (&self.pages[region.page], region.rect)
            }
        };
        let (texture, rect) = region(low);
        if low != high {
            let (_, target) = region(high);
            let material = &self.materials[levels[high - 1].page];
            material.set_uniform(
                "UvTransform",
                [
                    target.w / PAGE as f32 / (rect.w / texture.width()),
                    target.h / PAGE as f32 / (rect.h / texture.height()),
                    (target.x - rect.x * target.w / rect.w) / PAGE as f32,
                    (target.y - rect.y * target.h / rect.h) / PAGE as f32,
                ],
            );
            material.set_uniform("Blend", blend);
            gl_use_material(material);
        }
        draw_texture_ex(
            texture,
            position.x,
            position.y,
            tint,
            DrawTextureParams {
                source: Some(rect),
                ..params.clone()
            },
        );
        if low != high {
            gl_use_default_material();
        }
        true
    }
}

fn blend_material(texture: &Texture2D) -> Result<Material> {
    use macroquad::miniquad::{
        BlendFactor, BlendState, BlendValue, Equation, PipelineParams, UniformDesc, UniformType,
    };
    let material = load_material(
        ShaderSource::Glsl {
            vertex: r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
uniform mat4 Model;
uniform mat4 Projection;
varying highp vec2 uv;
varying lowp vec4 color;
void main() {
    gl_Position = Projection * Model * vec4(position, 1.0);
    uv = texcoord;
    color = color0 / 255.0;
}"#,
            fragment: r#"#version 100
precision highp float;
varying highp vec2 uv;
varying lowp vec4 color;
uniform sampler2D Texture;
uniform sampler2D Reduced;
uniform vec4 UvTransform;
uniform float Blend;
void main() {
    vec4 a = texture2D(Texture, uv);
    vec4 b = texture2D(Reduced, uv * UvTransform.xy + UvTransform.zw);
    float alpha = mix(a.a, b.a, Blend);
    vec3 rgb = mix(a.rgb * a.a, b.rgb * b.a, Blend) / max(alpha, 0.00001);
    gl_FragColor = color * vec4(rgb, alpha);
}"#,
        },
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::SourceAlpha),
                    BlendFactor::OneMinusValue(BlendValue::SourceAlpha),
                )),
                ..Default::default()
            },
            uniforms: vec![
                UniformDesc::new("UvTransform", UniformType::Float4),
                UniformDesc::new("Blend", UniformType::Float1),
            ],
            textures: vec!["Reduced".into()],
        },
    )
    .context("loading unit LOD blend shader")?;
    // Macroquad snapshots uniforms per draw, but custom textures are pipeline
    // state. Each destination page therefore owns a material with a fixed binding.
    material.set_texture("Reduced", texture.clone());
    Ok(material)
}

struct Packer {
    images: Vec<Image>,
    x: usize,
    y: usize,
    row_height: usize,
}

impl Packer {
    fn new() -> Self {
        Self {
            images: vec![Image::gen_image_color(PAGE as u16, PAGE as u16, BLANK)],
            x: 2,
            y: 2,
            row_height: 0,
        }
    }

    fn insert(&mut self, image: &Image) -> Region {
        let (width, height) = (image.width as usize, image.height as usize);
        if self.x + width + 2 > PAGE {
            self.x = 2;
            self.y += self.row_height + 4;
            self.row_height = 0;
        }
        if self.y + height + 2 > PAGE {
            self.images
                .push(Image::gen_image_color(PAGE as u16, PAGE as u16, BLANK));
            self.x = 2;
            self.y = 2;
            self.row_height = 0;
        }
        let page = self.images.len() - 1;
        let output = &mut self.images[page];
        // Extrude each reduced sprite independently, including corners.
        for dy in -1..=height as isize {
            for dx in -1..=width as isize {
                let sx = dx.clamp(0, width as isize - 1) as usize;
                let sy = dy.clamp(0, height as isize - 1) as usize;
                let src = (sy * width + sx) * 4;
                let dst =
                    ((self.y as isize + dy) as usize * PAGE + (self.x as isize + dx) as usize) * 4;
                output.bytes[dst..dst + 4].copy_from_slice(&image.bytes[src..src + 4]);
            }
        }
        let region = Region {
            page,
            rect: Rect::new(self.x as f32, self.y as f32, width as f32, height as f32),
        };
        self.x += width + 4;
        self.row_height = self.row_height.max(height);
        region
    }
}

fn reduce(image: &Image, source: Source, factor: usize) -> Image {
    let [x, y, w, h] = source.map(|v| v as usize);
    let mut result = Image::gen_image_color((w / factor) as u16, (h / factor) as u16, BLANK);
    for oy in 0..h / factor {
        for ox in 0..w / factor {
            let mut color = [0_u32; 4];
            for sy in y + oy * factor..y + (oy + 1) * factor {
                for sx in x + ox * factor..x + (ox + 1) * factor {
                    let pixel = &image.bytes[(sy * image.width as usize + sx) * 4..][..4];
                    for channel in 0..3 {
                        color[channel] += u32::from(pixel[channel]) * u32::from(pixel[3]);
                    }
                    color[3] += u32::from(pixel[3]);
                }
            }
            let dst = &mut result.bytes[(oy * (w / factor) + ox) * 4..][..4];
            for channel in 0..3 {
                dst[channel] = color[channel].checked_div(color[3]).unwrap_or(0) as u8;
            }
            dst[3] = (color[3] / (factor * factor) as u32) as u8;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_resolution_selects_the_approved_levels() {
        let (low, high, mix) = lod_mix(128.0, 21.0, 20.0);
        assert_eq!((low, high), (2, 3));
        assert!(mix > 0.9 && mix < 1.0);
        let (low, high, mix) = lod_mix(128.0, 42.0, 20.0);
        assert_eq!((low, high), (1, 2));
        assert!(mix > 0.5 && mix < 1.0);
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            assert_eq!(lod_mix(128.0, 33.6 * dpi, 32.0), (0, 0, 0.0));
        }
    }

    #[test]
    fn zoom_and_level_boundaries_are_continuous_at_every_display_scale() {
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for size in [128.0, 256.0] {
                let weights = |zoom| {
                    let (low, high, blend) = lod_mix(size, zoom * 1.05 * dpi, zoom);
                    let mut weights = [0.0; 4];
                    weights[low] += 1.0 - blend;
                    weights[high] += blend;
                    weights
                };
                for step in 8000..33000 {
                    let zoom = step as f32 / 1000.0;
                    let a = weights(zoom);
                    let b = weights(zoom + 0.001);
                    assert!((a.iter().sum::<f32>() - 1.0).abs() < 1e-5);
                    for (a, b) in a.into_iter().zip(b) {
                        assert!((a - b).abs() < 0.01, "discontinuity at {zoom}, {dpi}x");
                    }
                }
            }
        }
    }

    #[test]
    fn packed_edges_and_corners_never_sample_neighbors() {
        let mut packer = Packer::new();
        let red = packer.insert(&Image::gen_image_color(2, 2, RED));
        packer.insert(&Image::gen_image_color(2, 2, BLUE));
        let page = &packer.images[red.page];
        for y in red.rect.y as u32 - 1..=red.rect.y as u32 + 2 {
            for x in red.rect.x as u32 - 1..=red.rect.x as u32 + 2 {
                assert_eq!(page.get_pixel(x, y), page.get_pixel(2, 2));
            }
        }
        packer.x = PAGE - 2;
        packer.y = PAGE - 2;
        let next = packer.insert(&Image::gen_image_color(2, 2, GREEN));
        assert_eq!(next.page, 1);
        assert_eq!(
            packer.images[1].get_pixel(1, 1),
            Image::gen_image_color(1, 1, GREEN).get_pixel(0, 0)
        );
    }

    #[test]
    fn reduction_preserves_alpha_coverage_without_transparent_color_bleed() {
        let image = Image {
            width: 2,
            height: 2,
            bytes: vec![255, 80, 10, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0],
        };
        let reduced = reduce(&image, [0, 0, 2, 2], 2);
        assert_eq!(reduced.bytes, [255, 80, 10, 63]);
    }

    #[test]
    fn reduction_never_reads_adjacent_atlas_sprites() {
        let mut image = Image::gen_image_color(4, 2, BLUE);
        for y in 0..2 {
            for x in 0..2 {
                image.set_pixel(x, y, Color::new(1.0, 0.0, 0.0, 1.0));
            }
        }
        assert_eq!(reduce(&image, [0, 0, 2, 2], 2).bytes, [255, 0, 0, 255]);
    }

    #[test]
    fn independent_hulls_mounts_and_accent_masks_enter_the_lod_bank() {
        let manifest: HashMap<String, [f32; 4]> =
            serde_json::from_str(include_str!("../../assets/sprites/atlas.json")).unwrap();
        for name in [
            "rig_sentinel_hull_ferrous",
            "rig_sentinel_mount_ferrous",
            "rig_sentinel_mount_accent",
            "harvester_ferrous_cargo0",
        ] {
            assert!(
                manifest.contains_key(name),
                "missing rendered source {name}"
            );
            assert!(is_unit_source(name), "source skipped by LOD bank: {name}");
        }
        assert!(!is_unit_source("rig_array_mount_ferrous"));
        assert!(is_unit_source("scout_radar"));
    }
}
