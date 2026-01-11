//! GPU upload path for terminal-inline images.
//!
//! Unlike glyphs, terminal images vary wildly in size — a graphics
//! protocol payload can be a full-resolution photo, a sixel drawing
//! can be a few cells tall. Atlas packing gives no benefit there, so
//! this module uses "one texture per image" and keys them by
//! [`carrot_grid::ImageIndex`].
//!
//! [`ImageGpuCache`] holds the GPU-side state; [`upload_new`] scans
//! the block's [`ImageStore`] and uploads any entry whose index has
//! not been seen yet. The cache is append-only during a block's
//! active lifetime — matching the `ImageStore` contract.

use std::collections::HashMap;

use carrot_grid::{ImageFormat, ImageIndex, ImageStore};
use wgpu::{
    Device, Extent3d, Origin3d, Queue, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

use crate::msdf_upload::rgb_to_rgba;

/// GPU-side image cache. `ImageIndex` → allocated `wgpu::Texture`.
/// Call [`upload_new`] once per frame to bring the GPU side in sync
/// with the CPU [`ImageStore`].
pub struct ImageGpuCache {
    entries: HashMap<ImageIndex, GpuImage>,
}

/// A resident image. The `view` is what the image-pass bind group
/// samples; the `format` is recorded so the consumer can pick the
/// right shader branch (grayscale vs. rgba).
pub struct GpuImage {
    texture: Texture,
    view: TextureView,
    format: TextureFormat,
    width: u32,
    height: u32,
}

impl GpuImage {
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    pub fn view(&self) -> &TextureView {
        &self.view
    }

    pub fn format(&self) -> TextureFormat {
        self.format
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

impl ImageGpuCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: ImageIndex) -> Option<&GpuImage> {
        self.entries.get(&id)
    }

    /// Whether `id` is already resident on the GPU. Consumers use this
    /// to skip redundant uploads — [`upload_new`] does the same check
    /// internally, but exposing it lets callers drive their own
    /// batching.
    pub fn contains(&self, id: ImageIndex) -> bool {
        self.entries.contains_key(&id)
    }

    /// Drop every cached texture. Called when the owning block is
    /// frozen and its GPU resources move to the frozen-block atlas,
    /// or when a theme change invalidates image decoding.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for ImageGpuCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Upload every entry in `store` that's not yet in `cache`. Returns
/// the count of newly uploaded images.
///
/// Pixel data is converted to a `wgpu`-compatible format:
/// - `Rgba8` → `Rgba8UnormSrgb` (straight copy).
/// - `Rgb8` → `Rgba8UnormSrgb` (padded via [`rgb_to_rgba`]).
/// - `Grayscale8` → `R8Unorm` (single-channel, no conversion).
///
/// The sRGB variant of `Rgba8Unorm` is chosen because terminal image
/// formats (PNG, JPEG, GIF, sixel) all deliver sRGB-encoded pixels and
/// the compositor blends in linear space — letting wgpu do the
/// gamma decode in the sampler is both correct and free.
pub fn upload_new(
    cache: &mut ImageGpuCache,
    store: &ImageStore,
    device: &Device,
    queue: &Queue,
) -> usize {
    let mut uploaded = 0;
    for (ix, entry) in store.iter().enumerate() {
        let id = ImageIndex(ix as u32);
        if cache.contains(id) {
            continue;
        }
        let image = &entry.image;
        let (format, pixel_bytes): (TextureFormat, Vec<u8>) = match image.format {
            ImageFormat::Rgba8 => (TextureFormat::Rgba8UnormSrgb, image.pixels.clone()),
            ImageFormat::Rgb8 => (TextureFormat::Rgba8UnormSrgb, rgb_to_rgba(&image.pixels)),
            ImageFormat::Grayscale8 => (TextureFormat::R8Unorm, image.pixels.clone()),
        };
        let gpu = allocate_and_fill(
            device,
            queue,
            image.width,
            image.height,
            format,
            &pixel_bytes,
            Some("carrot-image"),
        );
        cache.entries.insert(id, gpu);
        uploaded += 1;
    }
    uploaded
}

fn allocate_and_fill(
    device: &Device,
    queue: &Queue,
    width: u32,
    height: u32,
    format: TextureFormat,
    pixels: &[u8],
    label: Option<&str>,
) -> GpuImage {
    let texture = device.create_texture(&TextureDescriptor {
        label,
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes_per_pixel = match format {
        TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm => 4,
        TextureFormat::R8Unorm => 1,
        _ => unreachable!("unsupported image format"),
    };
    queue.write_texture(
        TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        pixels,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_pixel),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&TextureViewDescriptor::default());
    GpuImage {
        texture,
        view,
        format,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use carrot_grid::{DecodedImage, Placement};

    #[test]
    fn contains_starts_false_and_clear_is_idempotent() {
        let cache = ImageGpuCache::new();
        assert!(!cache.contains(ImageIndex(0)));
        assert!(cache.is_empty());
        let mut cache = cache;
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn store_push_yields_monotonic_image_indices() {
        let mut store = ImageStore::new();
        let img = Arc::new(DecodedImage::new(2, 2, ImageFormat::Rgba8, vec![0u8; 16]));
        let a = store.push(img.clone(), Placement::at(0, 0, 1, 2));
        let b = store.push(img, Placement::at(1, 0, 1, 2));
        assert_eq!(a.0, 0);
        assert_eq!(b.0, 1);
    }

    #[test]
    fn rgb_input_pads_to_rgba_length() {
        let rgb = vec![9u8; 3 * 4 * 4];
        let rgba = rgb_to_rgba(&rgb);
        assert_eq!(rgba.len(), 4 * 4 * 4);
    }
}
