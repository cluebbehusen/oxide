//! Independent sprite reductions and premultiplied-alpha sampling.
use crate::numeric;
use crate::numeric::Fit;
use anyhow::{Context, Result};
use macroquad::prelude::*;
use std::cell::RefCell;
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
    contacts: HashMap<Source, crate::sprite_contact::SpriteContact>,
    mesh: RefCell<Mesh>,
}
fn source_key(rect: Rect) -> Source {
    [
        numeric::to_u32(rect.x),
        numeric::to_u32(rect.y),
        numeric::to_u32(rect.w),
        numeric::to_u32(rect.h),
    ]
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
        .chain(
            oxide_sim::BuildingKind::ALL
                .iter()
                .filter_map(|&k| crate::look::defense(k))
                .map(|defense| defense.mount),
        )
        .chain(["scrap", "wreck_pile", "extractor_frame", "scout_radar"])
        .any(|stem| {
            name == stem
                || name
                    .strip_prefix(stem)
                    .is_some_and(|rest| rest.starts_with('_'))
        })
}
fn entity_sources(manifest: &HashMap<String, [f32; 4]>) -> BTreeSet<Source> {
    manifest
        .iter()
        .filter(|(name, _)| {
            if !is_entity_source(name) {
                return false;
            }
            let Some((body, pose)) = name.rsplit_once('_') else {
                return true;
            };
            if !pose.starts_with("move") && !pose.starts_with("action") {
                return true;
            }
            let stem = body.strip_suffix("_accent").unwrap_or(body);
            // Layered units draw their hull and mount; only the complete idle
            // sprite is used, for portraits. Units without rig layers keep
            // their full poses.
            !manifest.contains_key(&format!("rig_{stem}_hull"))
        })
        .map(|(_, row)| row.map(numeric::to_u32))
        .collect()
}

fn lod_mix(source: Vec2, physical: Vec2) -> (usize, usize, f32) {
    let ratio = (source.x / physical.x.max(1.0)).max(source.y / physical.y.max(1.0));
    let lod = (ratio.log2() - 0.4).clamp(0.0, 3.0);
    (
        numeric::to_usize(lod.floor()),
        numeric::to_usize(lod.ceil()),
        lod.fract(),
    )
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
fn contact_sources(manifest: &HashMap<String, [f32; 4]>) -> BTreeSet<Source> {
    manifest
        .iter()
        .filter(|(name, _)| {
            (oxide_sim::UnitKind::ALL.iter().any(|&kind| {
                let stem = crate::assets::unit_stem(kind);
                // Only tinted rows: a stem's untinted extras, such as cargo
                // meters, carry no accent twin and take no contacts.
                [
                    stem.to_owned(),
                    format!("rig_{stem}_hull"),
                    format!("rig_{stem}_body"),
                ]
                .iter()
                .any(|prefix| {
                    name.strip_prefix(prefix.as_str()).is_some_and(|suffix| {
                        (suffix.is_empty() || suffix.starts_with('_'))
                            && manifest.contains_key(&format!("{prefix}_accent{suffix}"))
                    })
                })
            }) || name.starts_with("scrap_")
                || *name == "wreck_pile"
                || oxide_sim::BuildingKind::ALL.iter().any(|&kind| {
                    name.strip_prefix("rig_")
                        .unwrap_or(name)
                        .starts_with(&format!("{}_", crate::assets::building_stem(kind)))
                }))
                && !name.contains("_accent")
        })
        .map(|(_, row)| row.map(numeric::to_u32))
        .collect()
}
impl EntityLod {
    pub(crate) fn load(manifest: &HashMap<String, [f32; 4]>, page_height: f32) -> Result<Self> {
        let sources = entity_sources(manifest);
        let count = sources
            .iter()
            .map(|k| k[1] as usize / numeric::to_usize(page_height) + 1)
            .max()
            .unwrap_or(0);
        let mut originals = Vec::new();
        for index in 0..count {
            let name = if index == 0 {
                "atlas.png".to_owned()
            } else {
                format!("atlas_{index}.png")
            };
            originals.push(crate::assets::load_resource_image(&format!(
                "assets/sprites/{name}"
            ))?);
        }
        let mut packer = Packer::new();
        let mut sprites = HashMap::new();
        let mut bounds = HashMap::new();
        let contact_sources = contact_sources(manifest);
        let mut contacts = HashMap::new();
        for &key in &sources {
            let page = key[1] as usize / numeric::to_usize(page_height);
            let local = [
                key[0],
                key[1] % numeric::to_u32(page_height),
                key[2],
                key[3],
            ];
            anyhow::ensure!(
                key[2] >= 8 && key[3] >= 8 && key[2] % 8 == 0 && key[3] % 8 == 0,
                "sprite dimensions must be multiples of eight"
            );
            anyhow::ensure!(
                key[2] as usize + 4 <= PAGE && key[3] as usize + 4 <= PAGE,
                "sprite exceeds texture page"
            );
            anyhow::ensure!(
                local[0] + local[2] <= u32::from(originals[page].width)
                    && local[1] + local[3] <= u32::from(originals[page].height),
                "sprite exceeds source atlas page"
            );
            bounds.insert(key, opaque_bounds(&originals[page], local));
            if contact_sources.contains(&key) {
                contacts.insert(
                    key,
                    crate::sprite_contact::SpriteContact::capture(&originals[page], local),
                );
            }
        }
        for (key, level) in packing_order(&sources) {
            let page = key[1] as usize / numeric::to_usize(page_height);
            let local = [
                key[0],
                key[1] % numeric::to_u32(page_height),
                key[2],
                key[3],
            ];
            let region = packer.insert(&reduce(&originals[page], local, LEVELS[level]));
            sprites.entry(key).or_insert([region; 4])[level] = region;
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
            contacts,
            mesh: RefCell::new(Mesh {
                vertices: vec![Vertex::new(0.0, 0.0, 0.0, 0.0, 0.0, WHITE); 4],
                indices: vec![0, 1, 2, 0, 2, 3],
                texture: None,
            }),
        })
    }
    pub(crate) fn contact(
        &self,
        source: Rect,
        from: Vec2,
        aim: Vec2,
        origin: Vec2,
        size: Vec2,
    ) -> Option<Vec2> {
        self.contacts
            .get(&source_key(source))?
            .contact(from, aim, origin, size)
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
        // Keep the compatible material through intervening ordinary 2D draws.
        // The screen boundary restores the default material; per-sprite resets split batches.
        gl_use_material(&self.materials[b.page]);
        let mut mesh = self.mesh.borrow_mut();
        mesh.vertices.copy_from_slice(&sprite_vertices(
            position, tint, params, a.rect, b.rect, blend,
        ));
        mesh.texture = Some(self.pages[a.page].clone());
        draw_mesh(&mesh);

        true
    }
}
// Sort by shelf height first so small mip levels cannot waste full-size rows.
fn packing_order(sources: &BTreeSet<Source>) -> Vec<(Source, usize)> {
    let mut entries: Vec<_> = sources
        .iter()
        .flat_map(|&key| (0..4).map(move |level| (key, level)))
        .collect();
    entries.sort_by_key(|&(key, level)| {
        (
            std::cmp::Reverse(key[3] / LEVELS[level].fit::<u32>()),
            std::cmp::Reverse(key[2] / LEVELS[level].fit::<u32>()),
            key,
            level,
        )
    });
    entries
}

fn sprite_vertices(
    position: Vec2,
    tint: Color,
    params: &DrawTextureParams,
    a: Rect,
    b: Rect,
    blend: f32,
) -> [Vertex; 4] {
    let mut origin = position;
    let mut size = params.dest_size.unwrap();
    if params.flip_x {
        origin.x += size.x;
        size.x = -size.x;
    }
    if params.flip_y {
        origin.y += size.y;
        size.y = -size.y;
    }
    let pivot = params.pivot.unwrap_or(origin + size * 0.5);
    let (sin, cos) = params.rotation.sin_cos();
    [
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0),
    ]
    .map(|corner| {
        let offset = origin + corner * size - pivot;
        let pos = pivot
            + vec2(
                offset.x * cos - offset.y * sin,
                offset.x * sin + offset.y * cos,
            );
        let uv = (a.point() + corner * a.size()) / PAGE as f32;
        let reduced = (b.point() + corner * b.size()) / PAGE as f32;
        let mut vertex = Vertex::new(pos.x, pos.y, 0.0, uv.x, uv.y, tint);
        // Macroquad reserves normal for user data; ordinary 2D draws leave it zero.
        vertex.normal = vec4(reduced.x, reduced.y, blend, 1.0);
        vertex
    })
}

fn blend_material(texture: &Texture2D) -> Result<Material> {
    use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation, PipelineParams};
    let material = load_material(
        ShaderSource::Glsl {
            vertex: r"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;
uniform mat4 Model;
uniform mat4 Projection;
varying highp vec2 uv;
varying lowp vec4 color;
varying highp vec4 sampling;
void main(){ gl_Position=Projection*Model*vec4(position,1.0); uv=texcoord; color=color0/255.0; sampling=normal; }
",
            fragment: r"#version 100
precision highp float;
varying highp vec2 uv;
varying lowp vec4 color;
varying highp vec4 sampling;
uniform sampler2D Texture;
uniform sampler2D Reduced;
void main(){
    vec4 pixel=texture2D(Texture,uv);
    if (sampling.w > 0.5) {
        pixel=mix(pixel,texture2D(Reduced,sampling.xy),sampling.z);
    } else {
        pixel.rgb*=pixel.a;
    }
    gl_FragColor=vec4(pixel.rgb*color.rgb*color.a,pixel.a*color.a);
}
",
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
            uniforms: vec![],
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
            images: vec![Image::gen_image_color(
                PAGE.fit::<u16>(),
                PAGE.fit::<u16>(),
                BLANK,
            )],
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
            self.images.push(Image::gen_image_color(
                PAGE.fit::<u16>(),
                PAGE.fit::<u16>(),
                BLANK,
            ));
            self.x = 2;
            self.y = 2;
            self.row_height = 0;
        }
        let page = self.images.len() - 1;
        let output = &mut self.images[page];
        // Extrude each reduced sprite independently, including corners.
        for dy in -1..=height.fit::<isize>() {
            for dx in -1..=width.fit::<isize>() {
                let sx = dx.clamp(0, width.fit::<isize>() - 1).fit::<usize>();
                let sy = dy.clamp(0, height.fit::<isize>() - 1).fit::<usize>();
                let src = (sy * width + sx) * 4;
                let dst = ((self.y.fit::<isize>() + dy).fit::<usize>() * PAGE
                    + (self.x.fit::<isize>() + dx).fit::<usize>())
                    * 4;
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
    let mut result =
        Image::gen_image_color((w / factor).fit::<u16>(), (h / factor).fit::<u16>(), BLANK);
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
                dst[channel] = (sum[channel] / (factor * factor * 255).fit::<u32>()).fit::<u8>();
            }
            dst[3] = (sum[3] / (factor * factor).fit::<u32>()).fit::<u8>();
        }
    }
    result
}

#[cfg(test)]
mod tests;
