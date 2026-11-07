//! One sampled sheet for GPUs that cannot hold a bindless texture table.
//!
//! A FL10_0 or GL 2.1 shader gets a single texture and constant addressing, so every texture the
//! engine uploads has to live inside one sheet, and the id a mesh record carries has to become a
//! rectangle in it. Packing happens here, on the CPU, from the pixels a backend is handed by
//! `write_texture`. Sources are kept after packing, so growing the sheet is a repack rather than
//! a failed frame: a big resource pack on a 64 MB GPU needs that.

use std::collections::HashMap;

/// Where one texture sits in a sheet, in texels. The border around it belongs to no texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Sheet size when this rectangle was placed: divide by it to get 0..1 across the sheet.
    pub sheet: u16,
}

impl Rect {
    /// `uv` across the texture (0..1), as 0..1 across the sheet. It lands on texel centres and
    /// stops half a texel inside the rectangle, so no filtered read reaches a neighbour.
    pub fn uv(&self, u: f32, v: f32) -> (f32, f32) {
        let (x, y, sheet) = (f32::from(self.x), f32::from(self.y), f32::from(self.sheet.max(1)));
        let (w, h) = (f32::from(self.width).max(1.0), f32::from(self.height).max(1.0));
        (
            (x + (u.clamp(0.0, 1.0) * (w - 1.0)) + 0.5) / sheet,
            (y + (v.clamp(0.0, 1.0) * (h - 1.0)) + 0.5) / sheet,
        )
    }
}

/// A picture with no side under 1 or wider than the sheet, i.e. beyond the caller's GPU limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooBig;

/// Shelf-packed sheet: rows as tall as their tallest texture, one texel of border all round.
pub struct Atlas {
    sheet: u32,
    pixels: Vec<u8>,
    /// Ids in insertion order, so a backend that has to re-upload can do it deterministically.
    ids: Vec<u32>,
    rects: HashMap<u32, Rect>,
    /// `id, width, height, rgba` - everything needed to rebuild the sheet at another size.
    sources: HashMap<u32, (u32, u32, Vec<u8>)>,
    /// Next free column, the row's height, and the row's top edge.
    x: u32,
    row: u32,
    y: u32,
    /// Ids that found no room, for the caller to grow the sheet.
    dropped: Vec<u32>,
}

const BORDER: u32 = 1;

impl Atlas {
    pub fn new(sheet: u32) -> Atlas {
        let sheet = sheet.next_power_of_two().clamp(256, 8192);
        Atlas {
            sheet,
            pixels: vec![0; (sheet as usize).saturating_mul(sheet as usize).saturating_mul(4)],
            ids: Vec::new(),
            rects: HashMap::new(),
            sources: HashMap::new(),
            x: 0,
            row: 0,
            y: 0,
            dropped: Vec::new(),
        }
    }

    pub fn sheet(&self) -> u32 {
        self.sheet
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn rect(&self, id: u32) -> Option<Rect> {
        self.rects.get(&id).copied()
    }

    /// Textures that are packed, in insertion order.
    pub fn ids(&self) -> &[u32] {
        &self.ids
    }

    /// Packed pictures still missing at this size. `grow` clears them if the bigger sheet fits.
    pub fn dropped(&self) -> &[u32] {
        &self.dropped
    }

    /// Packs `id` and pastes it, keeping the pixels. Uploading the same id again replaces its
    /// picture, which is what a resource pack change does.
    pub fn insert(&mut self, id: u32, width: u32, height: u32, rgba: &[u8]) -> Result<Rect, TooBig> {
        let need = width.checked_mul(height).and_then(|n| n.checked_mul(4));
        if width == 0 || height == 0 || width > self.sheet || height > self.sheet {
            return Err(TooBig);
        }
        match need {
            Some(n) if rgba.len() >= n as usize => {
                let pixels = rgba[..n as usize].to_vec();
                self.sources.insert(id, (width, height, pixels.clone()));
                if !self.ids.contains(&id) {
                    self.ids.push(id);
                }
                self.dropped.retain(|d| *d != id);
                // Same size as the picture it already has: paste over it. A mesh built last frame
                // points at that rectangle, so moving it would need every chunk remeshed.
                if let Some(r) = self.rects.get(&id).copied() {
                    if r.width as u32 == width && r.height as u32 == height && r.sheet as u32 == self.sheet {
                        self.blit(r.x as u32, r.y as u32, width, height, &pixels);
                        return Ok(r);
                    }
                }
                self.pack(id)
            }
            _ => Err(TooBig),
        }
    }

    /// A bigger sheet with everything repacked, or `None` once 8192 has been reached.
    pub fn grow(mut self) -> Option<Atlas> {
        let next = self.sheet.checked_mul(2)?;
        if next > 8192 {
            return None;
        }
        let sources = std::mem::take(&mut self.sources);
        let ids = std::mem::take(&mut self.ids);
        let mut bigger = Atlas::new(next);
        for id in ids {
            if let Some((w, h, pixels)) = sources.get(&id) {
                bigger.insert(id, *w, *h, pixels).ok();
            }
        }
        Some(bigger)
    }

    /// Forgets a picture and closes the gap it left. Meshes built against the old rectangles have
    /// to be rebuilt, exactly like after `grow`.
    pub fn remove(&mut self, id: u32) {
        if self.sources.remove(&id).is_none() {
            return;
        }
        self.ids.retain(|i| *i != id);
        self.dropped.retain(|i| *i != id);
        self.rects.remove(&id);
        self.repack();
    }

    /// Rebuilds the sheet from the kept sources: after a resize, or to drop a texture.
    pub fn repack(&mut self) {
        let sources = std::mem::take(&mut self.sources);
        let ids = std::mem::take(&mut self.ids);
        let dropped = std::mem::take(&mut self.dropped);
        self.pixels.iter_mut().for_each(|p| *p = 0);
        self.rects.clear();
        (self.x, self.y, self.row) = (0, 0, 0);
        self.sources = sources;
        self.ids = ids;
        self.dropped = Vec::new();
        for id in &self.ids.clone() {
            self.pack(*id).ok();
        }
        for id in dropped {
            if !self.rects.contains_key(&id) {
                self.dropped.push(id);
            }
        }
    }

    /// Places one kept source, or records it as dropped when the sheet has no room left.
    fn pack(&mut self, id: u32) -> Result<Rect, TooBig> {
        let Some((width, height, _)) = self.sources.get(&id) else {
            return Err(TooBig);
        };
        let (width, height) = (*width, *height);
        let Some((x, y)) = self.slot(width, height) else {
            if !self.dropped.contains(&id) {
                self.dropped.push(id);
            }
            return Err(TooBig);
        };
        let pixels = self.sources[&id].2.clone();
        self.advance(width, height);
        self.blit(x, y, width, height, &pixels);
        let rect = Rect { x: x as u16, y: y as u16, width: width as u16, height: height as u16, sheet: self.sheet as u16 };
        self.rects.insert(id, rect);
        Ok(rect)
    }

    /// Where the next picture of this size would go: the current row if it fits, else a new row.
    fn slot(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        let need = width + 2 * BORDER;
        let tall = height + 2 * BORDER;
        let (x, y, row) = if self.x + need > self.sheet {
            (0, self.y + self.row, tall)
        } else {
            (self.x, self.y, self.row.max(tall))
        };
        (y + row <= self.sheet).then(|| (x + BORDER, y + BORDER))
    }

    /// Moves the cursor past a placed picture, starting a new row when the current one ends.
    fn advance(&mut self, width: u32, height: u32) {
        let need = width + 2 * BORDER;
        let tall = height + 2 * BORDER;
        if self.x + need > self.sheet {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        self.row = self.row.max(tall);
        self.x += need;
    }

    /// Pastes with its border filled by the edge texels, so an edge read gets the edge colour.
    fn blit(&mut self, x: u32, y: u32, width: u32, height: u32, rgba: &[u8]) {
        let sheet = self.sheet as usize;
        let left = x - BORDER;
        let top = y - BORDER;
        for by in 0..height + 2 * BORDER {
            let sy = by.saturating_sub(BORDER).min(height - 1) as usize;
            for bx in 0..width + 2 * BORDER {
                let sx = bx.saturating_sub(BORDER).min(width - 1) as usize;
                let dst = ((top + by) as usize * sheet + (left + bx) as usize) * 4;
                let src = (sy * width as usize + sx) * 4;
                self.pixels[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32, colour: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..w * h {
            v.extend_from_slice(&colour);
        }
        v
    }

    fn at(a: &Atlas, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * a.sheet() + x) * 4) as usize;
        [a.pixels()[i], a.pixels()[i + 1], a.pixels()[i + 2], a.pixels()[i + 3]]
    }

    #[test]
    fn remove_closes_the_gap_and_keeps_the_rest() {
        let mut a = Atlas::new(64);
        for i in 0..3 {
            a.insert(i, 16, 16, &picture(16, 16, [i as u8 + 1, 0, 0, 255])).unwrap();
        }
        a.remove(1);
        assert!(a.rect(1).is_none());
        assert_eq!(a.ids(), &[0, 2]);
        for id in [0, 2] {
            let r = a.rect(id).unwrap();
            assert_eq!(at(&a, r.x as u32, r.y as u32), [id as u8 + 1, 0, 0, 255]);
        }
        a.insert(3, 32, 32, &picture(32, 32, [9, 9, 9, 255])).unwrap();
        a.remove(99);
        assert_eq!(a.ids(), &[0, 2, 3]);
    }

    #[test]
    fn packs_without_overlap() {
        let mut a = Atlas::new(64);
        for i in 0..8 {
            a.insert(i, 16, 16, &picture(16, 16, [i as u8, 0, 0, 255])).unwrap();
        }
        assert!(a.dropped().is_empty());
        let rects: Vec<Rect> = (0..8).map(|i| a.rect(i).unwrap()).collect();
        for (i, r) in rects.iter().enumerate() {
            for o in &rects[i + 1..] {
                let apart = r.x + r.width + 1 <= o.x
                    || o.x + o.width + 1 <= r.x
                    || r.y + r.height + 1 <= o.y
                    || o.y + o.height + 1 <= r.y;
                assert!(apart, "{r:?} touches {o:?}");
            }
        }
    }

    #[test]
    fn pixels_land_where_the_rect_says_and_the_border_repeats_them() {
        let mut a = Atlas::new(64);
        a.insert(7, 16, 16, &picture(16, 16, [12, 34, 56, 255])).unwrap();
        let r = a.rect(7).unwrap();
        assert_eq!(at(&a, r.x as u32, r.y as u32), [12, 34, 56, 255]);
        assert_eq!(at(&a, r.x as u32 + 15, r.y as u32 + 15), [12, 34, 56, 255]);
        assert_eq!(at(&a, r.x as u32 - 1, r.y as u32), [12, 34, 56, 255]);
        // The texel past the border belongs to the next texture in the row, not to this one.
        assert_eq!(at(&a, r.x as u32 + 17, r.y as u32), [0, 0, 0, 0]);
    }

    #[test]
    fn uv_covers_the_texture_inside_its_border() {
        let mut a = Atlas::new(64);
        a.insert(1, 16, 16, &picture(16, 16, [1, 2, 3, 4])).unwrap();
        let r = a.rect(1).unwrap();
        let s = f32::from(r.sheet);
        for (u, v) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.25), (2.0, -1.0)] {
            let (x, y) = r.uv(u, v);
            assert!(x * s >= f32::from(r.x) && x * s <= f32::from(r.x) + f32::from(r.width), "{u},{v} -> {x}");
            assert!(y * s >= f32::from(r.y) && y * s <= f32::from(r.y) + f32::from(r.height), "{u},{v} -> {y}");
        }
        assert_eq!(r.uv(0.0, 0.0), (0.5 / s + f32::from(r.x) / s, 0.5 / s + f32::from(r.y) / s));
    }

    #[test]
    fn a_full_sheet_drops_and_growing_places_everything() {
        let mut a = Atlas::new(256);
        for i in 0..200 {
            a.insert(i, 16, 16, &picture(16, 16, [i as u8, 1, 2, 255])).ok();
        }
        assert!(!a.dropped().is_empty(), "200 textures of 16 do not fit a 256 sheet");
        let lost = a.dropped().to_vec();
        let mut b = Atlas::new(256);
        for i in 0..200 {
            b.insert(i, 16, 16, &picture(16, 16, [i as u8, 1, 2, 255])).ok();
        }
        let grew = b.grow().unwrap();
        assert_eq!(grew.sheet(), 512);
        assert!(grew.dropped().is_empty(), "{lost:?} still dropped at 512");
        for id in 0..200 {
            assert!(grew.rect(id).is_some(), "{id} has no rectangle");
        }
    }

    #[test]
    fn a_repeat_upload_replaces_the_picture() {
        let mut a = Atlas::new(64);
        a.insert(3, 16, 16, &picture(16, 16, [1, 1, 1, 255])).unwrap();
        let first = a.rect(3).unwrap();
        a.insert(3, 16, 16, &picture(16, 16, [2, 2, 2, 255])).unwrap();
        assert_eq!(a.rect(3), Some(first), "the same id should stay put");
        assert_eq!(at(&a, first.x as u32, first.y as u32), [2, 2, 2, 255]);
        assert_eq!(a.ids().len(), 1);
    }

    #[test]
    fn pictures_too_big_or_too_small_are_refused_not_panicked() {
        let mut a = Atlas::new(256);
        assert_eq!(a.insert(1, 512, 512, &[0; 4]), Err(TooBig));
        assert_eq!(a.insert(2, 0, 8, &[]), Err(TooBig));
        assert_eq!(a.insert(3, 8, 8, &[0; 4]), Err(TooBig), "short pixels");
        assert_eq!(a.insert(4, 8, 8, &picture(8, 8, [0, 0, 0, 255])), Ok(Rect { x: 1, y: 1, width: 8, height: 8, sheet: 256 }));
    }

    #[test]
    fn repack_after_a_resize_keeps_every_texture() {
        let mut a = Atlas::new(512);
        for i in 0..40 {
            a.insert(i, 16, 16, &picture(16, 16, [i as u8, 7, 7, 255])).unwrap();
        }
        let before: Vec<Rect> = (0..40).map(|i| a.rect(i).unwrap()).collect();
        a.repack();
        for (i, r) in before.iter().enumerate() {
            assert_eq!(a.rect(i as u32), Some(*r), "repack should place as before");
            assert_eq!(at(&a, r.x as u32, r.y as u32), [i as u8, 7, 7, 255]);
        }
    }
}
