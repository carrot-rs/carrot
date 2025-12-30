//! Text shaping via HarfRust.
//!
//! Wraps harfrust's `Shaper` + `UnicodeBuffer` → `GlyphBuffer`
//! pipeline in a Carrot-shaped API so consumers (the glyph
//! resolution pass in the MSDF atlas, carrot-cmdline's tree-sitter
//! runs) don't touch harfrust types directly.
//!
//! # Why a wrapper
//!
//! - Isolates harfrust version upgrades. If a future HarfBuzz spec
//!   bump changes semantics, we adapt this one module instead of
//!   every caller.
//! - Gives us a stable owned-output type ([`ShapedGlyph`]) — harfrust
//!   hands back a `GlyphBuffer` that keeps its `UnicodeBuffer`
//!   alive; we copy out `glyph_id + cluster + advance + offset` so
//!   the caller can drop the buffer immediately.
//! - Adds a cache hook in future work — the same `(text,
//!   font_hash, size)` tuple re-shapes the same result; a LRU
//!   wraps this wrapper in F.4 without touching the API.
//!
//! # Contract
//!
//! - `shape_run` accepts a UTF-8 string, runs a single shape pass,
//!   returns a `Vec<ShapedGlyph>`. Direction / script / language
//!   default to harfrust's auto-detect; explicit overrides via
//!   [`ShapeOptions`] are available when the caller knows better.
//! - No `unsafe`, no `unwrap`, no panic paths in the shaping call.
//! - Font loading errors (missing tables, corrupt data) surface as
//!   [`ShapingError::FontLoad`] — callers decide between fallback
//!   font vs. hard error.
//!
//! # Not in scope yet
//!
//! - Shape-cache (LRU) — ships alongside the MSDF atlas.
//! - Font-variant selection (bold / italic) — Layer 5 picks the
//!   right font family before calling into shaping.
//! - OpenType feature toggles (ligatures on/off, alt figures) —
//!   [`ShapeOptions::features`] forwards user-supplied features,
//!   but we don't synthesize them yet.

use harfrust::{FontRef, GlyphBuffer, Shaper, ShaperData, UnicodeBuffer};

/// One shaped glyph, owned. Fields match harfrust's
/// `GlyphInfo`/`GlyphPosition` pair, extracted so the caller can
/// drop the upstream buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph ID inside the font — the index the GPU atlas will use.
    pub glyph_id: u32,
    /// Cluster index — byte offset into the input string this glyph
    /// represents. Preserves grapheme-cluster information for
    /// complex-script caret placement.
    pub cluster: u32,
    /// Horizontal advance in font units.
    pub x_advance: i32,
    /// Vertical advance (non-zero for vertical scripts).
    pub y_advance: i32,
    /// Horizontal glyph offset from its logical position.
    pub x_offset: i32,
    /// Vertical glyph offset.
    pub y_offset: i32,
}

/// Optional per-shape options. Default: auto-detect direction,
/// script, and language; no OpenType feature overrides.
#[derive(Clone, Default)]
pub struct ShapeOptions<'a> {
    /// Explicit script override. `None` lets harfrust detect from
    /// the input text.
    pub script: Option<harfrust::Script>,
    /// Explicit direction override. `None` lets harfrust detect.
    pub direction: Option<harfrust::Direction>,
    /// BCP-47 language tag override.
    pub language: Option<harfrust::Language>,
    /// OpenType features (e.g. disable ligatures with `-liga`).
    pub features: &'a [harfrust::Feature],
}

/// Possible failures. The shape call itself cannot fail once a
/// context has been built — the error space is entirely around
/// font loading.
#[derive(Debug)]
pub enum ShapingError {
    /// Font bytes could not be parsed by `read-fonts`. Wraps the
    /// underlying read-fonts error via `Display`.
    FontLoad(String),
}

impl std::fmt::Display for ShapingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShapingError::FontLoad(msg) => write!(f, "font load failed: {msg}"),
        }
    }
}

impl std::error::Error for ShapingError {}

/// A loaded font ready for shaping. Owns the font bytes plus the
/// `ShaperData` derived from them — building that data is relatively
/// expensive and should happen once per font, not per shape call.
pub struct ShapingFont {
    bytes: Box<[u8]>,
    shaper_data: ShaperData,
}

impl ShapingFont {
    /// Load a font from in-memory bytes. The bytes are copied into
    /// the struct so the caller's buffer can be dropped.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ShapingError> {
        let owned: Box<[u8]> = bytes.to_vec().into_boxed_slice();
        // Parse once to validate; we'll re-parse lazily below. The
        // validation catches corrupt data at load time instead of
        // panicking on the first shape call.
        let _ = FontRef::new(&owned).map_err(|e| ShapingError::FontLoad(format!("{e:?}")))?;
        let font_ref =
            FontRef::new(&owned).map_err(|e| ShapingError::FontLoad(format!("{e:?}")))?;
        let shaper_data = ShaperData::new(&font_ref);
        Ok(Self {
            bytes: owned,
            shaper_data,
        })
    }

    /// Raw font bytes — sometimes callers also need a glyph
    /// rasteriser or a font-table query side-by-side.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Shape one run of text with the given font. Returns owned
/// [`ShapedGlyph`]s — the harfrust buffer is dropped before this
/// function returns.
pub fn shape_run(
    font: &ShapingFont,
    text: &str,
    options: ShapeOptions<'_>,
) -> Result<Vec<ShapedGlyph>, ShapingError> {
    let font_ref =
        FontRef::new(&font.bytes).map_err(|e| ShapingError::FontLoad(format!("{e:?}")))?;
    let shaper: Shaper<'_> = font.shaper_data.shaper(&font_ref).build();

    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    if let Some(script) = options.script {
        buffer.set_script(script);
    }
    if let Some(direction) = options.direction {
        buffer.set_direction(direction);
    }
    if let Some(ref lang) = options.language {
        buffer.set_language(lang.clone());
    }

    let shaped: GlyphBuffer = shaper.shape(buffer, options.features);
    let infos = shaped.glyph_infos();
    let positions = shaped.glyph_positions();

    let mut out = Vec::with_capacity(infos.len());
    for (info, pos) in infos.iter().zip(positions.iter()) {
        out.push(ShapedGlyph {
            glyph_id: info.glyph_id,
            cluster: info.cluster,
            x_advance: pos.x_advance,
            y_advance: pos.y_advance,
            x_offset: pos.x_offset,
            y_offset: pos.y_offset,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_font_bytes_return_error() {
        let bad: Vec<u8> = b"not a font".to_vec();
        let err = ShapingFont::from_bytes(&bad);
        assert!(err.is_err());
        let msg = format!("{}", err.err().unwrap());
        assert!(msg.contains("font load failed"));
    }

    #[test]
    fn empty_bytes_fail_to_load() {
        let err = ShapingFont::from_bytes(&[]);
        assert!(err.is_err());
    }

    #[test]
    fn shape_options_default_is_all_none() {
        let opts = ShapeOptions::default();
        assert!(opts.script.is_none());
        assert!(opts.direction.is_none());
        assert!(opts.language.is_none());
        assert!(opts.features.is_empty());
    }

    #[test]
    fn shaped_glyph_is_copy() {
        // Assert the struct remains cheap-to-pass.
        let g = ShapedGlyph {
            glyph_id: 1,
            cluster: 2,
            x_advance: 3,
            y_advance: 4,
            x_offset: 5,
            y_offset: 6,
        };
        let copy = g;
        assert_eq!(g, copy);
    }

    #[test]
    fn shaping_error_display_is_readable() {
        let err = ShapingError::FontLoad("oh no".into());
        assert!(format!("{err}").contains("oh no"));
    }

    // End-to-end shape tests need a font fixture. Will land when a
    // permissively-licensed test font (Inter, JetBrains Mono subset,
    // etc.) is vendored into crates/carrot-block-render/tests/fonts/.
    // Tracked as part of the golden-image test corpus (plan A8).
}
