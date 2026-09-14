//! Independent sprite reductions and premultiplied-alpha sampling.
use anyhow::{Context, Result};
use macroquad::prelude::*;
use std::collections::{BTreeSet, HashMap};
const PAGE: usize = 2048;
const LEVELS: [usize; 4] = [1, 2, 4, 8];
type Source = [u32; 4];
#[derive(Debug, Clone, Copy)]
struct Region {
    page: usize,
    rect: Rect,
}
pub(crate) struct EntityLod {
    pages: Vec<Texture2D>,
    materials: Vec<Material>,
    sprites: HashMap<Source, [Region; 4]>,
    bounds: HashMap<Source, Rect>,
}
fn source_key(rect: Rect) -> Source {
    [rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32]
}
fn is_entity_source(name: &str) -> bool {
    let name = name.strip_prefix("rig_").unwrap_or(name);
    oxide_sim::UnitKind::ALL
        .iter()
        .map(|&k| crate::assets::unit_stem(k))
        .chain(
            oxide_sim::BuildingKind::ALL
                .iter()
                .map(|&k| crate::assets::building_stem(k)),
        )
        .chain([
            "flak_mount",
            "scrap",
            "wreck_pile",
            "extractor_frame",
            "scout_radar",
        ])
        .any(|stem| {
            name == stem
                || name
                    .strip_prefix(stem)
                    .is_some_and(|rest| rest.starts_with('_'))
        })
}
fn lod_mix(source: Vec2, physical: Vec2) -> (usize, usize, f32) {
    let ratio = (source.x / physical.x.max(1.0)).max(source.y / physical.y.max(1.0));
    let lod = (ratio.log2() - 0.4).clamp(0.0, 3.0);
    (lod.floor() as usize, lod.ceil() as usize, lod.fract())
}
fn opaque_bounds(image: &Image, source: Source) -> Rect {
    let [x, y, w, h] = source;
    let (mut left, mut top, mut right, mut bottom) = (w, h, 0, 0);
    for sy in 0..h {
        for sx in 0..w {
            if image.bytes[(((y + sy) * u32::from(image.width) + x + sx) * 4 + 3) as usize] > 8 {
                left = left.min(sx);
                top = top.min(sy);
                right = right.max(sx + 1);
                bottom = bottom.max(sy + 1);
            }
        }
    }
    if right == 0 {
        return Rect::new(0.0, 0.0, 1.0, 1.0);
    }
    Rect::new(
        left as f32 / w as f32,
        top as f32 / h as f32,
        (right - left) as f32 / w as f32,
        (bottom - top) as f32 / h as f32,
    )
}
impl EntityLod {
    pub(crate) async fn load(
        manifest: &HashMap<String, [f32; 4]>,
        page_height: f32,
    ) -> Result<Self> {
        let sources: BTreeSet<Source> = manifest
            .iter()
            .filter(|(name, _)| is_entity_source(name))
            .map(|(_, row)| row.map(|v| v as u32))
            .collect();
        let count = sources
            .iter()
            .map(|k| k[1] as usize / page_height as usize + 1)
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
                load_image(&crate::assets::resource(&format!("assets/sprites/{name}"))).await?,
            );
        }
        let mut packer = Packer::new();
        let mut sprites = HashMap::new();
        let mut bounds = HashMap::new();
        for key in sources {
            let page = key[1] as usize / page_height as usize;
            let local = [key[0], key[1] % page_height as u32, key[2], key[3]];
            anyhow::ensure!(
                key[2] >= 8 && key[3] >= 8 && key[2] % 8 == 0 && key[3] % 8 == 0,
                "sprite dimensions must be multiples of eight"
            );
            anyhow::ensure!(
                key[2] as usize + 4 <= PAGE && key[3] as usize + 4 <= PAGE,
                "sprite exceeds texture page"
            );
            bounds.insert(key, opaque_bounds(&originals[page], local));
            sprites.insert(
                key,
                LEVELS.map(|factor| packer.insert(&reduce(&originals[page], local, factor))),
            );
        }
        let pages: Vec<_> = packer
            .images
            .iter()
            .map(|image| {
                let t = Texture2D::from_image(image);
                t.set_filter(FilterMode::Linear);
                t
            })
            .collect();
        let materials = pages.iter().map(blend_material).collect::<Result<_>>()?;
        Ok(Self {
            pages,
            materials,
            sprites,
            bounds,
        })
    }
    pub(crate) fn bounds(&self, source: Rect) -> Rect {
        self.bounds
            .get(&source_key(source))
            .copied()
            .unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0))
    }
    pub(crate) fn draw(&self, position: Vec2, tint: Color, params: &DrawTextureParams) -> bool {
        let (Some(source), Some(dest)) = (params.source, params.dest_size) else {
            return false;
        };
        let Some(levels) = self.sprites.get(&source_key(source)) else {
            return false;
        };
        let (low, high, blend) = lod_mix(vec2(source.w, source.h), dest * screen_dpi_scale());
        let a = levels[low];
        let b = levels[high];
        let material = &self.materials[b.page];
        material.set_uniform(
            "UvTransform",
            [
                b.rect.w / a.rect.w,
                b.rect.h / a.rect.h,
                (b.rect.x - a.rect.x * b.rect.w / a.rect.w) / PAGE as f32,
                (b.rect.y - a.rect.y * b.rect.h / a.rect.h) / PAGE as f32,
            ],
        );
        material.set_uniform("Blend", blend);
        gl_use_material(material);
        draw_texture_ex(
            &self.pages[a.page],
            position.x,
            position.y,
            tint,
            DrawTextureParams {
                source: Some(a.rect),
                ..params.clone()
            },
        );
        gl_use_default_material();
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
void main(){ gl_Position=Projection*Model*vec4(position,1.0); uv=texcoord; color=color0/255.0; }
"#,
            fragment: r#"#version 100
precision highp float;
varying highp vec2 uv;
varying lowp vec4 color;
uniform sampler2D Texture;
uniform sampler2D Reduced;
uniform vec4 UvTransform;
uniform float Blend;
void main(){
    vec4 pixel=mix(texture2D(Texture,uv),texture2D(Reduced,uv*UvTransform.xy+UvTransform.zw),Blend);
    gl_FragColor=vec4(pixel.rgb*color.rgb*color.a,pixel.a*color.a);
}
"#,
        },
        MaterialParams {
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::One,
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
    .context("loading sprite reduction shader")?;
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
            let mut sum = [0_u32; 4];
            for sy in y + oy * factor..y + (oy + 1) * factor {
                for sx in x + ox * factor..x + (ox + 1) * factor {
                    let pixel = &image.bytes[(sy * image.width as usize + sx) * 4..][..4];
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixel[channel]) * u32::from(pixel[3]);
                    }
                    sum[3] += u32::from(pixel[3]);
                }
            }
            let dst = &mut result.bytes[(oy * (w / factor) + ox) * 4..][..4];
            for channel in 0..3 {
                dst[channel] = (sum[channel] / (factor * factor * 255) as u32) as u8;
            }
            dst[3] = (sum[3] / (factor * factor) as u32) as u8;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
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
        for width in 8000..129000 {
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
            "foundry_ferrous",
            "flak_turret_ferrous",
            "flak_mount_accent",
            "rig_array_t1_rotor_cupric",
            "repair_bay_ferrous",
            "scuttle_charge_accent",
            "scrap_full",
            "wreck_pile",
            "harvester_ferrous_cargo0",
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
        for y in red.rect.y as u32 - 1..=red.rect.y as u32 + 2 {
            for x in red.rect.x as u32 - 1..=red.rect.x as u32 + 2 {
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
}
