//! vte → grid `Color` conversion.
//!
//! The VT parser (in `crate::vte`) reports SGR colors as its own
//! `Color::Named/Indexed/Spec`. Layer 1 (carrot-grid) stores cells with
//! [`carrot_grid::Color`] — an unresolved tagged reference. This module
//! is the tiny bridge: map vte → grid, nothing else. Concrete Oklch
//! resolution lives in Layer 4 (carrot-block-render) against the active
//! theme's palette.

use carrot_grid::{Color, NamedColor as GridNamed};

use crate::vte::ansi::{Color as VteColor, NamedColor as VteNamed, Rgb};

/// Convert a vte SGR color into a grid [`Color`].
pub fn from_vte(color: VteColor) -> Color {
    match color {
        VteColor::Named(n) => Color::Named(map_named(n)),
        VteColor::Indexed(i) => Color::Indexed(i),
        VteColor::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
    }
}

fn map_named(n: VteNamed) -> GridNamed {
    match n {
        VteNamed::Black => GridNamed::Black,
        VteNamed::Red => GridNamed::Red,
        VteNamed::Green => GridNamed::Green,
        VteNamed::Yellow => GridNamed::Yellow,
        VteNamed::Blue => GridNamed::Blue,
        VteNamed::Magenta => GridNamed::Magenta,
        VteNamed::Cyan => GridNamed::Cyan,
        VteNamed::White => GridNamed::White,
        VteNamed::BrightBlack => GridNamed::BrightBlack,
        VteNamed::BrightRed => GridNamed::BrightRed,
        VteNamed::BrightGreen => GridNamed::BrightGreen,
        VteNamed::BrightYellow => GridNamed::BrightYellow,
        VteNamed::BrightBlue => GridNamed::BrightBlue,
        VteNamed::BrightMagenta => GridNamed::BrightMagenta,
        VteNamed::BrightCyan => GridNamed::BrightCyan,
        VteNamed::BrightWhite => GridNamed::BrightWhite,
        VteNamed::DimBlack => GridNamed::DimBlack,
        VteNamed::DimRed => GridNamed::DimRed,
        VteNamed::DimGreen => GridNamed::DimGreen,
        VteNamed::DimYellow => GridNamed::DimYellow,
        VteNamed::DimBlue => GridNamed::DimBlue,
        VteNamed::DimMagenta => GridNamed::DimMagenta,
        VteNamed::DimCyan => GridNamed::DimCyan,
        VteNamed::DimWhite => GridNamed::DimWhite,
        VteNamed::Foreground => GridNamed::Foreground,
        VteNamed::BrightForeground => GridNamed::BrightForeground,
        VteNamed::DimForeground => GridNamed::DimForeground,
        VteNamed::Background => GridNamed::Background,
        VteNamed::Cursor => GridNamed::Cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vte_named_cyan_maps_to_grid_named_cyan() {
        let out = from_vte(VteColor::Named(VteNamed::Cyan));
        assert_eq!(out, Color::Named(GridNamed::Cyan));
    }

    #[test]
    fn vte_indexed_passes_through() {
        let out = from_vte(VteColor::Indexed(42));
        assert_eq!(out, Color::Indexed(42));
    }

    #[test]
    fn vte_rgb_passes_through() {
        let out = from_vte(VteColor::Spec(Rgb {
            r: 232,
            g: 99,
            b: 11,
        }));
        assert_eq!(out, Color::Rgb(232, 99, 11));
    }
}
