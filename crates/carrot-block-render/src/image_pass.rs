//! Image-pass composition.
//!
//! Terminal image protocols (the common graphics protocol, Sixel, inline-image)
//! all decode to RGBA / RGB / Grayscale pixel buffers — carrot-grid's
//! [`ImageStore`] holds the decoded bytes plus a [`Placement`] that
//! says "this image spans rows × cols starting at (r, c), with
//! optional sub-cell pixel offsets".
//!
//! This module turns that data into [`ImageDraw`] commands in
//! cell-local pixel coordinates. The consumer (Layer 5 terminal view
//! via wgpu) composites them **after** the text pass so images sit
//! on top of the background layer but under the cursor.
//!
//! # Scope
//!
//! Foundation only. What's here:
//! - Placement → pixel-rect projection with sub-cell offsets.
//! - Row-range clipping so images partially scrolled off-screen
//!   still emit a correctly-bounded draw (the consumer crops via
//!   texture-coord math at render time, not here).
//! - Arc-shared image payload so multiple draws of the same image
//!   (e.g. a repeating background tile) cost nothing extra.
//!
//! What's **not** here:
//! - Actual wgpu texture uploads + lifecycle (atlas vs. individual
//!   textures, eviction).
//! - Alpha-premultiplication correctness for the chosen blend mode.
//! - Animated image frames (APNG / GIF) — a future tag inside the
//!   Placement.external_id namespace.

use std::ops::Range;
use std::sync::Arc;

use carrot_grid::{DecodedImage, ImageEntry, ImageStore, Placement};

/// One image ready to paint. Rect is in cell-local pixel space; the
/// consumer adds the block origin.
#[derive(Debug, Clone)]
pub struct ImageDraw {
    /// Placement the draw was derived from — gives the consumer
    /// access to row/col range for hit-testing and z-order.
    pub placement: Placement,
    /// Top-left pixel coordinate inside the block.
    pub pixel_x: f32,
    pub pixel_y: f32,
    /// Pixel dimensions of the projected image rect.
    pub pixel_w: f32,
    pub pixel_h: f32,
    /// The shared decoded image. `Arc::clone` is O(1); multiple
    /// ImageDraws can reference the same DecodedImage if a caller
    /// tiles an image or renders the same entry twice.
    pub image: Arc<DecodedImage>,
}

impl ImageDraw {
    /// Whether any row of the image falls inside `visible_rows`.
    /// Consumers use this to skip wholesale-off-screen images.
    pub fn intersects_rows(&self, visible_rows: &Range<u32>) -> bool {
        let start = self.placement.row_start;
        let end = start + self.placement.rows as u32;
        start < visible_rows.end && end > visible_rows.start
    }
}

/// Project every entry in `store` into pixel-space [`ImageDraw`]s.
///
/// `cell_w` / `cell_h` are the font-derived cell dimensions in pixels.
/// Sub-cell offsets on the Placement are applied on top.
///
/// Returns draws in insertion order (matches the scrollback order of
/// image placements); the consumer paints them in that order so
/// later-placed images sit above earlier ones (last-placement-wins).
pub fn render_images(store: &ImageStore, cell_w: f32, cell_h: f32) -> Vec<ImageDraw> {
    let mut out = Vec::with_capacity(store.len());
    for entry in store.iter() {
        out.push(project(entry, cell_w, cell_h));
    }
    out
}

fn project(entry: &ImageEntry, cell_w: f32, cell_h: f32) -> ImageDraw {
    let pixel_x = entry.placement.col_start as f32 * cell_w + entry.placement.offset_x as f32;
    let pixel_y = entry.placement.row_start as f32 * cell_h + entry.placement.offset_y as f32;
    let pixel_w = entry.placement.cols as f32 * cell_w;
    let pixel_h = entry.placement.rows as f32 * cell_h;
    ImageDraw {
        placement: entry.placement,
        pixel_x,
        pixel_y,
        pixel_w,
        pixel_h,
        image: Arc::clone(&entry.image),
    }
}

/// Filter `draws` to only those that intersect `visible_rows`.
/// Convenience for consumers that already have a visible range.
pub fn filter_visible(draws: &[ImageDraw], visible_rows: Range<u32>) -> Vec<ImageDraw> {
    draws
        .iter()
        .filter(|d| d.intersects_rows(&visible_rows))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::{DecodedImage, ImageFormat, Placement};

    fn rgba(w: u32, h: u32) -> Arc<DecodedImage> {
        let bytes = vec![0u8; (w * h * 4) as usize];
        Arc::new(DecodedImage::new(w, h, ImageFormat::Rgba8, bytes))
    }

    fn make_store(entries: Vec<(Placement, Arc<DecodedImage>)>) -> ImageStore {
        let mut store = ImageStore::new();
        for (placement, image) in entries {
            store.push(image, placement);
        }
        store
    }

    #[test]
    fn empty_store_produces_empty_draws() {
        let store = ImageStore::new();
        let draws = render_images(&store, 8.0, 16.0);
        assert!(draws.is_empty());
    }

    #[test]
    fn single_placement_projects_to_expected_rect() {
        let store = make_store(vec![(Placement::at(3, 5, 2, 4), rgba(32, 32))]);
        let draws = render_images(&store, 8.0, 16.0);
        assert_eq!(draws.len(), 1);
        let d = &draws[0];
        // col_start 5 × cell_w 8 = 40
        assert_eq!(d.pixel_x, 40.0);
        // row_start 3 × cell_h 16 = 48
        assert_eq!(d.pixel_y, 48.0);
        // 4 cols × 8 = 32, 2 rows × 16 = 32
        assert_eq!(d.pixel_w, 32.0);
        assert_eq!(d.pixel_h, 32.0);
    }

    #[test]
    fn sub_cell_offsets_add_to_pixel_origin() {
        let placement = Placement {
            row_start: 2,
            col_start: 3,
            rows: 1,
            cols: 1,
            offset_x: 4,
            offset_y: -2,
            external_id: 0,
        };
        let store = make_store(vec![(placement, rgba(8, 8))]);
        let draws = render_images(&store, 8.0, 16.0);
        // 3 × 8 + 4 = 28;  2 × 16 + (-2) = 30
        assert_eq!(draws[0].pixel_x, 28.0);
        assert_eq!(draws[0].pixel_y, 30.0);
    }

    #[test]
    fn multiple_images_preserve_insertion_order() {
        let store = make_store(vec![
            (Placement::at(0, 0, 1, 1), rgba(8, 8)),
            (Placement::at(5, 5, 1, 1), rgba(8, 8)),
            (Placement::at(10, 10, 1, 1), rgba(8, 8)),
        ]);
        let draws = render_images(&store, 8.0, 16.0);
        assert_eq!(draws.len(), 3);
        assert_eq!(draws[0].placement.row_start, 0);
        assert_eq!(draws[1].placement.row_start, 5);
        assert_eq!(draws[2].placement.row_start, 10);
    }

    #[test]
    fn intersects_rows_matches_overlap() {
        let store = make_store(vec![(Placement::at(10, 0, 5, 1), rgba(8, 80))]);
        let draws = render_images(&store, 8.0, 16.0);
        let d = &draws[0];
        // image spans rows 10..15
        assert!(d.intersects_rows(&(0..20)));
        assert!(d.intersects_rows(&(14..30)));
        assert!(!d.intersects_rows(&(0..10)));
        assert!(!d.intersects_rows(&(15..30)));
    }

    #[test]
    fn filter_visible_drops_offscreen_images() {
        let store = make_store(vec![
            (Placement::at(0, 0, 3, 2), rgba(16, 48)),
            (Placement::at(20, 0, 3, 2), rgba(16, 48)),
            (Placement::at(100, 0, 3, 2), rgba(16, 48)),
        ]);
        let all = render_images(&store, 8.0, 16.0);
        let visible = filter_visible(&all, 15..50);
        // Only the placement at row 20 intersects rows 15..50.
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].placement.row_start, 20);
    }

    #[test]
    fn shared_image_arc_counts_as_one_backing() {
        let shared = rgba(8, 8);
        let store = make_store(vec![
            (Placement::at(0, 0, 1, 1), shared.clone()),
            (Placement::at(5, 0, 1, 1), shared.clone()),
        ]);
        let draws = render_images(&store, 8.0, 16.0);
        // Both draws reference the same Arc payload.
        assert!(Arc::ptr_eq(&draws[0].image, &draws[1].image));
    }

    #[test]
    fn image_draw_carries_placement_for_hit_testing() {
        let p = Placement {
            row_start: 7,
            col_start: 2,
            rows: 3,
            cols: 4,
            offset_x: 0,
            offset_y: 0,
            external_id: 42,
        };
        let store = make_store(vec![(p, rgba(32, 48))]);
        let draws = render_images(&store, 8.0, 16.0);
        assert_eq!(draws[0].placement.external_id, 42);
        assert_eq!(draws[0].placement.rows, 3);
        assert_eq!(draws[0].placement.cols, 4);
    }
}
