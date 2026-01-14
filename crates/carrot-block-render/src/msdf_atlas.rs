//! Multi-channel Signed Distance Field glyph atlas.
//!
//! An MSDF atlas stores each glyph once at a single rasterisation
//! resolution. The GPU fragment shader samples the three channels
//! and reconstructs sharp edges at **any** scale — no re-rasterisation
//! when the user zooms, rotates, or uses a HiDPI display. That's
//! the payoff vs the classic grayscale alpha atlas.
//!
//! This module owns the **CPU side** of the atlas:
//!
//! - [`MsdfGlyph`]: atlas coordinates + metrics for one glyph.
//! - [`MsdfAtlas`]: keyed cache with LRU eviction + free-rect packer.
//! - [`GlyphKey`]: `(font_id, glyph_id, px_size)` tuple that uniquely
//!   identifies a glyph record.
//!
//! GPU upload lives in [`super::msdf_upload`]. The atlas tracks the
//! rectangles it has modified since the last flush; the uploader
//! drains that list, pads RGB → RGBA, and calls
//! `wgpu::Queue::write_texture` per rect.

use std::collections::HashMap;
use std::num::NonZeroU32;

/// Key identifying a cached glyph. `font_id` is opaque — the renderer
/// maps its own font handle to this id before calling in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font_id: u32,
    pub glyph_id: u32,
    /// Rasterisation px size, quantised to the nearest u16 so small
    /// float wiggles don't fragment the cache.
    pub px_size: u16,
}

impl GlyphKey {
    pub fn new(font_id: u32, glyph_id: u32, px_size: u16) -> Self {
        Self {
            font_id,
            glyph_id,
            px_size,
        }
    }
}

/// Per-glyph record. `atlas_*` are texture coordinates in pixels
/// (not UV 0–1 — the shader divides by atlas size). `bearing_*` and
/// `advance` are font metrics in px at the key's `px_size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsdfGlyph {
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
    pub bearing_x: f32,
    pub bearing_y: f32,
    pub advance: f32,
}

/// A dirty rectangle in atlas pixel coordinates. Emitted by
/// [`MsdfAtlas::insert`] / [`MsdfAtlas::clear`] so the uploader knows
/// exactly which sub-region of the `wgpu::Texture` to refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl DirtyRect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }
}

/// CPU-side MSDF atlas. Fixed-size texture with a simple shelf
/// packer and LRU eviction. 3 bytes per pixel (MSDF channels R, G, B;
/// alpha skipped because single-distance MSDF doesn't need it).
pub struct MsdfAtlas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    glyphs: HashMap<GlyphKey, GlyphRecord>,
    access_counter: u64,
    // Packer state.
    shelf_y: u32,
    shelf_h: u32,
    cursor_x: u32,
    /// Rectangles the CPU has written into since the last
    /// [`Self::take_pending_uploads`] call. Consumed by the GPU
    /// uploader; never grows unbounded because each flush drains it.
    pending_uploads: Vec<DirtyRect>,
}

#[derive(Debug, Clone, Copy)]
struct GlyphRecord {
    glyph: MsdfGlyph,
    last_used: u64,
}

impl MsdfAtlas {
    /// Fresh atlas with an opaque-black backing bitmap.
    ///
    /// `width` / `height` are texture dimensions in pixels. Typical
    /// choice is 2048×2048 on desktop; the constructor accepts any
    /// positive size.
    pub fn new(width: NonZeroU32, height: NonZeroU32) -> Self {
        let w = width.get();
        let h = height.get();
        Self {
            width: w,
            height: h,
            pixels: vec![0u8; (w * h * 3) as usize],
            glyphs: HashMap::new(),
            access_counter: 0,
            shelf_y: 0,
            shelf_h: 0,
            cursor_x: 0,
            pending_uploads: Vec::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Number of glyphs currently cached.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Lookup + LRU bump. Returns `None` when the glyph isn't cached.
    pub fn get(&mut self, key: GlyphKey) -> Option<MsdfGlyph> {
        let rec = self.glyphs.get_mut(&key)?;
        self.access_counter += 1;
        rec.last_used = self.access_counter;
        Some(rec.glyph)
    }

    /// Insert a new glyph. `bitmap` must be `w * h * 3` bytes of MSDF
    /// channel data. Returns the stored [`MsdfGlyph`] record or an
    /// error if the glyph is larger than the atlas dimensions.
    ///
    /// Packing strategy: single-row shelf. When the current shelf
    /// runs out of horizontal room, start a new shelf below. When
    /// the next shelf would overflow the atlas, evict the oldest-
    /// used glyph until we fit.
    pub fn insert(
        &mut self,
        key: GlyphKey,
        w: u32,
        h: u32,
        bearing_x: f32,
        bearing_y: f32,
        advance: f32,
        bitmap: &[u8],
    ) -> Result<MsdfGlyph, MsdfInsertError> {
        if w == 0 || h == 0 {
            return Err(MsdfInsertError::ZeroSizedGlyph);
        }
        if w > self.width || h > self.height {
            return Err(MsdfInsertError::GlyphLargerThanAtlas);
        }
        if bitmap.len() != (w * h * 3) as usize {
            return Err(MsdfInsertError::BitmapSizeMismatch);
        }

        // Advance shelf if the current row is full.
        if self.cursor_x + w > self.width {
            self.shelf_y += self.shelf_h;
            self.shelf_h = 0;
            self.cursor_x = 0;
        }
        // Grow shelf height to fit the tallest glyph on it.
        if h > self.shelf_h {
            self.shelf_h = h;
        }
        // Out of room → evict until we fit.
        while self.shelf_y + self.shelf_h > self.height {
            if !self.evict_lru() {
                // Nothing left to evict — atlas is too small.
                return Err(MsdfInsertError::AtlasExhausted);
            }
            self.shelf_y = 0;
            self.shelf_h = h;
            self.cursor_x = 0;
        }

        // Copy bitmap rows into the atlas.
        for row in 0..h {
            let src = (row * w * 3) as usize;
            let dst = ((self.shelf_y + row) * self.width * 3 + self.cursor_x * 3) as usize;
            self.pixels[dst..dst + (w * 3) as usize]
                .copy_from_slice(&bitmap[src..src + (w * 3) as usize]);
        }

        self.pending_uploads
            .push(DirtyRect::new(self.cursor_x, self.shelf_y, w, h));
        let glyph = MsdfGlyph {
            atlas_x: self.cursor_x,
            atlas_y: self.shelf_y,
            atlas_w: w,
            atlas_h: h,
            bearing_x,
            bearing_y,
            advance,
        };
        self.access_counter += 1;
        self.glyphs.insert(
            key,
            GlyphRecord {
                glyph,
                last_used: self.access_counter,
            },
        );
        self.cursor_x += w;
        Ok(glyph)
    }

    /// Drop the least-recently-used glyph. Returns `true` if something
    /// was evicted, `false` if the atlas is empty.
    ///
    /// Eviction frees the record but leaves the pixel data in place —
    /// the next shelf overwrites it. This avoids a full compaction
    /// pass on every miss.
    pub fn evict_lru(&mut self) -> bool {
        let Some((key, _)) = self
            .glyphs
            .iter()
            .min_by_key(|(_, rec)| rec.last_used)
            .map(|(k, rec)| (*k, rec.last_used))
        else {
            return false;
        };
        self.glyphs.remove(&key);
        true
    }

    /// Clear the entire atlas. Used by `ReflowReason::FontSize` /
    /// `ReflowReason::FontFamily` reflows where every glyph is
    /// invalidated simultaneously. Queues a full-atlas dirty rect so
    /// the GPU uploader overwrites the existing texture in one call.
    pub fn clear(&mut self) {
        self.glyphs.clear();
        self.pixels.fill(0);
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.cursor_x = 0;
        self.pending_uploads.clear();
        self.pending_uploads
            .push(DirtyRect::new(0, 0, self.width, self.height));
    }

    /// Extract a read-only view of the RGB pixel buffer for the
    /// sub-region described by `rect`. Rows are contiguous within
    /// the returned buffer: `rect.w * 3` bytes per row, `rect.h` rows.
    pub fn extract_rect_rgb(&self, rect: DirtyRect) -> Vec<u8> {
        assert!(
            rect.x + rect.w <= self.width && rect.y + rect.h <= self.height,
            "DirtyRect out of bounds",
        );
        let mut out = Vec::with_capacity((rect.w * rect.h * 3) as usize);
        for row in 0..rect.h {
            let src_row = rect.y + row;
            let src_start = (src_row * self.width * 3 + rect.x * 3) as usize;
            let src_end = src_start + (rect.w * 3) as usize;
            out.extend_from_slice(&self.pixels[src_start..src_end]);
        }
        out
    }

    /// Drain and return the list of rects that need a GPU upload.
    /// Callers pass the rects and the corresponding pixel bytes
    /// (via [`Self::extract_rect_rgb`]) to the uploader.
    pub fn take_pending_uploads(&mut self) -> Vec<DirtyRect> {
        std::mem::take(&mut self.pending_uploads)
    }

    /// Whether any rects are pending GPU upload. Cheap to poll every
    /// frame.
    pub fn has_pending_uploads(&self) -> bool {
        !self.pending_uploads.is_empty()
    }
}

impl std::fmt::Debug for MsdfAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MsdfAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("glyphs", &self.glyphs.len())
            .field("shelf_y", &self.shelf_y)
            .field("shelf_h", &self.shelf_h)
            .field("cursor_x", &self.cursor_x)
            .finish()
    }
}

/// Insertion failure modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsdfInsertError {
    ZeroSizedGlyph,
    GlyphLargerThanAtlas,
    BitmapSizeMismatch,
    /// Every cached glyph would need to be evicted and the new glyph
    /// still wouldn't fit. Callers should grow the atlas.
    AtlasExhausted,
}

impl std::fmt::Display for MsdfInsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsdfInsertError::ZeroSizedGlyph => write!(f, "glyph has zero dimensions"),
            MsdfInsertError::GlyphLargerThanAtlas => {
                write!(f, "glyph exceeds atlas dimensions")
            }
            MsdfInsertError::BitmapSizeMismatch => {
                write!(f, "bitmap length does not equal w * h * 3")
            }
            MsdfInsertError::AtlasExhausted => {
                write!(f, "atlas exhausted even after full eviction")
            }
        }
    }
}

impl std::error::Error for MsdfInsertError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas(w: u32, h: u32) -> MsdfAtlas {
        MsdfAtlas::new(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
    }

    fn bitmap_bytes(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 3) as usize]
    }

    #[test]
    fn fresh_atlas_is_empty() {
        let a = atlas(64, 64);
        assert_eq!(a.width(), 64);
        assert_eq!(a.height(), 64);
        assert!(a.is_empty());
        assert_eq!(a.pixels().len(), 64 * 64 * 3);
    }

    #[test]
    fn insert_and_lookup_roundtrip() {
        let mut a = atlas(64, 64);
        let key = GlyphKey::new(0, 42, 16);
        let g = a
            .insert(key, 8, 8, 0.0, 8.0, 9.5, &bitmap_bytes(8, 8))
            .unwrap();
        assert_eq!(g.atlas_x, 0);
        assert_eq!(g.atlas_y, 0);
        assert_eq!(g.atlas_w, 8);
        assert_eq!(g.advance, 9.5);
        assert_eq!(a.len(), 1);

        let looked_up = a.get(key).unwrap();
        assert_eq!(looked_up, g);
    }

    #[test]
    fn rejects_zero_dimensions() {
        let mut a = atlas(64, 64);
        let err = a
            .insert(GlyphKey::new(0, 0, 16), 0, 8, 0.0, 0.0, 0.0, &[])
            .unwrap_err();
        assert_eq!(err, MsdfInsertError::ZeroSizedGlyph);
    }

    #[test]
    fn rejects_oversized_glyph() {
        let mut a = atlas(16, 16);
        let err = a
            .insert(
                GlyphKey::new(0, 0, 16),
                32,
                8,
                0.0,
                0.0,
                0.0,
                &bitmap_bytes(32, 8),
            )
            .unwrap_err();
        assert_eq!(err, MsdfInsertError::GlyphLargerThanAtlas);
    }

    #[test]
    fn rejects_bitmap_size_mismatch() {
        let mut a = atlas(64, 64);
        let err = a
            .insert(GlyphKey::new(0, 0, 16), 8, 8, 0.0, 0.0, 0.0, &[0u8; 10])
            .unwrap_err();
        assert_eq!(err, MsdfInsertError::BitmapSizeMismatch);
    }

    #[test]
    fn second_glyph_packs_horizontally() {
        let mut a = atlas(64, 16);
        let a_key = GlyphKey::new(0, 1, 16);
        let b_key = GlyphKey::new(0, 2, 16);
        a.insert(a_key, 8, 16, 0.0, 0.0, 0.0, &bitmap_bytes(8, 16))
            .unwrap();
        let b = a
            .insert(b_key, 8, 16, 0.0, 0.0, 0.0, &bitmap_bytes(8, 16))
            .unwrap();
        assert_eq!(b.atlas_x, 8);
        assert_eq!(b.atlas_y, 0);
    }

    #[test]
    fn row_overflow_starts_new_shelf() {
        // 64-wide atlas fits exactly 4×16-pixel glyphs per shelf.
        let mut a = atlas(64, 32);
        for i in 0..4 {
            a.insert(
                GlyphKey::new(0, i, 16),
                16,
                16,
                0.0,
                0.0,
                0.0,
                &bitmap_bytes(16, 16),
            )
            .unwrap();
        }
        // Fifth glyph starts the next shelf at y = 16.
        let fifth = a
            .insert(
                GlyphKey::new(0, 4, 16),
                16,
                16,
                0.0,
                0.0,
                0.0,
                &bitmap_bytes(16, 16),
            )
            .unwrap();
        assert_eq!(fifth.atlas_x, 0);
        assert_eq!(fifth.atlas_y, 16);
    }

    #[test]
    fn saturated_atlas_recycles_via_lru_eviction() {
        let mut a = atlas(8, 8);
        let k1 = GlyphKey::new(0, 0, 16);
        let k2 = GlyphKey::new(0, 1, 16);
        a.insert(k1, 8, 8, 0.0, 0.0, 0.0, &bitmap_bytes(8, 8))
            .unwrap();
        // Second glyph of the same size triggers LRU eviction of k1,
        // then packs at (0, 0). Atlas stays size 1.
        let second = a
            .insert(k2, 8, 8, 0.0, 0.0, 0.0, &bitmap_bytes(8, 8))
            .unwrap();
        assert_eq!(second.atlas_x, 0);
        assert_eq!(second.atlas_y, 0);
        assert_eq!(a.len(), 1);
        assert!(a.get(k1).is_none());
        assert!(a.get(k2).is_some());
    }

    #[test]
    fn clear_resets_state() {
        let mut a = atlas(32, 32);
        a.insert(
            GlyphKey::new(0, 0, 16),
            8,
            8,
            0.0,
            0.0,
            0.0,
            &bitmap_bytes(8, 8),
        )
        .unwrap();
        a.clear();
        assert!(a.is_empty());
        // After clear, first insert starts at (0, 0) again.
        let after = a
            .insert(
                GlyphKey::new(0, 1, 16),
                8,
                8,
                0.0,
                0.0,
                0.0,
                &bitmap_bytes(8, 8),
            )
            .unwrap();
        assert_eq!(after.atlas_x, 0);
        assert_eq!(after.atlas_y, 0);
    }

    #[test]
    fn evict_lru_drops_oldest() {
        let mut a = atlas(64, 64);
        let k1 = GlyphKey::new(0, 1, 16);
        let k2 = GlyphKey::new(0, 2, 16);
        a.insert(k1, 8, 8, 0.0, 0.0, 0.0, &bitmap_bytes(8, 8))
            .unwrap();
        a.insert(k2, 8, 8, 0.0, 0.0, 0.0, &bitmap_bytes(8, 8))
            .unwrap();
        // Touch k2 so k1 is oldest.
        a.get(k2);
        assert!(a.evict_lru());
        assert!(a.get(k1).is_none());
        assert!(a.get(k2).is_some());
    }

    #[test]
    fn evict_empty_atlas_returns_false() {
        let mut a = atlas(16, 16);
        assert!(!a.evict_lru());
    }

    #[test]
    fn insert_error_display_includes_category() {
        for err in [
            MsdfInsertError::ZeroSizedGlyph,
            MsdfInsertError::GlyphLargerThanAtlas,
            MsdfInsertError::BitmapSizeMismatch,
            MsdfInsertError::AtlasExhausted,
        ] {
            let s = format!("{err}");
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn debug_output_has_useful_fields() {
        let a = atlas(32, 32);
        let s = format!("{a:?}");
        assert!(s.contains("width"));
        assert!(s.contains("glyphs"));
    }
}
