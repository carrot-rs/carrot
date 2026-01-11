//! WGSL shader bundle.
//!
//! Shaders are stored as .wgsl files under `shaders/` and embedded
//! at compile time via `include_str!`. Keeping the text in a
//! separate file means the editor's WGSL syntax highlighting works
//! and the shader can be edited without touching Rust.
//!
//! The fragment shader is parsed at program start through inazuma's
//! shader registry (and validated via `naga` in tests). The WGSL
//! source itself makes no runtime assumptions and can be validated
//! independently.

/// MSDF glyph reconstruction fragment shader. Samples the three-
/// channel MSDF atlas, computes the median signed distance,
/// derives subpixel AA from `fwidth`, and outputs pre-multiplied
/// colour suitable for a standard `ONE` / `ONE_MINUS_SRC_ALPHA`
/// blend.
pub const MSDF_GLYPH_FRAGMENT: &str = include_str!("../shaders/msdf_glyph.wgsl");

/// Scrollback-search compute shader. Scans the GPU-resident cell
/// buffer for cells whose content matches a needle character and
/// writes matched offsets into an atomic counter + offset buffer.
/// Dispatched one workgroup per 1024 cells.
pub const SCROLLBACK_SEARCH_COMPUTE: &str = include_str!("../shaders/scrollback_search.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msdf_shader_is_non_empty() {
        assert!(!MSDF_GLYPH_FRAGMENT.trim().is_empty());
        // Sanity: the shader declares the entry point we document.
        assert!(MSDF_GLYPH_FRAGMENT.contains("@fragment"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("fn main"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("median3"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("textureSample"));
    }

    #[test]
    fn msdf_shader_references_three_channels() {
        // MSDF-correctness guard: we must sample all three of R, G, B.
        assert!(MSDF_GLYPH_FRAGMENT.contains("sample.r"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("sample.g"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("sample.b"));
    }

    #[test]
    fn msdf_shader_declares_distance_range_constant() {
        assert!(MSDF_GLYPH_FRAGMENT.contains("DISTANCE_RANGE"));
    }

    #[test]
    fn msdf_shader_premultiplies_alpha() {
        // Standard blend state assumes pre-multiplied alpha.
        assert!(MSDF_GLYPH_FRAGMENT.contains("glyph_color.rgb * alpha"));
    }

    #[test]
    fn msdf_shader_binds_atlas_and_sampler() {
        assert!(MSDF_GLYPH_FRAGMENT.contains("msdf_atlas: texture_2d"));
        assert!(MSDF_GLYPH_FRAGMENT.contains("msdf_sampler: sampler"));
    }

    #[test]
    fn msdf_shader_uses_fwidth_for_aa() {
        // Subpixel AA via fwidth rather than manual supersampling.
        assert!(MSDF_GLYPH_FRAGMENT.contains("fwidth"));
    }

    // ─── Scrollback search compute shader tests ─────────────────

    #[test]
    fn scrollback_search_shader_is_non_empty() {
        assert!(!SCROLLBACK_SEARCH_COMPUTE.trim().is_empty());
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("@compute"));
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("workgroup_size(1024)"));
    }

    #[test]
    fn scrollback_search_shader_atomics() {
        // Atomic counter for thread-safe match accumulation.
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("atomicAdd"));
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("atomicStore"));
    }

    #[test]
    fn scrollback_search_shader_bounds_check() {
        // Out-of-bounds invocations must return early; otherwise the
        // last workgroup overruns the cells buffer.
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("total_cells"));
    }

    #[test]
    fn scrollback_search_shader_masks_content_portion() {
        // Content_mask keeps us in sync with CONTENT_BITS in the
        // Cell packed layout.
        assert!(SCROLLBACK_SEARCH_COMPUTE.contains("CONTENT_MASK"));
    }
}
