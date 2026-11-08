//! Mesh records turned into ordinary vertex arrays.
//!
//! The engine keeps geometry in storage buffers and its shaders pull faces out of them by index,
//! which needs bindless sampling and indirect draws - neither of which a FL10_0 or GL 2.1 GPU has.
//! A legacy backend already holds every byte those buffers were given, so it decodes them here
//! into attributes a fixed vertex layout can carry. This is the one copy of that decode: the
//! numbers below must match `crates/engine/src/gpu_layout.rs` and `world.wgsl`, and
//! `gpu_layout::legacy_decode_matches_the_shader` checks them against the engine's own writer.

use weft_abi::FACE_CORNERS;

use crate::atlas::Rect;

/// Face record word 0: x, y, z in the cube, direction, corner AO, tint kind.
pub mod bits {
    pub const POS_MASK: u32 = 15;
    pub const Y_SHIFT: u32 = 4;
    pub const Z_SHIFT: u32 = 8;
    pub const DIR_SHIFT: u32 = 12;
    pub const DIR_MASK: u32 = 7;
    pub const AO_SHIFT: u32 = 16;
    pub const AO_MASK: u32 = 3;
    pub const TINT_SHIFT: u32 = 24;
    pub const TINT_MASK: u32 = 15;
    pub const TEX_MASK: u32 = 0xFFFF;
    pub const BIOME_SHIFT: u32 = 16;
    pub const LIGHT_MASK: u32 = 15;
    pub const CORNER_SHIFT: u32 = 16;
}

/// Quad record words: four corners, then texture and biome, uv and tint, then the light pair.
pub mod quad_bits {
    pub const POS_MASK: u32 = 1023;
    pub const POS_BITS: u32 = 10;
    pub const POS_OFFSET: u32 = 32;
    pub const UV_MASK: u32 = 31;
    pub const UV_BITS: u32 = 5;
    pub const UV_FINE_BITS: u32 = 3;
    pub const UV_FINE_LOW_SHIFT: u32 = 30;
    pub const UV_FINE_HIGH_SHIFT: u32 = 20;
    pub const TINT_SHIFT: u32 = 24;
    pub const FLAT: u32 = 16;
    pub const WORDS: usize = 8;
}

/// Group 1 of `world.wgsl`, in the order `gpu_layout::bindings!` numbers it: what the engine binds
/// and what a legacy backend has to pull out of the bytes behind each binding.
pub mod world_bind {
    pub const G: u32 = 0;
    pub const ORIGINS: u32 = 1;
    pub const RECORDS: u32 = 2;
    pub const BIOMES: u32 = 3;
    pub const TEX_INFO: u32 = 4;
    pub const FRAMES: u32 = 5;
    pub const OFFSETS: u32 = 6;
    pub const GLOW_MAP: u32 = 7;
    pub const TEX_SHADER: u32 = 8;
    pub const SCENE: u32 = 9;
    pub const LOD_MASK: u32 = 10;
    pub const SOLID: u32 = 11;
}

/// Byte offsets inside the two `world.wgsl` uniform records, whose fields are all 16 bytes.
pub mod globals {
    pub const CAM_FRAC: usize = 0;
    pub const IDS: usize = 16;
    pub const CAM_CUBE: usize = 32;
    pub const EXTRA: usize = 48;
    pub const SIZE: usize = 64;
}

pub mod scene {
    pub const VIEW_PROJ: usize = 0;
    pub const FOG_COLOR: usize = 96;
    pub const PARAMS: usize = 112;
    pub const LIGHT: usize = 128;
    pub const SIZE: usize = 160;
}

use bits::*;

/// Corner order of a face's picture (`UVS`).
pub const UVS: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
/// How much each direction is lit, -X +X -Y +Y -Z +Z (`SHADE`).
pub const SHADE: [f32; 6] = [0.6, 0.6, 0.5, 1.0, 0.8, 0.8];
/// Ambient occlusion steps, darkest to none (`AO`).
pub const AO: [f32; 4] = [0.42, 0.62, 0.8, 1.0];

/// Floats per vertex, in the order a legacy shader declares its attributes: position, uv, colour,
/// and the four light channels.
pub const F32S_PER_VERTEX: usize = 12;

/// Everything about one corner of a face that a fixed pipeline needs, read the way `vs_face` does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    /// Where it is inside the cube, in blocks, before the region origin is added.
    pub local: [f32; 3],
    /// Face direction, the index into `weft_abi::FACE_DIRS`.
    pub dir: usize,
    /// The picture's corner inside the texture, 0..1.
    pub uv: [f32; 2],
    /// Texture id from the record, before animation and overlays.
    pub texture: u32,
    pub biome: u32,
    pub tint: u32,
    /// Direction shade times the corner's AO; multiply by the tint colour for the vertex colour.
    pub shade: f32,
    /// Four light channels, 0..1 (`corner_light`).
    pub light: [f32; 4],
    /// The block corner, which is where the tint table is looked up.
    pub block: [i32; 3],
}

/// One face record's four corners, in the order `vs_face` visits them: the AO-determined diagonal
/// flip is applied, so the caller emits indices 0,1,2,0,2,3 over them.
pub fn corners(record: &[u32; 4]) -> [Corner; 4] {
    let (w0, w1) = (record[0], record[1]);
    let pos = [w0 & POS_MASK, (w0 >> Y_SHIFT) & POS_MASK, (w0 >> Z_SHIFT) & POS_MASK];
    let dir = ((w0 >> DIR_SHIFT) & DIR_MASK) as usize;
    let ao = w0 >> AO_SHIFT;
    let a = [ao & AO_MASK, (ao >> 2) & AO_MASK, (ao >> 4) & AO_MASK, (ao >> 6) & AO_MASK];
    let flip = a[0] + a[2] < a[1] + a[3];
    let (texture, biome) = (w1 & TEX_MASK, w1 >> BIOME_SHIFT);
    let tint = (w0 >> TINT_SHIFT) & TINT_MASK;
    std::array::from_fn(|i| {
        let c = (i as u32 + if flip { 1 } else { 0 }) % 4;
        let corner = FACE_CORNERS[dir][c as usize];
        Corner {
            local: [
                pos[0] as f32 + corner[0] as f32,
                pos[1] as f32 + corner[1] as f32,
                pos[2] as f32 + corner[2] as f32,
            ],
            dir,
            uv: UVS[c as usize],
            texture,
            biome,
            tint,
            shade: SHADE[dir] * AO[a[c as usize] as usize],
            light: corner_light(record[2], record[3], c),
            block: [pos[0] as i32 + corner[0], pos[1] as i32 + corner[1], pos[2] as i32 + corner[2]],
        }
    })
}

/// A record at word offset `at` of a face page, or none past its end.
pub fn record_at(bytes: &[u8], at: usize) -> Option<[u32; 4]> {
    let start = at.checked_mul(16)?;
    let word = |n: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(start + n * 4..start + n * 4 + 4)?.try_into().ok()?)) };
    Some([word(0)?, word(1)?, word(2)?, word(3)?])
}

/// A quad record at word offset `at` (eight words), or none past its end.
pub fn quad_record_at(bytes: &[u8], at: usize) -> Option<[u32; quad_bits::WORDS]> {
    let start = at.checked_mul(quad_bits::WORDS * 4)?;
    let word = |n: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(start + n * 4..start + n * 4 + 4)?.try_into().ok()?)) };
    let mut record = [0u32; quad_bits::WORDS];
    for (i, w) in record.iter_mut().enumerate() {
        *w = word(i)?;
    }
    Some(record)
}

/// One quad corner, in blocks from the cube's corner (`quad_pos`).
fn quad_pos(w: u32) -> [f32; 3] {
    std::array::from_fn(|a| {
        let p = (w >> (a as u32 * quad_bits::POS_BITS)) & quad_bits::POS_MASK;
        (p as f32 - quad_bits::POS_OFFSET as f32) / 16.0
    })
}

/// `u0, v0, u1, v1` across the texture, to its 1/8th (`quad_uv_fine`).
fn quad_uv(w: u32, corners: &[u32; 4]) -> [f32; 4] {
    let s = quad_bits::UV_FINE_LOW_SHIFT;
    let f = (corners[0] >> s)
        | (corners[1] >> s) << 2
        | (corners[2] >> s) << 4
        | (corners[3] >> s) << 6
        | ((w >> quad_bits::UV_FINE_HIGH_SHIFT) & 15) << 8;
    std::array::from_fn(|i| {
        let whole = ((w >> (i as u32 * quad_bits::UV_BITS)) & quad_bits::UV_MASK) as f32;
        let fine = ((f >> (i as u32 * quad_bits::UV_FINE_BITS)) & 7) as f32;
        (whole + fine / 8.0) / 16.0
    })
}

/// What `vs_quad` makes of a record: its four corners, with the shade its normal gives and no
/// direction or AO (the normal already carries both).
pub fn quad_corners(record: &[u32; quad_bits::WORDS]) -> [Corner; 4] {
    let p = std::array::from_fn::<[f32; 3], 4, _>(|i| quad_pos(record[i]));
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let (e1, e2) = (sub(p[1], p[0]), sub(p[2], p[1]));
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let long = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = if long > 0.0 { [n[0] / long, n[1] / long, n[2] / long] } else { [0.0; 3] };
    let flat = (record[5] >> quad_bits::TINT_SHIFT) & quad_bits::FLAT;
    let mut shade = n[0] * n[0] * 0.6 + n[2] * n[2] * 0.8 + n[1] * n[1] * if n[1] > 0.0 { 1.0 } else { 0.5 };
    if flat != 0 {
        shade = 1.0;
    }
    let uv = quad_uv(record[5], &[record[0], record[1], record[2], record[3]]);
    let uvs = [[uv[0], uv[3]], [uv[2], uv[3]], [uv[2], uv[1]], [uv[0], uv[1]]];
    let (texture, biome) = (record[4] & TEX_MASK, record[4] >> BIOME_SHIFT);
    let tint = (record[5] >> TINT_SHIFT) & TINT_MASK;
    std::array::from_fn(|i| Corner {
        local: p[i],
        dir: 0,
        uv: uvs[i],
        texture,
        biome,
        tint,
        shade,
        light: corner_light(record[6], record[7], i as u32),
        block: [p[i][0].round() as i32, p[i][1].round() as i32, p[i][2].round() as i32],
    })
}

/// Corner `c`'s light from a record's two light words, the channels 0..1 (`corner_light`).
fn corner_light(lo: u32, hi: u32, c: u32) -> [f32; 4] {
    let half = if c >= 2 { hi } else { lo };
    let l = (half >> ((c & 1) * CORNER_SHIFT)) & 0xFFFF;
    std::array::from_fn(|k| ((l >> (k * 4)) & LIGHT_MASK) as f32 / 15.0)
}

/// One region's place: cube coordinates relative to the camera's cube, and what body it rides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub cube: [i32; 3],
    /// 0: the world's own light. Otherwise the entry in `Offset` of the structure it belongs to.
    pub body: i32,
}

/// `origins` is a `vec4<i32>` per region: xyz the cube, w the body.
pub fn origin_at(bytes: &[u8], at: usize) -> Option<Origin> {
    let start = at.checked_mul(16)?;
    let word = |n: usize| -> Option<i32> { Some(i32::from_le_bytes(bytes.get(start + n * 4..start + n * 4 + 4)?.try_into().ok()?)) };
    Some(Origin { cube: [word(0)?, word(1)?, word(2)?], body: word(3)? })
}

/// A moving structure: where its cubes are, and the world's light where it stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offset {
    pub at: [f32; 3],
    /// Packed light, the way `unpack_light` reads it.
    pub light: u32,
}

/// `offsets` is a `vec4<f32>` per structure, its w the light as bits.
pub fn offset_at(bytes: &[u8], at: usize) -> Option<Offset> {
    let start = at.checked_mul(16)?;
    let bits = |n: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(start + n * 4..start + n * 4 + 4)?.try_into().ok()?)) };
    Some(Offset { at: [f32::from_bits(bits(0)?), f32::from_bits(bits(1)?), f32::from_bits(bits(2)?)], light: bits(3)? })
}

/// Light 0..1 of a packed word, four channels of up to 15 each.
pub fn unpack_light(packed: u32) -> [f32; 4] {
    std::array::from_fn(|k| ((packed >> (k * 8)) & 255).min(15) as f32 / 15.0)
}

/// What `on_body` makes of a corner's light where the region stands on a moving structure: its
/// own sky light capped by the world's, and the world's block light kept where it is brighter.
pub fn merge_light(corner: [f32; 4], body: [f32; 4]) -> [f32; 4] {
    [corner[0].min(body[0]), corner[1].max(body[1]), corner[2].max(body[2]), corner[3].max(body[3])]
}

/// One `DrawIndexedArgs`, which the engine writes on the CPU every frame - so a legacy backend
/// reads it there instead of asking a GPU that cannot draw indirectly to do so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Faces in the run: the engine keeps six indices per face.
    pub faces: u32,
    /// Record index in the page; four vertices form a record of either kind.
    pub records: usize,
    /// Which region's origin the run uses.
    pub origin: usize,
}

pub fn draw_at(bytes: &[u8], at: usize) -> Option<Draw> {
    let start = at.checked_mul(20)?;
    let word = |n: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(start + n * 4..start + n * 4 + 4)?.try_into().ok()?)) };
    let (indices, offset) = (word(0)?, word(3)? as i32);
    if offset < 0 {
        return None;
    }
    Some(Draw { faces: indices / 6, records: offset as usize / 4, origin: word(4)? as usize })
}

/// Mixes one word into a content hash (the 64-bit FNV prime). A legacy backend keys its
/// decoded-geometry cache on the bytes a run reads, so a steady view decodes and uploads nothing.
pub fn mix(h: u64, w: u64) -> u64 {
    (h ^ w).wrapping_mul(0x1000_0000_01b3)
}

/// Hashes a byte range a run reads, eight bytes at a time.
pub fn hash_bytes(mut h: u64, bytes: &[u8]) -> u64 {
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        h = mix(h, u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    for &b in chunks.remainder() {
        h = mix(h, b as u64);
    }
    h
}

/// What a legacy backend has to add from its own tables.
pub trait Resolve {
    /// Where a texture id's picture is now - animation included - or none if it has no room.
    fn rect(&mut self, texture: u32) -> Option<Rect>;

    /// The tint colour of a face corner (`tint_color` in the shader). `block` is the absolute
    /// block the corner sits at - the record's cube and the camera's cube, which is how the shader
    /// addresses the biome's blended column colours.
    fn tint(&mut self, kind: u32, biome: u32, block: [i32; 3]) -> [f32; 3];
}

/// Where the camera is, both halves of it as `world.wgsl` gets them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cam {
    /// The cube the origins are counted from (`g.cam_cube`).
    pub cube: [i32; 3],
    /// The camera's place inside that cube, in blocks (`g.cam_frac`).
    pub frac: [f32; 3],
}

/// What one run of a page holds: block faces, or free quads. The two differ in words per record
/// and in where their shade comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Faces,
    Quads,
}

/// Faces' worth of attributes and the indices that join them into triangles.
#[derive(Default)]
pub struct Geometry {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl Geometry {
    /// Room for `faces` faces, kept from the previous frame so a steady view stops allocating.
    pub fn with_capacity(faces: usize) -> Geometry {
        Geometry { vertices: Vec::with_capacity(faces * 4 * F32S_PER_VERTEX), indices: Vec::with_capacity(faces * 6) }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    /// Appends one face, as `vs_face` draws it: indices 0,1,2,0,2,3 over its four corners.
    pub fn push_face(&mut self, record: &[u32; 4], origin: Origin, offset: Option<Offset>, cam: Cam, resolve: &mut dyn Resolve) {
        self.push_corners(corners(record), origin, offset, cam, resolve);
    }

    /// Appends one quad, as `vs_quad` draws it.
    pub fn push_quad(&mut self, record: &[u32; 8], origin: Origin, offset: Option<Offset>, cam: Cam, resolve: &mut dyn Resolve) {
        self.push_corners(quad_corners(record), origin, offset, cam, resolve);
    }

    fn push_corners(&mut self, corners: [Corner; 4], origin: Origin, offset: Option<Offset>, cam: Cam, resolve: &mut dyn Resolve) {
        let base = (self.vertices.len() / F32S_PER_VERTEX) as u32;
        for corner in corners {
            self.push_corner(&corner, origin, offset, cam, resolve);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Appends one corner: where it is, what it samples, and the colour and light it carries.
    fn push_corner(&mut self, corner: &Corner, origin: Origin, offset: Option<Offset>, cam: Cam, resolve: &mut dyn Resolve) {
        let body = offset.map(|o| unpack_light(o.light));
        let light = match (origin.body, body) {
            (0, _) | (_, None) => corner.light,
            (_, Some(body)) => merge_light(corner.light, body),
        };
        let tint = if corner.tint == 0 {
            [1.0; 3]
        } else {
            let block = std::array::from_fn(|a| (origin.cube[a] + cam.cube[a]) * 16 + corner.block[a]);
            resolve.tint(corner.tint, corner.biome, block)
        };
        let uv = match resolve.rect(corner.texture) {
            Some(rect) => {
                let (u, v) = rect.uv(corner.uv[0], corner.uv[1]);
                [u, v]
            }
            // Nothing packed for it: the middle of a texture would be some other block's
            // colours, so a flat spot is the honest answer, and `rect` says why in the log.
            None => [0.5, 0.5],
        };
        // `finish` in the shader: the cube's corner, the corner inside it, less where the camera
        // sits in its own cube, plus whatever body this region rides on.
        let put = |axis: usize, local: f32| {
            origin.cube[axis] as f32 * 16.0 + local - cam.frac[axis] + offset.map_or(0.0, |o| o.at[axis])
        };
        let pos = [put(0, corner.local[0]), put(1, corner.local[1]), put(2, corner.local[2])];
        self.vertices.extend_from_slice(&[
            pos[0], pos[1], pos[2], uv[0], uv[1], tint[0] * corner.shade, tint[1] * corner.shade, tint[2] * corner.shade,
            light[0], light[1], light[2], light[3],
        ]);
    }

    /// Every record of one run. Records or origins beyond the bytes stop the run rather than
    /// wrapping: half a chunk somewhere else in the world is worse than the last faces missing.
    pub fn push_run(
        &mut self,
        kind: Kind,
        draw: Draw,
        records: &[u8],
        origins: &[u8],
        offsets: &[u8],
        cam: Cam,
        resolve: &mut dyn Resolve,
    ) {
        let Some(origin) = origin_at(origins, draw.origin) else { return };
        let offset = if origin.body == 0 { None } else { offset_at(offsets, origin.body as usize) };
        for f in 0..draw.faces {
            let at = draw.records + f as usize;
            match kind {
                Kind::Faces => {
                    let Some(record) = record_at(records, at) else { return };
                    self.push_face(&record, origin, offset, cam, resolve);
                }
                Kind::Quads => {
                    let Some(record) = quad_record_at(records, at) else { return };
                    self.push_quad(&record, origin, offset, cam, resolve);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet of 16x16 textures with no tint table at all.
    struct Flat;
    impl Resolve for Flat {
        fn rect(&mut self, texture: u32) -> Option<Rect> {
            (texture < 8).then_some(Rect { x: 0, y: 0, width: 16, height: 16, sheet: 16 })
        }
        fn tint(&mut self, _kind: u32, _biome: u32, _block: [i32; 3]) -> [f32; 3] {
            [1.0; 3]
        }
    }

    /// As the engine writes it: `light` is one packed 16-bit word per corner, four channels each.
    fn record(pos: [u32; 3], dir: u32, ao: [u32; 4], tint: u32, tex: u32, biome: u32, light: [u32; 4]) -> [u32; 4] {
        let w0 = pos[0]
            | pos[1] << Y_SHIFT
            | pos[2] << Z_SHIFT
            | dir << DIR_SHIFT
            | ao.iter().enumerate().fold(0, |w, (i, &v)| w | v << (AO_SHIFT + 2 * i as u32))
            | tint << TINT_SHIFT;
        [w0, tex | biome << BIOME_SHIFT, light[0] | light[1] << CORNER_SHIFT, light[2] | light[3] << CORNER_SHIFT]
    }

    fn bytes_of(words: &[u32]) -> Vec<u8> {
        let mut v = Vec::new();
        for w in words {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    /// As the engine writes a quad: corners in 1/16ths of a block from -2, texture, uv in whole
    /// 1/16ths beside the tint byte, then the light pair.
    fn quad(pos: [[f32; 3]; 4], tex: u32, uv: [f32; 4], tint: u32, flat: bool, light: [u32; 4]) -> [u32; 8] {
        let put = |p: [f32; 3]| {
            let e = |v: f32| ((v * 16.0).round() as i32 + quad_bits::POS_OFFSET as i32).clamp(0, quad_bits::POS_MASK as i32) as u32;
            e(p[0]) | e(p[1]) << quad_bits::POS_BITS | e(p[2]) << (2 * quad_bits::POS_BITS)
        };
        let w = uv.map(|v| (v * 16.0).round() as u32 & quad_bits::UV_MASK);
        [
            put(pos[0]),
            put(pos[1]),
            put(pos[2]),
            put(pos[3]),
            tex,
            w[0] | w[1] << quad_bits::UV_BITS | w[2] << (2 * quad_bits::UV_BITS) | w[3] << (3 * quad_bits::UV_BITS)
                | (tint | if flat { quad_bits::FLAT } else { 0 }) << quad_bits::TINT_SHIFT,
            light[0] | light[1] << CORNER_SHIFT,
            light[2] | light[3] << CORNER_SHIFT,
        ]
    }

    #[test]
    fn a_face_reads_back_what_was_written_into_it() {
        let r = record([3, 14, 9], 4, [3, 3, 3, 3], 7, 1234, 11, [0x4321, 0, 0, 0xFFFF]);
        let c = corners(&r);
        assert_eq!(c[0].dir, 4);
        assert_eq!(c[0].texture, 1234);
        assert_eq!(c[0].biome, 11);
        assert_eq!(c[0].tint, 7);
        let first = FACE_CORNERS[4][0];
        assert_eq!(c[0].local, [3.0 + first[0] as f32, 14.0 + first[1] as f32, 9.0 + first[2] as f32]);
        assert_eq!(c[0].light, [1.0 / 15.0, 2.0 / 15.0, 3.0 / 15.0, 4.0 / 15.0]);
        assert_eq!(c[3].light, [1.0, 1.0, 1.0, 1.0], "the third corner is the high word of the second light word");
    }

    #[test]
    fn ao_flips_the_diagonal_the_way_the_shader_does() {
        let straight = corners(&record([0, 0, 0], 5, [3, 0, 3, 0], 0, 1, 0, [0; 4]));
        assert_eq!(straight[0].uv, UVS[0], "a0 + a2 is not below a1 + a3 here");
        let flipped = corners(&record([0, 0, 0], 5, [0, 3, 0, 3], 0, 1, 0, [0; 4]));
        assert_eq!(flipped[0].uv, UVS[1], "a flipped face starts one corner later");
        assert_eq!(flipped[3].uv, UVS[0], "and the fourth corner wraps to the first");
    }

    #[test]
    fn shade_is_the_direction_times_the_corners_ao() {
        let c = corners(&record([0, 0, 0], 3, [0, 3, 0, 3], 0, 1, 0, [0; 4]));
        assert_eq!(c[0].shade, SHADE[3] * AO[3], "corner 0 of a flipped face is the record's corner 1");
        assert_eq!(c[3].shade, SHADE[3] * AO[0]);
        let down = corners(&record([0, 0, 0], 2, [0, 0, 0, 0], 0, 1, 0, [0; 4]));
        assert_eq!(down[0].shade, SHADE[2] * AO[0], "the darkest corner of the darkest direction");
    }

    #[test]
    fn buffer_bytes_are_little_endian_words() {
        let page = bytes_of(&record([1, 2, 3], 1, [1, 1, 1, 1], 0, 42, 3, [5, 6, 7, 8]));
        assert_eq!(record_at(&page, 0), Some(record([1, 2, 3], 1, [1, 1, 1, 1], 0, 42, 3, [5, 6, 7, 8])));
        assert_eq!(record_at(&page, 1), None, "one record is not two");
        let origins = bytes_of(&[(-4i32) as u32, 3, 17, 2]);
        assert_eq!(origin_at(&origins, 0), Some(Origin { cube: [-4, 3, 17], body: 2 }));
        assert_eq!(origin_at(&origins, 1), None);
        let mut offsets = Vec::new();
        for f in [1.5f32, -2.0, 0.0] {
            offsets.extend_from_slice(&f.to_le_bytes());
        }
        offsets.extend_from_slice(&0x0000_0F00u32.to_le_bytes());
        let o = offset_at(&offsets, 0).unwrap();
        assert_eq!((o.at, o.light), ([1.5, -2.0, 0.0], 0x0F00));
    }

    #[test]
    fn draw_args_carry_faces_records_and_the_region() {
        let args = bytes_of(&[12 * 6, 1, 0, 7 * 4, 5, 0]);
        assert_eq!(draw_at(&args, 0), Some(Draw { faces: 12, records: 7, origin: 5 }));
        assert_eq!(draw_at(&args, 1), None, "a second run needs twenty more bytes");
        assert_eq!(draw_at(&bytes_of(&[6, 1, 0, (-4i32) as u32, 0]), 0), None, "a negative record start");
    }

    #[test]
    fn a_run_puts_the_region_where_the_shader_puts_it() {
        let records = bytes_of(&record([0, 0, 0], 3, [3, 3, 3, 3], 0, 1, 0, [0xFFFF; 4]));
        let origins = bytes_of(&[2u32, (-1i32) as u32, 4, 0]);
        let mut g = Geometry::default();
        g.push_run(
            Kind::Faces,
            Draw { faces: 1, records: 0, origin: 0 },
            &records,
            &origins,
            &[],
            Cam { cube: [0; 3], frac: [0.5, 0.5, 0.5] },
            &mut Flat,
        );
        assert_eq!(g.vertices.len(), F32S_PER_VERTEX * 4);
        assert_eq!(&g.indices[..], &[0, 1, 2, 0, 2, 3]);
        // Cube (2,-1,4) is 32,-16,64 blocks from the camera's cube, less where the camera sits in it.
        let corner = FACE_CORNERS[3][0];
        assert_eq!(&g.vertices[..3], &[32.0 + corner[0] as f32 - 0.5, -16.0 + corner[1] as f32 - 0.5, 64.0 + corner[2] as f32 - 0.5]);
        assert_eq!(&g.vertices[8..12], &[1.0, 1.0, 1.0, 1.0], "full light on every channel");
        assert_eq!(&g.vertices[5..8], &[SHADE[3], SHADE[3], SHADE[3]], "up, unoccluded, untinted");
    }

    #[test]
    fn a_body_offset_moves_the_face_and_its_light() {
        // Sky light 15, nothing else, on a region riding structure 1.
        let records = bytes_of(&record([0, 0, 0], 5, [3, 3, 3, 3], 0, 1, 0, [0x000F, 0, 0, 0]));
        let origins = bytes_of(&[0, 0, 0, 1]);
        // Entry 0 is the world itself, so a body's own place starts at 1.
        let mut offsets = bytes_of(&[0, 0, 0, 0]);
        for f in [10.0f32, 0.0, 0.0] {
            offsets.extend_from_slice(&f.to_le_bytes());
        }
        // The world's light where the structure stands: sky 8, no block light.
        offsets.extend_from_slice(&0x0000_0008u32.to_le_bytes());
        let mut g = Geometry::default();
        g.push_run(Kind::Faces, Draw { faces: 1, records: 0, origin: 0 }, &records, &origins, &offsets, Cam::default(), &mut Flat);
        assert_eq!(g.vertices[0], 10.0 + FACE_CORNERS[5][0][0] as f32, "the structure's own place");
        assert_eq!(&g.vertices[8..12], &[8.0 / 15.0, 0.0, 0.0, 0.0], "the darker of the two sky lights");
    }

    #[test]
    fn a_texture_with_no_room_still_draws_its_face() {
        let records = bytes_of(&record([0, 0, 0], 0, [3, 3, 3, 3], 0, 99, 0, [0; 4]));
        let origins = bytes_of(&[0, 0, 0, 0]);
        let mut g = Geometry::default();
        g.push_run(Kind::Faces, Draw { faces: 1, records: 0, origin: 0 }, &records, &origins, &[], Cam::default(), &mut Flat);
        assert_eq!(g.vertices.len(), F32S_PER_VERTEX * 4);
        assert_eq!(&g.vertices[3..5], &[0.5, 0.5]);
    }

    #[test]
    fn corners_become_the_sheet_spots_of_their_texture() {
        let records = bytes_of(&record([0, 0, 0], 0, [3, 3, 3, 3], 0, 1, 0, [0; 4]));
        let origins = bytes_of(&[0, 0, 0, 0]);
        let mut g = Geometry::default();
        g.push_run(Kind::Faces, Draw { faces: 1, records: 0, origin: 0 }, &records, &origins, &[], Cam::default(), &mut Flat);
        let sheet = Rect { x: 0, y: 0, width: 16, height: 16, sheet: 16 };
        for i in 0..4 {
            let want = sheet.uv(UVS[i][0], UVS[i][1]);
            let got = (g.vertices[i * F32S_PER_VERTEX + 3], g.vertices[i * F32S_PER_VERTEX + 4]);
            assert_eq!(got, want, "corner {i} should sample its own texels");
        }
    }

    #[test]
    fn a_run_steps_one_record_per_face() {
        let mut page = bytes_of(&record([0, 0, 0], 3, [3, 3, 3, 3], 0, 1, 0, [0; 4]));
        page.extend_from_slice(&bytes_of(&record([1, 2, 3], 3, [3, 3, 3, 3], 0, 2, 0, [0; 4])));
        let origins = bytes_of(&[0u32, 0, 0, 0]);
        let mut g = Geometry::default();
        g.push_run(Kind::Faces, Draw { faces: 2, records: 0, origin: 0 }, &page, &origins, &[], Cam::default(), &mut Flat);
        assert_eq!(g.vertices.len(), 2 * 4 * F32S_PER_VERTEX, "a run is one record per face, not one per four");
        let corner = FACE_CORNERS[3][0];
        let second = 4 * F32S_PER_VERTEX;
        assert_eq!(&g.vertices[second..second + 3], &[1.0 + corner[0] as f32, 2.0 + corner[1] as f32, 3.0 + corner[2] as f32]);
        assert_eq!(&g.indices[6..], &[4, 5, 6, 4, 6, 7], "the second face joins its own four corners");
        let mut later = Geometry::default();
        later.push_run(Kind::Faces, Draw { faces: 1, records: 1, origin: 0 }, &page, &origins, &[], Cam::default(), &mut Flat);
        assert_eq!(&later.vertices[..3], &g.vertices[second..second + 3], "a run can start further along the page");
    }

    #[test]
    fn a_run_of_quads_steps_eight_words_a_record() {
        let a = quad([[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0]], 1, [0.0, 0.0, 1.0, 1.0], 0, true, [0; 4]);
        let b = quad([[2.0, 0.0, 0.0], [2.0, 0.0, 1.0], [3.0, 0.0, 1.0], [3.0, 0.0, 0.0]], 2, [0.0, 0.0, 1.0, 1.0], 0, true, [0; 4]);
        let mut page = bytes_of(&a);
        page.extend_from_slice(&bytes_of(&b));
        let origins = bytes_of(&[0u32, 0, 0, 0]);
        let mut g = Geometry::default();
        g.push_run(Kind::Quads, Draw { faces: 2, records: 0, origin: 0 }, &page, &origins, &[], Cam::default(), &mut Flat);
        assert_eq!(g.vertices.len(), 2 * 4 * F32S_PER_VERTEX);
        assert_eq!(g.vertices[4 * F32S_PER_VERTEX], 2.0, "the second record is the one two blocks over");
        assert_eq!(g.vertices[4 * F32S_PER_VERTEX + 3], g.vertices[3], "both unwrap the whole picture");
    }

    #[test]
    fn a_quads_shade_comes_from_its_normal() {
        let shade = |pos: [[f32; 3]; 4]| quad_corners(&quad(pos, 1, [0.0, 0.0, 1.0, 1.0], 0, false, [0; 4]))[0].shade;
        assert_eq!(shade([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]]), 0.6, "n = +X");
        assert_eq!(shade([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]), 0.8, "n = +Z");
        assert_eq!(shade([[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0]]), 1.0, "n = +Y, the brightest");
        assert_eq!(shade([[1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]), 0.5, "n = -Y, which is not upward");
        let plant = quad([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], 1, [0.0, 0.0, 1.0, 1.0], 0, true, [0; 4]);
        assert_eq!(quad_corners(&plant)[0].shade, 1.0, "FLAT opts out, as crossed squares do in vanilla");
    }

    #[test]
    fn a_quads_uv_word_is_the_part_of_the_picture_it_unwraps() {
        let r = quad([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], 3, [0.0, 0.0625, 0.5, 0.75], 0, true, [0; 4]);
        let c = quad_corners(&r);
        assert_eq!(c[0].uv, [0.0, 0.75], "u0, v1");
        assert_eq!(c[1].uv, [0.5, 0.75]);
        assert_eq!(c[2].uv, [0.5, 0.0625]);
        assert_eq!(c[3].uv, [0.0, 0.0625]);
        assert_eq!((c[0].texture, c[0].tint), (3, 0));
        assert_eq!(c[2].block, [1, 1, 0], "the block a tint table is looked up at");
    }
}
