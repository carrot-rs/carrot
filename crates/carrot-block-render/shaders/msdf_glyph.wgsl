// MSDF glyph reconstruction fragment shader.
//
// Samples the 3-channel MSDF atlas (R/G/B = X/Y/Z signed distances),
// derives the median distance, and produces a hard-edge alpha value.
// The supersampling comes for free because the GPU blends the quad
// fragment; subpixel AA lives in `fwidth`.
//
// Constants:
//   - DISTANCE_RANGE: the range of signed distances baked into the
//     atlas at generation time. 4.0 px is typical for text at
//     14–16 px. Keep in sync with the MSDF rasteriser.
//
// Uniforms:
//   - Atlas dimensions (used to translate the u32 atlas_x / atlas_y
//     cell-space coordinates into 0..1 texture UVs).
//   - Glyph colour (foreground).
//
// Expected vertex inputs:
//   - @location(0) atlas_uv: vec2<f32>    UV inside the MSDF atlas
//   - @location(1) glyph_color: vec4<f32> OKLCH quadruplet
//
// Expected bindings:
//   - @group(0) @binding(0) msdf_atlas: texture_2d<f32>
//   - @group(0) @binding(1) msdf_sampler: sampler

const DISTANCE_RANGE: f32 = 4.0;

struct VertexInput {
    @location(0) atlas_uv: vec2<f32>,
    @location(1) glyph_color: vec4<f32>,
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
}

@group(0) @binding(0) var msdf_atlas: texture_2d<f32>;
@group(0) @binding(1) var msdf_sampler: sampler;

// Median of three — the core of MSDF reconstruction. Sampling the
// three channels gives three signed-distance estimates; the median
// resolves edge crossings robustly (per Chlumsky 2015).
fn median3(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
}

// Convert a signed-distance sample at (u, v) to an alpha value.
// `fwidth` gives us the pixel-space derivative, which we use as
// the AA step width — no explicit supersampling needed.
fn sdf_alpha(distance: f32, screen_px: f32) -> f32 {
    // screen_px is the size of one pixel in atlas-distance units.
    // When distance is 0.5 we're on the edge; 0.5 - screen_px/2 is
    // the start of the anti-aliased band; 0.5 + screen_px/2 is the
    // end.
    let half_band = 0.5 * screen_px / DISTANCE_RANGE;
    return clamp(
        (distance - 0.5 + half_band) / (2.0 * half_band + 1e-6),
        0.0,
        1.0,
    );
}

@fragment
fn main(input: VertexInput) -> FragmentOutput {
    let sample = textureSample(msdf_atlas, msdf_sampler, input.atlas_uv);
    let signed_distance = median3(sample.r, sample.g, sample.b);
    // fwidth reports the magnitude of the derivative in screen-
    // space pixels; this gives us the AA band width automatically.
    let screen_px = fwidth(signed_distance);
    let alpha = sdf_alpha(signed_distance, screen_px);

    // Output pre-multiplied colour so the blend state can use
    // standard (ONE, ONE_MINUS_SRC_ALPHA).
    var out: FragmentOutput;
    let premul = vec4<f32>(input.glyph_color.rgb * alpha, alpha);
    out.color = premul;
    return out;
}
