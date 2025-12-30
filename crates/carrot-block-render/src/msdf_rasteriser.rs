//! MSDF bitmap rasteriser.
//!
//! Turns a binary / grayscale alpha bitmap into a 3-channel signed-
//! distance-field bitmap suitable for `MsdfAtlas::insert`. The
//! fragment shader under `shaders/msdf_glyph.wgsl` samples the
//! three channels, computes the median, and reconstructs sharp
//! edges at any scale.
//!
//! # Approach
//!
//! A full MSDF rasteriser derives per-channel signed distances
//! from the glyph's outline contours (Chlumsky, Shape-Driven
//! MSDFs, 2015). That requires the outline before flattening —
//! which swash doesn't expose today. Instead we take the
//! **alpha bitmap** swash already produces and compute the
//! **Euclidean signed distance** from every pixel to the nearest
//! edge, then replicate the same value into all three channels.
//!
//! The result is a degenerate MSDF (R == G == B) that the shader's
//! median collapses to a plain SDF. That reproduces the classical
//! SDF look — not full multi-channel sharpness at corners — but
//! keeps the full atlas / shader / GPU pipeline exercised end-to-end
//! with real data.
//!
//! When a true outline-driven MSDF rasteriser lands (by porting
//! `msdfgen` or switching to a font loader that exposes glyph
//! contours), it slots in behind the same [`rasterise_msdf`]
//! signature — callers don't change.
//!
//! # Perf
//!
//! The EDT is the naive O(w · h · k) brute-force sweep. For
//! typical glyph sizes (32×32 px, distance_range=4) this is ~130k
//! ops per glyph, well under a millisecond. A jump-flooding
//! implementation would cut that to O(w · h · log max(w,h)) but
//! the current workload doesn't need it.

/// Output bitmap size + bytes produced by [`rasterise_msdf`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterisedGlyph {
    pub width: u32,
    pub height: u32,
    /// 3 bytes per pixel (R, G, B MSDF channels), row-major.
    pub bytes: Vec<u8>,
}

/// Convert a binary / grayscale alpha bitmap into a 3-channel MSDF
/// bitmap.
///
/// - `alpha`: `width * height` bytes. Pixels with alpha ≥ 128 are
///   treated as **inside** the glyph, all others as outside.
/// - `distance_range`: the signed-distance range baked into the
///   output — must match the shader's `DISTANCE_RANGE` constant
///   (4.0 px by default). Pixels outside this range are clamped.
pub fn rasterise_msdf(
    alpha: &[u8],
    width: u32,
    height: u32,
    distance_range: f32,
) -> RasterisedGlyph {
    let w = width as usize;
    let h = height as usize;
    assert_eq!(
        alpha.len(),
        w * h,
        "alpha bitmap length must equal width * height",
    );
    let distance_range = distance_range.max(1.0);

    // Signed distance from every pixel to the nearest edge.
    let distances = signed_distance_field(alpha, w, h);

    // Map each signed distance to a single byte:
    // distance == 0   → 128
    // distance == +r  → 255  (well inside)
    // distance == -r  → 0    (well outside)
    let mut bytes = vec![0u8; w * h * 3];
    for (i, &d) in distances.iter().enumerate() {
        let norm = (d / distance_range).clamp(-1.0, 1.0);
        let byte = ((norm * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
        bytes[i * 3] = byte;
        bytes[i * 3 + 1] = byte;
        bytes[i * 3 + 2] = byte;
    }

    RasterisedGlyph {
        width,
        height,
        bytes,
    }
}

/// Brute-force signed Euclidean distance transform. Each pixel's
/// distance is positive inside the glyph, negative outside.
fn signed_distance_field(alpha: &[u8], w: usize, h: usize) -> Vec<f32> {
    // Mark "inside" pixels by alpha threshold. Edge pixels are the
    // boundary between inside and outside — we collect their
    // sub-pixel centres.
    let inside: Vec<bool> = alpha.iter().map(|&a| a >= 128).collect();
    let edges = edge_pixels(&inside, w, h);

    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let ix = y * w + x;
            let min_sq = edges
                .iter()
                .map(|&(ex, ey)| {
                    let dx = x as f32 + 0.5 - (ex + 0.5);
                    let dy = y as f32 + 0.5 - (ey + 0.5);
                    dx * dx + dy * dy
                })
                .fold(f32::INFINITY, f32::min);
            let dist = min_sq.sqrt();
            out[ix] = if inside[ix] { dist } else { -dist };
        }
    }
    out
}

/// Collect sub-pixel centres of every edge pixel — a pixel is on
/// the edge when any 4-neighbour differs from it.
fn edge_pixels(inside: &[bool], w: usize, h: usize) -> Vec<(f32, f32)> {
    let mut edges = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let ix = y * w + x;
            let me = inside[ix];
            let neighbours = [
                (x.checked_sub(1).map(|nx| (nx, y))),
                (if x + 1 < w { Some((x + 1, y)) } else { None }),
                (y.checked_sub(1).map(|ny| (x, ny))),
                (if y + 1 < h { Some((x, y + 1)) } else { None }),
            ];
            for n in neighbours.into_iter().flatten() {
                if inside[n.1 * w + n.0] != me {
                    edges.push((x as f32, y as f32));
                    break;
                }
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small 8×8 bitmap: a centred 4×4 filled square.
    fn centred_square() -> (Vec<u8>, u32, u32) {
        let w = 8u32;
        let h = 8u32;
        let mut alpha = vec![0u8; (w * h) as usize];
        for y in 2..6 {
            for x in 2..6 {
                alpha[(y * w + x) as usize] = 255;
            }
        }
        (alpha, w, h)
    }

    #[test]
    fn rasterise_produces_correct_bitmap_size() {
        let (alpha, w, h) = centred_square();
        let out = rasterise_msdf(&alpha, w, h, 4.0);
        assert_eq!(out.width, w);
        assert_eq!(out.height, h);
        assert_eq!(out.bytes.len(), (w * h * 3) as usize);
    }

    #[test]
    fn rasterise_centre_is_brightest() {
        let (alpha, w, h) = centred_square();
        let out = rasterise_msdf(&alpha, w, h, 4.0);
        // Pixel (3,3) is inside the square, far from edges.
        let inside_ix = (3 * w + 3) * 3;
        let inside_byte = out.bytes[inside_ix as usize];
        // Pixel (0,0) is outside the square.
        let outside_ix = 0;
        let outside_byte = out.bytes[outside_ix];
        assert!(
            inside_byte > 128,
            "interior should be > 128, got {inside_byte}"
        );
        assert!(
            outside_byte < 128,
            "exterior should be < 128, got {outside_byte}"
        );
    }

    #[test]
    fn rasterise_channels_are_equal_in_grayscale_mode() {
        // Our rasteriser replicates the alpha-derived SDF into all
        // three channels — downstream consumers that expect equal
        // R/G/B for the grayscale path rely on this.
        let (alpha, w, h) = centred_square();
        let out = rasterise_msdf(&alpha, w, h, 4.0);
        for px in out.bytes.chunks_exact(3) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
    }

    #[test]
    fn rasterise_fully_transparent_bitmap_maps_to_zero_distance() {
        // All-zero alpha: every pixel is "outside" with no edges.
        // The EDT returns inf, which clamps to the minimum byte.
        let w = 4u32;
        let h = 4u32;
        let alpha = vec![0u8; (w * h) as usize];
        let out = rasterise_msdf(&alpha, w, h, 4.0);
        // Because there are no edges, every distance is -inf which
        // clamps to -distance_range, mapping to byte 0.
        for b in &out.bytes {
            assert_eq!(*b, 0);
        }
    }

    #[test]
    fn rasterise_fully_opaque_bitmap_maps_to_saturated_inside() {
        let w = 4u32;
        let h = 4u32;
        let alpha = vec![255u8; (w * h) as usize];
        let out = rasterise_msdf(&alpha, w, h, 4.0);
        // No edges → +inf → clamps to +distance_range → byte 255.
        for b in &out.bytes {
            assert_eq!(*b, 255);
        }
    }

    #[test]
    fn rasterise_feeds_msdf_atlas_round_trip() {
        use crate::msdf_atlas::{GlyphKey, MsdfAtlas};
        use std::num::NonZeroU32;

        let (alpha, w, h) = centred_square();
        let rasterised = rasterise_msdf(&alpha, w, h, 4.0);
        let mut atlas = MsdfAtlas::new(NonZeroU32::new(64).unwrap(), NonZeroU32::new(64).unwrap());
        let key = GlyphKey::new(0, 65, 16);
        let glyph = atlas
            .insert(key, w, h, 0.0, h as f32, w as f32, &rasterised.bytes)
            .expect("atlas insert");
        assert_eq!(glyph.atlas_w, w);
        assert_eq!(glyph.atlas_h, h);
        assert_eq!(atlas.get(key), Some(glyph));
    }

    #[test]
    fn distance_range_clamps_large_interior_distance() {
        // 11×11 bitmap with a 7×7 filled square in the middle. The
        // very centre pixel is 3 px from any edge — with a tight
        // distance_range of 0.5 the normalised value saturates to
        // +1.0, byte 255.
        let w = 11u32;
        let h = 11u32;
        let mut alpha = vec![0u8; (w * h) as usize];
        for y in 2..9 {
            for x in 2..9 {
                alpha[(y * w + x) as usize] = 255;
            }
        }
        let out = rasterise_msdf(&alpha, w, h, 0.5);
        let centre_ix = (5 * w + 5) * 3;
        assert!(out.bytes[centre_ix as usize] >= 200);
    }

    #[test]
    #[should_panic(expected = "alpha bitmap length must equal width * height")]
    fn rasterise_panics_on_size_mismatch() {
        let alpha = vec![0u8; 10];
        rasterise_msdf(&alpha, 4, 4, 4.0);
    }
}
