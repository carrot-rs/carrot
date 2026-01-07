//! GPU upload path for the MSDF atlas.
//!
//! The CPU atlas keeps 3 bytes per pixel (MSDF R/G/B). `wgpu` exposes
//! no standard RGB8 texture format, so this module pads to RGBA8 on
//! upload and writes into a `wgpu::TextureFormat::Rgba8Unorm` surface.
//! That matches the MSDF fragment shader — it samples
//! all four channels and uses the RGB lanes for the distance field
//! (alpha is ignored by the shader and set to `0xFF` here to keep the
//! texture debugger happy).
//!
//! Only dirty sub-rects round-trip to the GPU per frame. The atlas
//! tracks them during `insert` / `clear`; [`MsdfGpuAtlas::upload`]
//! drains them via [`super::msdf_atlas::MsdfAtlas::take_pending_uploads`]
//! and issues one `write_texture` call per rect.

use wgpu::{
    Device, Extent3d, Origin3d, Queue, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

use crate::msdf_atlas::{DirtyRect, MsdfAtlas};

/// GPU-side companion to [`MsdfAtlas`]. Owns the `wgpu::Texture` and
/// knows how to stream dirty sub-rects from the CPU atlas.
pub struct MsdfGpuAtlas {
    texture: Texture,
    view: TextureView,
    width: u32,
    height: u32,
}

impl MsdfGpuAtlas {
    /// Allocate a `width × height` `Rgba8Unorm` texture with
    /// `TEXTURE_BINDING | COPY_DST` usage. Dimensions must match the
    /// CPU [`MsdfAtlas`] the uploader will feed from.
    pub fn new(device: &Device, width: u32, height: u32, label: Option<&str>) -> Self {
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
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width,
            height,
        }
    }

    /// The backing texture — used by pipeline bind-group setup.
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    /// Shader-ready view of the texture.
    pub fn view(&self) -> &TextureView {
        &self.view
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Drain all dirty rects from `atlas` and upload them to the GPU.
    /// Each rect becomes one `queue.write_texture` call.
    ///
    /// Returns the number of rects uploaded.
    pub fn upload(&self, queue: &Queue, atlas: &mut MsdfAtlas) -> usize {
        let rects = atlas.take_pending_uploads();
        let count = rects.len();
        for rect in rects {
            let rgb = atlas.extract_rect_rgb(rect);
            let rgba = rgb_to_rgba(&rgb);
            self.upload_rect(queue, rect, &rgba);
        }
        count
    }

    fn upload_rect(&self, queue: &Queue, rect: DirtyRect, rgba: &[u8]) {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: rect.x,
                    y: rect.y,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            rgba,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rect.w * 4),
                rows_per_image: Some(rect.h),
            },
            Extent3d {
                width: rect.w,
                height: rect.h,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Pad an `R,G,B, R,G,B, ...` buffer to `R,G,B,0xFF, R,G,B,0xFF, ...`.
/// Alpha is hard-coded to `0xFF`; MSDF reconstruction in the fragment
/// shader doesn't use it, but leaving it `0xFF` makes GPU captures
/// readable in RenderDoc / Xcode.
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    debug_assert!(
        rgb.len().is_multiple_of(3),
        "RGB input length must be a multiple of 3"
    );
    let pixel_count = rgb.len() / 3;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in rgb.chunks_exact(3) {
        rgba.extend_from_slice(chunk);
        rgba.push(0xFF);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_to_rgba_pads_alpha_to_ff() {
        let rgb = vec![1, 2, 3, 4, 5, 6];
        let rgba = rgb_to_rgba(&rgb);
        assert_eq!(rgba, vec![1, 2, 3, 0xFF, 4, 5, 6, 0xFF]);
    }

    #[test]
    fn rgb_to_rgba_on_empty_is_empty() {
        assert!(rgb_to_rgba(&[]).is_empty());
    }

    #[test]
    fn extract_rect_matches_inserted_bitmap() {
        use std::num::NonZeroU32;
        let mut atlas = MsdfAtlas::new(NonZeroU32::new(16).unwrap(), NonZeroU32::new(16).unwrap());
        let glyph_bitmap = vec![
            1, 2, 3, 4, 5, 6, //
            7, 8, 9, 10, 11, 12,
        ];
        // 2 cols × 2 rows glyph → 12 bytes of RGB.
        let key = crate::msdf_atlas::GlyphKey::new(0, 1, 12);
        let record = atlas
            .insert(key, 2, 2, 0.0, 0.0, 2.0, &glyph_bitmap)
            .expect("insert");
        let rects = atlas.take_pending_uploads();
        assert_eq!(rects.len(), 1);
        let rect = rects[0];
        assert_eq!(rect.w, 2);
        assert_eq!(rect.h, 2);
        assert_eq!(rect.x, record.atlas_x);
        assert_eq!(rect.y, record.atlas_y);
        let extracted = atlas.extract_rect_rgb(rect);
        assert_eq!(extracted, glyph_bitmap);
    }

    #[test]
    fn atlas_clear_queues_full_rect_and_drops_previous() {
        use std::num::NonZeroU32;
        let mut atlas = MsdfAtlas::new(NonZeroU32::new(32).unwrap(), NonZeroU32::new(32).unwrap());
        let pixels = vec![0u8; 3 * 4 * 4];
        atlas
            .insert(
                crate::msdf_atlas::GlyphKey::new(0, 1, 12),
                4,
                4,
                0.0,
                0.0,
                4.0,
                &pixels,
            )
            .unwrap();
        atlas.clear();
        let rects = atlas.take_pending_uploads();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], DirtyRect::new(0, 0, 32, 32));
    }

    #[test]
    fn pending_flag_reflects_queue_state() {
        use std::num::NonZeroU32;
        let mut atlas = MsdfAtlas::new(NonZeroU32::new(8).unwrap(), NonZeroU32::new(8).unwrap());
        assert!(!atlas.has_pending_uploads());
        atlas
            .insert(
                crate::msdf_atlas::GlyphKey::new(0, 1, 12),
                2,
                2,
                0.0,
                0.0,
                2.0,
                &[0u8; 12],
            )
            .unwrap();
        assert!(atlas.has_pending_uploads());
        let drained = atlas.take_pending_uploads();
        assert_eq!(drained.len(), 1);
        assert!(!atlas.has_pending_uploads());
    }
}
