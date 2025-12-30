//! 8-byte packed terminal cell.
//!
//! Layout (little-endian u64):
//!
//! ```text
//! bits 0..21   (21)  content — codepoint | grapheme_index | image_index | shaped_run_index
//! bits 21..24  (3)   tag     — CellTag enum
//! bits 24..40  (16)  style_id — index into CellStyleAtlas
//! bits 40..41  (1)   dirty
//! bits 41..42  (1)   wrap_continuation
//! bits 42..43  (1)   protected
//! bits 43..44  (1)   hyperlink_ref
//! bits 44..64  (20)  reserved
//! ```

use std::fmt;

/// Maximum valid codepoint (Unicode U+10FFFF fits in 21 bits).
const CONTENT_MASK: u64 = 0x001F_FFFF;
const CONTENT_BITS: u32 = 21;

const TAG_MASK: u64 = 0b111 << CONTENT_BITS;
const TAG_BITS: u32 = 3;

const STYLE_SHIFT: u32 = CONTENT_BITS + TAG_BITS; // 24
const STYLE_MASK: u64 = 0xFFFF << STYLE_SHIFT;

const FLAGS_SHIFT: u32 = STYLE_SHIFT + 16; // 40
const DIRTY_BIT: u64 = 1 << FLAGS_SHIFT;
const WRAP_CONTINUATION_BIT: u64 = 1 << (FLAGS_SHIFT + 1);
const PROTECTED_BIT: u64 = 1 << (FLAGS_SHIFT + 2);
const HYPERLINK_BIT: u64 = 1 << (FLAGS_SHIFT + 3);

/// Discriminant carried by the top 3 bits of [`Cell`]'s content word.
///
/// Spike note: the enum order is stable — adding a variant means bumping a
/// reserved slot, never renumbering. Layer 4 renderer switches on this tag.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellTag {
    /// Pure ASCII single-byte cell. Fast path.
    Ascii = 0,
    /// Any valid Unicode scalar (BMP + supplementary planes).
    Codepoint = 1,
    /// Cell content is an index into the block's grapheme table.
    Grapheme = 2,
    /// Second cell of a wide (double-width) character. Content = 0.
    Wide2nd = 3,
    /// Index into the block's image store (inline-image cell).
    Image = 4,
    /// Index into the block's shaped-run cache (complex-script run).
    ShapedRun = 5,
    /// Plugin-provided render hook; content = plugin-specific index.
    CustomRender = 6,
    /// Reserved — do not construct.
    Reserved = 7,
}

impl CellTag {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b111 {
            0 => Self::Ascii,
            1 => Self::Codepoint,
            2 => Self::Grapheme,
            3 => Self::Wide2nd,
            4 => Self::Image,
            5 => Self::ShapedRun,
            6 => Self::CustomRender,
            _ => Self::Reserved,
        }
    }
}

/// Per-cell bitflags accessible via [`Cell::flags`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellFlags(u8);

impl CellFlags {
    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn dirty(self) -> bool {
        self.0 & 1 != 0
    }
    pub const fn wrap_continuation(self) -> bool {
        self.0 & 0b10 != 0
    }
    pub const fn protected(self) -> bool {
        self.0 & 0b100 != 0
    }
    pub const fn hyperlink(self) -> bool {
        self.0 & 0b1000 != 0
    }
}

/// Newtype indices for the out-of-band content tables referenced by non-ASCII cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellStyleId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GraphemeIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageIndex(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapedRunIndex(pub u32);

/// 8-byte packed terminal cell. See module-level docs for bit layout.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell(u64);

impl Cell {
    /// An empty ASCII space with no style.
    pub const EMPTY: Cell = Cell(0);

    /// Construct an ASCII cell with the given byte and style id.
    pub const fn ascii(c: u8, style: CellStyleId) -> Self {
        let content = c as u64;
        let tag = (CellTag::Ascii as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(content | tag | style)
    }

    /// Construct a codepoint cell.
    pub const fn codepoint(c: char, style: CellStyleId) -> Self {
        let content = (c as u32 as u64) & CONTENT_MASK;
        let tag = (CellTag::Codepoint as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(content | tag | style)
    }

    /// Construct a grapheme cell referring to the out-of-band grapheme table.
    pub const fn grapheme(idx: GraphemeIndex, style: CellStyleId) -> Self {
        let content = (idx.0 as u64) & CONTENT_MASK;
        let tag = (CellTag::Grapheme as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(content | tag | style)
    }

    /// Second cell of a wide (double-width) character.
    pub const fn wide_2nd(style: CellStyleId) -> Self {
        let tag = (CellTag::Wide2nd as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(tag | style)
    }

    /// Construct an image cell.
    pub const fn image(idx: ImageIndex, style: CellStyleId) -> Self {
        let content = (idx.0 as u64) & CONTENT_MASK;
        let tag = (CellTag::Image as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(content | tag | style)
    }

    /// Construct a shaped-run cell.
    pub const fn shaped_run(idx: ShapedRunIndex, style: CellStyleId) -> Self {
        let content = (idx.0 as u64) & CONTENT_MASK;
        let tag = (CellTag::ShapedRun as u64) << CONTENT_BITS;
        let style = (style.0 as u64) << STYLE_SHIFT;
        Cell(content | tag | style)
    }

    /// Tag — which interpretation applies to [`Self::content`].
    pub fn tag(self) -> CellTag {
        let bits = ((self.0 & TAG_MASK) >> CONTENT_BITS) as u8;
        CellTag::from_bits(bits)
    }

    /// Raw content word (21-bit). Interpret with respect to [`Self::tag`].
    pub const fn content(self) -> u32 {
        (self.0 & CONTENT_MASK) as u32
    }

    /// CellStyle id — index into the owning block's [`crate::CellStyleAtlas`].
    pub const fn style(self) -> CellStyleId {
        CellStyleId(((self.0 & STYLE_MASK) >> STYLE_SHIFT) as u16)
    }

    /// Active flag bits.
    pub const fn flags(self) -> CellFlags {
        let mut bits = 0u8;
        if self.0 & DIRTY_BIT != 0 {
            bits |= 1;
        }
        if self.0 & WRAP_CONTINUATION_BIT != 0 {
            bits |= 0b10;
        }
        if self.0 & PROTECTED_BIT != 0 {
            bits |= 0b100;
        }
        if self.0 & HYPERLINK_BIT != 0 {
            bits |= 0b1000;
        }
        CellFlags(bits)
    }

    /// Returns a copy with the `dirty` bit set to `on`.
    pub const fn with_dirty(self, on: bool) -> Self {
        if on {
            Cell(self.0 | DIRTY_BIT)
        } else {
            Cell(self.0 & !DIRTY_BIT)
        }
    }

    /// Returns a copy with the `wrap_continuation` bit set to `on`.
    pub const fn with_wrap_continuation(self, on: bool) -> Self {
        if on {
            Cell(self.0 | WRAP_CONTINUATION_BIT)
        } else {
            Cell(self.0 & !WRAP_CONTINUATION_BIT)
        }
    }

    /// Returns a copy with the `protected` bit set to `on`.
    pub const fn with_protected(self, on: bool) -> Self {
        if on {
            Cell(self.0 | PROTECTED_BIT)
        } else {
            Cell(self.0 & !PROTECTED_BIT)
        }
    }

    /// Returns a copy with the `hyperlink` bit set to `on`.
    pub const fn with_hyperlink(self, on: bool) -> Self {
        if on {
            Cell(self.0 | HYPERLINK_BIT)
        } else {
            Cell(self.0 & !HYPERLINK_BIT)
        }
    }

    /// Raw u64 view — for GPU upload and debugging.
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// Reconstruct from raw u64 bits.
    pub const fn from_bits(bits: u64) -> Self {
        Cell(bits)
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::EMPTY
    }
}

impl fmt::Debug for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cell")
            .field("tag", &self.tag())
            .field("content", &self.content())
            .field("style", &self.style())
            .field("flags", &self.flags())
            .finish()
    }
}

const _: () = {
    assert!(std::mem::size_of::<Cell>() == 8, "Cell must stay 8 bytes");
    assert!(std::mem::align_of::<Cell>() == 8);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_is_exactly_8_bytes() {
        assert_eq!(std::mem::size_of::<Cell>(), 8);
        assert_eq!(std::mem::align_of::<Cell>(), 8);
    }

    #[test]
    fn ascii_roundtrip() {
        let c = Cell::ascii(b'A', CellStyleId(42));
        assert_eq!(c.tag(), CellTag::Ascii);
        assert_eq!(c.content(), b'A' as u32);
        assert_eq!(c.style().0, 42);
    }

    #[test]
    fn codepoint_roundtrip_bmp() {
        let c = Cell::codepoint('€', CellStyleId(1));
        assert_eq!(c.tag(), CellTag::Codepoint);
        assert_eq!(c.content(), '€' as u32);
    }

    #[test]
    fn codepoint_roundtrip_supplementary() {
        // U+1F600 grinning face — requires more than 16 bits.
        let c = Cell::codepoint('\u{1F600}', CellStyleId(7));
        assert_eq!(c.content(), 0x1F600);
    }

    #[test]
    fn grapheme_roundtrip() {
        let c = Cell::grapheme(GraphemeIndex(12345), CellStyleId(3));
        assert_eq!(c.tag(), CellTag::Grapheme);
        assert_eq!(c.content(), 12345);
    }

    #[test]
    fn wide_2nd_has_zero_content() {
        let c = Cell::wide_2nd(CellStyleId(5));
        assert_eq!(c.tag(), CellTag::Wide2nd);
        assert_eq!(c.content(), 0);
        assert_eq!(c.style().0, 5);
    }

    #[test]
    fn flag_bits_toggle_independently() {
        let base = Cell::ascii(b'x', CellStyleId(0));
        assert!(!base.flags().dirty());
        let dirty = base.with_dirty(true);
        assert!(dirty.flags().dirty());
        assert!(!dirty.flags().wrap_continuation());
        let both = dirty.with_wrap_continuation(true);
        assert!(both.flags().dirty());
        assert!(both.flags().wrap_continuation());
        let cleared = both.with_dirty(false);
        assert!(!cleared.flags().dirty());
        assert!(cleared.flags().wrap_continuation());
    }

    #[test]
    fn max_content_fits() {
        let idx = GraphemeIndex((1 << CONTENT_BITS) - 1);
        let c = Cell::grapheme(idx, CellStyleId(0));
        assert_eq!(c.content(), (1 << CONTENT_BITS) - 1);
    }
}
