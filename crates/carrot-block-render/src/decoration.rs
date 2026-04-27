//! Decoration primitives.
//!
//! Underline and strikethrough are thin horizontal rules drawn on
//! top of the glyph pass. Bold / italic / reverse-video are style
//! modifiers that the glyph resolver must pick up from the
//! `CellStyleFlags` — this module provides the helpers to extract the
//! appropriate font variant and decoration rects.
//!
//! Reverse-video (SGR 7) is a style concept, not a decoration rect:
//! it swaps fg / bg at resolution time. This module's
//! [`apply_reverse_video`] does that swap for callers that need it
//! before glyph lookup.
//!
//! Hidden (SGR 8) and blink (SGR 5) are animation concerns — the
//! consumer handles timing and skips emission. We just surface the
//! flag predicates for convenience.

use carrot_grid::{CellStyle, CellStyleFlags, Color};

use crate::palette::{DefaultSlot, TerminalPalette};

/// Decoration rect emitted per styled cell. Cell-local coordinates —
/// caller adds block origin + cell origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecorationDraw {
    pub kind: DecorationKind,
    /// Distance from the top of the cell, in cell-height fractions.
    /// 0.0 = top, 1.0 = bottom.
    pub y_offset_frac: f32,
    /// Height in cell-height fractions.
    pub height_frac: f32,
    /// Color for the rule, resolved against the terminal palette.
    pub color: [f32; 4],
}

/// What kind of rule this decoration draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationKind {
    /// Thin rule along the bottom of the cell (SGR 4).
    Underline,
    /// Thin rule roughly through the character midline (SGR 9).
    Strikethrough,
}

/// Emit every decoration rule the style asks for. Returns 0, 1, or
/// 2 draws (underline + strikethrough can stack).
///
/// `underline_color_override` lets the caller force the underline
/// color (some terminals paint underlines with a theme-accent instead
/// of the foreground). Pass `None` to use the style's own underline
/// color, and fall back to the style's foreground when neither is set.
pub fn render_decorations(
    style: &CellStyle,
    palette: &TerminalPalette,
    underline_color_override: Option<Color>,
) -> Vec<DecorationDraw> {
    let mut out = Vec::new();

    let bold = style.flags.contains(CellStyleFlags::BOLD);
    if style.flags.contains(CellStyleFlags::UNDERLINE) {
        let color_tag = underline_color_override
            .or(style.underline_color)
            .unwrap_or(style.fg);
        // Promote bold + base ANSI to the bright variant — keep the
        // underline color in lock-step with the glyph color so a
        // `\e[1;31;4m` segment doesn't show a muted underline below
        // a bright-red word.
        out.push(DecorationDraw {
            kind: DecorationKind::Underline,
            y_offset_frac: 0.90,
            height_frac: 0.06,
            color: palette.resolve_styled(color_tag, DefaultSlot::Foreground, bold),
        });
    }

    if style.flags.contains(CellStyleFlags::STRIKETHROUGH) {
        out.push(DecorationDraw {
            kind: DecorationKind::Strikethrough,
            y_offset_frac: 0.50,
            height_frac: 0.06,
            color: palette.resolve_styled(style.fg, DefaultSlot::Foreground, bold),
        });
    }

    out
}

/// Apply SGR 7 (reverse video) by swapping fg / bg on a copy of
/// `style`. Used at glyph-resolution time so the rest of the
/// pipeline sees a plain style with the correct effective colors.
pub fn apply_reverse_video(style: &CellStyle) -> CellStyle {
    if style.flags.contains(CellStyleFlags::REVERSE) {
        let mut swapped = *style;
        std::mem::swap(&mut swapped.fg, &mut swapped.bg);
        // Underline keeps its own colour; swap doesn't touch it.
        swapped
    } else {
        *style
    }
}

/// Font-weight + style selectors derived from `CellStyleFlags`. The
/// consumer's glyph resolver uses these to pick a font variant —
/// e.g. inazuma's `Font` struct has `weight` and `style` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FontVariantSelector {
    pub bold: bool,
    pub italic: bool,
    /// SGR 2 (dim / faint). Usually rendered by attenuating the
    /// alpha channel of `fg`; the consumer decides how.
    pub dim: bool,
}

impl FontVariantSelector {
    pub fn from_flags(flags: CellStyleFlags) -> Self {
        Self {
            bold: flags.contains(CellStyleFlags::BOLD),
            italic: flags.contains(CellStyleFlags::ITALIC),
            dim: flags.contains(CellStyleFlags::DIM),
        }
    }
}

/// Animation-only flag predicates.
#[derive(Debug, Clone, Copy)]
pub struct AnimationFlags {
    pub blink: bool,
    pub hidden: bool,
}

impl AnimationFlags {
    pub fn from_flags(flags: CellStyleFlags) -> Self {
        Self {
            blink: flags.contains(CellStyleFlags::BLINK),
            hidden: flags.contains(CellStyleFlags::HIDDEN),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_grid::NamedColor;

    fn style_with(flags: CellStyleFlags) -> CellStyle {
        CellStyle {
            flags,
            ..CellStyle::DEFAULT
        }
    }

    #[test]
    fn no_flags_produces_no_decorations() {
        let p = TerminalPalette::CARROT_DARK;
        let draws = render_decorations(&style_with(CellStyleFlags::empty()), &p, None);
        assert!(draws.is_empty());
    }

    #[test]
    fn underline_flag_emits_bottom_rule() {
        let p = TerminalPalette::CARROT_DARK;
        let draws = render_decorations(&style_with(CellStyleFlags::UNDERLINE), &p, None);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].kind, DecorationKind::Underline);
        assert!(draws[0].y_offset_frac > 0.85);
        assert!(draws[0].height_frac < 0.15);
    }

    #[test]
    fn strikethrough_flag_emits_midline_rule() {
        let p = TerminalPalette::CARROT_DARK;
        let draws = render_decorations(&style_with(CellStyleFlags::STRIKETHROUGH), &p, None);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].kind, DecorationKind::Strikethrough);
        assert!(draws[0].y_offset_frac > 0.4 && draws[0].y_offset_frac < 0.6);
    }

    #[test]
    fn underline_plus_strikethrough_stack() {
        let p = TerminalPalette::CARROT_DARK;
        let combined = CellStyleFlags::UNDERLINE.insert(CellStyleFlags::STRIKETHROUGH);
        let draws = render_decorations(&style_with(combined), &p, None);
        assert_eq!(draws.len(), 2);
        let kinds: Vec<_> = draws.iter().map(|d| d.kind).collect();
        assert!(kinds.contains(&DecorationKind::Underline));
        assert!(kinds.contains(&DecorationKind::Strikethrough));
    }

    #[test]
    fn underline_uses_explicit_color_override() {
        let p = TerminalPalette::CARROT_DARK;
        let override_color = Color::Rgb(128, 64, 255);
        let draws = render_decorations(
            &style_with(CellStyleFlags::UNDERLINE),
            &p,
            Some(override_color),
        );
        assert_eq!(
            draws[0].color,
            p.resolve(override_color, DefaultSlot::Foreground),
        );
    }

    #[test]
    fn underline_uses_style_underline_color_when_no_override() {
        let p = TerminalPalette::CARROT_DARK;
        let style = CellStyle {
            flags: CellStyleFlags::UNDERLINE,
            underline_color: Some(Color::Named(NamedColor::Red)),
            ..CellStyle::DEFAULT
        };
        let draws = render_decorations(&style, &p, None);
        assert_eq!(draws[0].color, p.red);
    }

    #[test]
    fn underline_falls_back_to_fg_when_no_color_set() {
        let p = TerminalPalette::CARROT_DARK;
        let style = CellStyle {
            flags: CellStyleFlags::UNDERLINE,
            fg: Color::Named(NamedColor::Cyan),
            ..CellStyle::DEFAULT
        };
        let draws = render_decorations(&style, &p, None);
        assert_eq!(draws[0].color, p.cyan);
    }

    #[test]
    fn reverse_video_swaps_fg_and_bg() {
        let style = CellStyle {
            flags: CellStyleFlags::REVERSE,
            fg: Color::Named(NamedColor::Green),
            bg: Color::Named(NamedColor::Red),
            ..CellStyle::DEFAULT
        };
        let out = apply_reverse_video(&style);
        assert_eq!(out.fg, Color::Named(NamedColor::Red));
        assert_eq!(out.bg, Color::Named(NamedColor::Green));
    }

    #[test]
    fn reverse_video_without_flag_is_identity() {
        let style = style_with(CellStyleFlags::empty());
        assert_eq!(apply_reverse_video(&style), style);
    }

    #[test]
    fn font_variant_selector_reads_flags() {
        let sel = FontVariantSelector::from_flags(
            CellStyleFlags::BOLD
                .insert(CellStyleFlags::ITALIC)
                .insert(CellStyleFlags::DIM),
        );
        assert!(sel.bold && sel.italic && sel.dim);

        let none = FontVariantSelector::from_flags(CellStyleFlags::empty());
        assert!(!none.bold && !none.italic && !none.dim);
    }

    #[test]
    fn animation_flags_read_blink_and_hidden() {
        let both = AnimationFlags::from_flags(CellStyleFlags::BLINK.insert(CellStyleFlags::HIDDEN));
        assert!(both.blink && both.hidden);

        let neither = AnimationFlags::from_flags(CellStyleFlags::BOLD);
        assert!(!neither.blink && !neither.hidden);
    }
}
