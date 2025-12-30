//! Image store — out-of-band texture data referenced by [`crate::CellTag::Image`] cells.
//!
//! Supports the three terminal image-transport protocols at the data layer.
//! The store only holds **decoded** image bytes; protocol-specific decoding
//! (Sixel decompression, base64-decode, PNG/JPEG via `image` crate) happens
//! upstream in Layer 2's VT state machine.
//!
//! # Contract
//!
//! - An `ImageStore` is append-only during a block's active lifetime.
//! - Indices stay valid across the lifetime — callers pin them into cells.
//! - Arc-wrapped pixel data means the `ImageStore` of a frozen block is
//!   cheap to clone (only the `Arc` count increments).

use std::sync::Arc;

use crate::cell::ImageIndex;

/// Pixel format — only what terminal image protocols emit.
///
/// Sixel → `Rgba8` post-decode. The other protocols default to RGBA with
/// an alpha channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Rgba8,
    Rgb8,
    Grayscale8,
}

impl ImageFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            ImageFormat::Rgba8 => 4,
            ImageFormat::Rgb8 => 3,
            ImageFormat::Grayscale8 => 1,
        }
    }
}

/// Decoded image bytes. Shared via `Arc` so frozen-block clones are O(1).
#[derive(Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub pixels: Vec<u8>,
}

impl DecodedImage {
    pub fn new(width: u32, height: u32, format: ImageFormat, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            format,
            pixels,
        }
    }

    /// Expected buffer size given `width * height * bytes_per_pixel`.
    pub fn expected_len(&self) -> usize {
        self.width as usize * self.height as usize * self.format.bytes_per_pixel()
    }
}

/// Placement of an image inside the terminal grid.
///
/// Spans `rows × cols` cells starting at `(row_start, col_start)`. Pixel-level
/// offsets (`offset_x`, `offset_y`) allow sub-cell shifts when the image
/// doesn't align to a character boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub row_start: u32,
    pub col_start: u16,
    pub rows: u16,
    pub cols: u16,
    /// Sub-cell pixel offset in X.
    pub offset_x: i16,
    /// Sub-cell pixel offset in Y.
    pub offset_y: i16,
    /// Protocol-specific external identifier (image id or sequence number).
    /// `0` = no external id.
    pub external_id: u32,
}

impl Placement {
    pub const fn at(row_start: u32, col_start: u16, rows: u16, cols: u16) -> Self {
        Self {
            row_start,
            col_start,
            rows,
            cols,
            offset_x: 0,
            offset_y: 0,
            external_id: 0,
        }
    }
}

/// One entry in the store — the decoded bytes plus placement metadata.
#[derive(Debug)]
pub struct ImageEntry {
    pub image: Arc<DecodedImage>,
    pub placement: Placement,
}

/// Append-only image table per block. Cells reference entries by [`ImageIndex`].
#[derive(Debug, Default)]
pub struct ImageStore {
    entries: Vec<ImageEntry>,
}

impl ImageStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Append an image and return the id cells should reference.
    ///
    /// Saturates at [`u32::MAX`] entries — beyond that `push` returns
    /// `ImageIndex(u32::MAX)` without storing. Realistic workloads stay
    /// far below that.
    pub fn push(&mut self, image: Arc<DecodedImage>, placement: Placement) -> ImageIndex {
        if self.entries.len() >= u32::MAX as usize {
            return ImageIndex(u32::MAX);
        }
        let id = self.entries.len() as u32;
        self.entries.push(ImageEntry { image, placement });
        ImageIndex(id)
    }

    pub fn get(&self, idx: ImageIndex) -> Option<&ImageEntry> {
        self.entries.get(idx.0 as usize)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate all entries in insertion order — used by the renderer to
    /// emit the per-block image pass after the text pass.
    pub fn iter(&self) -> std::slice::Iter<'_, ImageEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_image() -> Arc<DecodedImage> {
        Arc::new(DecodedImage::new(2, 2, ImageFormat::Rgba8, vec![0u8; 16]))
    }

    #[test]
    fn format_byte_math_matches() {
        assert_eq!(ImageFormat::Rgba8.bytes_per_pixel(), 4);
        assert_eq!(ImageFormat::Rgb8.bytes_per_pixel(), 3);
        assert_eq!(ImageFormat::Grayscale8.bytes_per_pixel(), 1);
    }

    #[test]
    fn expected_len_from_dims() {
        let img = DecodedImage::new(4, 3, ImageFormat::Rgba8, vec![0u8; 48]);
        assert_eq!(img.expected_len(), 48);
    }

    #[test]
    fn push_returns_monotonic_ids() {
        let mut store = ImageStore::new();
        let a = store.push(tiny_image(), Placement::at(0, 0, 1, 1));
        let b = store.push(tiny_image(), Placement::at(1, 0, 1, 1));
        assert_eq!(a.0, 0);
        assert_eq!(b.0, 1);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn get_returns_entry() {
        let mut store = ImageStore::new();
        let id = store.push(tiny_image(), Placement::at(5, 10, 2, 4));
        let entry = store.get(id).expect("inserted");
        assert_eq!(entry.placement.row_start, 5);
        assert_eq!(entry.placement.col_start, 10);
        assert_eq!(entry.image.width, 2);
    }

    #[test]
    fn out_of_range_id_returns_none() {
        let store = ImageStore::new();
        assert!(store.get(ImageIndex(999)).is_none());
    }

    #[test]
    fn arc_shares_pixel_buffer_across_entries() {
        let shared = tiny_image();
        let mut store = ImageStore::new();
        let a = store.push(shared.clone(), Placement::at(0, 0, 1, 1));
        let b = store.push(shared.clone(), Placement::at(1, 0, 1, 1));
        let entry_a = store.get(a).expect("a");
        let entry_b = store.get(b).expect("b");
        // Same Arc => same pixel pointer.
        assert!(Arc::ptr_eq(&entry_a.image, &entry_b.image));
    }
}
