//! Interned style table.
//!
//! Cells carry only a `u16` style id; the resolved colors + flags live here.
//! Every unique `(fg, bg, flags, font_override)` tuple lands in the atlas
//! once. The atlas is cheap to upload to the GPU as a storage buffer;
//! cell rendering indexes into it by id.

use inazuma_collections::FxHashMap;

use crate::cell::CellStyleId;
use crate::color::Color;
use crate::hyperlink::HyperlinkId;

/// Bitflags for per-style rendering hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellStyleFlags(pub u16);

impl CellStyleFlags {
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const STRIKETHROUGH: Self = Self(1 << 3);
    pub const BLINK: Self = Self(1 << 4);
    pub const REVERSE: Self = Self(1 << 5);
    pub const DIM: Self = Self(1 << 6);
    pub const HIDDEN: Self = Self(1 << 7);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn insert(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

/// A terminal-cell style — unresolved colors + attributes.
///
/// Distinct from `inazuma::Style` (UI layout). Cells carry a
/// [`CellStyleId`] into a [`CellStyleAtlas`] which stores these.
///
/// Colors are stored as [`Color`] tags (default / named / indexed / rgb).
/// The concrete Oklch values are resolved by the render layer against the
/// active theme's terminal palette, so a theme switch re-colours every
/// existing cell without touching the grid data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub fg: Color,
    pub bg: Color,
    pub underline_color: Option<Color>,
    pub flags: CellStyleFlags,
    /// OSC 8 hyperlink reference. `HyperlinkId::NONE` for plain
    /// styled cells (the common case). Cells in the same hyperlink
    /// span share a `CellStyleId` and therefore this field — the URL
    /// itself lives in the per-block [`crate::HyperlinkStore`].
    pub hyperlink: HyperlinkId,
}

impl CellStyle {
    /// Default style: theme foreground on theme background, no flags.
    pub const DEFAULT: CellStyle = CellStyle {
        fg: Color::Default,
        bg: Color::Default,
        underline_color: None,
        flags: CellStyleFlags::empty(),
        hyperlink: HyperlinkId::NONE,
    };
}

impl Default for CellStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Interned cell styles for one block. Cells hold a [`CellStyleId`]
/// into this table.
///
/// The atlas is append-only during a block's active lifetime; at `freeze()`
/// time, Carrot's Layer-2 builder merges per-block atlases into a view-wide
/// atlas if cross-block dedup is worthwhile (decision deferred).
#[derive(Debug, Clone, Default)]
pub struct CellStyleAtlas {
    styles: Vec<CellStyle>,
    index: FxHashMap<CellStyle, u16>,
}

impl CellStyleAtlas {
    /// Construct a fresh atlas with the default style at id `0`.
    pub fn new() -> Self {
        let mut atlas = Self {
            styles: Vec::new(),
            index: FxHashMap::default(),
        };
        let _ = atlas.intern(CellStyle::DEFAULT);
        atlas
    }

    /// Intern a style, returning its id. Identical styles share an id.
    ///
    /// Saturates at `u16::MAX` styles — beyond that the atlas returns
    /// `CellStyleId(u16::MAX)` for any new lookup. 65 k unique styles per
    /// block far exceeds any realistic workload.
    pub fn intern(&mut self, style: CellStyle) -> CellStyleId {
        if let Some(&id) = self.index.get(&style) {
            return CellStyleId(id);
        }
        if self.styles.len() >= u16::MAX as usize {
            return CellStyleId(u16::MAX);
        }
        let id = self.styles.len() as u16;
        self.styles.push(style);
        self.index.insert(style, id);
        CellStyleId(id)
    }

    /// Look up a style by id. Falls back to [`CellStyle::DEFAULT`] on
    /// out-of-range ids — the renderer should treat an invalid id as a
    /// soft error, not a panic.
    pub fn get(&self, id: CellStyleId) -> CellStyle {
        self.styles
            .get(id.0 as usize)
            .copied()
            .unwrap_or(CellStyle::DEFAULT)
    }

    /// Number of distinct styles stored.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// Raw view of the interned styles — for bulk GPU upload.
    pub fn as_slice(&self) -> &[CellStyle] {
        &self.styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::NamedColor;

    #[test]
    fn default_style_is_id_zero() {
        let atlas = CellStyleAtlas::new();
        assert_eq!(atlas.len(), 1);
        assert_eq!(atlas.get(CellStyleId(0)), CellStyle::DEFAULT);
    }

    #[test]
    fn identical_styles_dedup() {
        let mut atlas = CellStyleAtlas::new();
        let s = CellStyle {
            fg: Color::Named(NamedColor::Cyan),
            ..CellStyle::DEFAULT
        };
        let a = atlas.intern(s);
        let b = atlas.intern(s);
        assert_eq!(a, b);
        assert_eq!(atlas.len(), 2);
    }

    #[test]
    fn flag_delta_creates_new_entry() {
        let mut atlas = CellStyleAtlas::new();
        let bold = CellStyle {
            flags: CellStyleFlags::BOLD,
            ..CellStyle::DEFAULT
        };
        let italic = CellStyle {
            flags: CellStyleFlags::ITALIC,
            ..CellStyle::DEFAULT
        };
        let a = atlas.intern(bold);
        let b = atlas.intern(italic);
        assert_ne!(a, b);
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn out_of_range_id_returns_default() {
        let atlas = CellStyleAtlas::new();
        assert_eq!(atlas.get(CellStyleId(42)), CellStyle::DEFAULT);
    }
}
