//! ANSI color resolution for the debugger console.
//!
//! Lifted out of `carrot-terminal-view` because the *real* terminal grid
//! resolves colors through `carrot_block_render::TerminalPalette`. The
//! debugger console can't reuse that path directly because it consumes
//! the upstream `vte::ansi::Color` type rather than the renderer's
//! `carrot_grid::Color`. Keeping a small theme-aware translator here
//! (instead of duplicating the logic in `carrot-terminal-view`) makes it
//! plain that ANSI handling lives in two places only because the two
//! enum families come from two layers.

use carrot_term::vte::ansi::{Color as AnsiColor, NamedColor};
use carrot_theme::Theme;
use inazuma::Oklch;

/// Convert an ANSI escape color into a theme-aware Oklch color, applying
/// the bold-as-bright convention (matches `palette::resolve_styled` on
/// the terminal grid).
pub fn ansi_to_oklch(color: &AnsiColor, theme: &Theme, bold: bool) -> Oklch {
    let tc = &theme.styles.colors;
    match color {
        AnsiColor::Named(name) => named_to_oklch(*name, bold, theme),
        AnsiColor::Spec(rgb) => rgb_to_oklch(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(idx) => match idx {
            0..=15 => named_to_oklch(named_for_index(*idx), bold, theme),
            16..=231 => {
                let i = idx - 16;
                let r = (i / 36) * 51;
                let g = ((i % 36) / 6) * 51;
                let b = (i % 6) * 51;
                rgb_to_oklch(r, g, b)
            }
            232..=255 => {
                let v = 8 + (idx - 232) * 10;
                rgb_to_oklch(v, v, v)
            }
        },
    }
    .unwrap_or(tc.terminal.foreground)
}

fn named_to_oklch(name: NamedColor, bold: bool, theme: &Theme) -> Option<Oklch> {
    let tc = &theme.styles.colors;
    let a = &tc.terminal.ansi;
    // Bold + base ANSI promotes to the bright slot — matches xterm /
    // alacritty / Warp behaviour.
    let promoted = if bold { promote_to_bright(name) } else { name };
    Some(match promoted {
        NamedColor::Background => return Some(tc.terminal.background),
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
            return Some(tc.terminal.foreground);
        }
        NamedColor::Cursor => return Some(tc.text_accent),
        NamedColor::Black => a.black,
        NamedColor::Red => a.red,
        NamedColor::Green => a.green,
        NamedColor::Yellow => a.yellow,
        NamedColor::Blue => a.blue,
        NamedColor::Magenta => a.magenta,
        NamedColor::Cyan => a.cyan,
        NamedColor::White => a.white,
        NamedColor::BrightBlack | NamedColor::DimBlack => a.bright_black,
        NamedColor::BrightRed | NamedColor::DimRed => a.bright_red,
        NamedColor::BrightGreen | NamedColor::DimGreen => a.bright_green,
        NamedColor::BrightYellow | NamedColor::DimYellow => a.bright_yellow,
        NamedColor::BrightBlue | NamedColor::DimBlue => a.bright_blue,
        NamedColor::BrightMagenta | NamedColor::DimMagenta => a.bright_magenta,
        NamedColor::BrightCyan | NamedColor::DimCyan => a.bright_cyan,
        NamedColor::BrightWhite | NamedColor::DimWhite => a.bright_white,
    })
}

fn named_for_index(idx: u8) -> NamedColor {
    match idx {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        7 => NamedColor::White,
        8 => NamedColor::BrightBlack,
        9 => NamedColor::BrightRed,
        10 => NamedColor::BrightGreen,
        11 => NamedColor::BrightYellow,
        12 => NamedColor::BrightBlue,
        13 => NamedColor::BrightMagenta,
        14 => NamedColor::BrightCyan,
        15 => NamedColor::BrightWhite,
        _ => unreachable!("0..=15 handled by caller"),
    }
}

fn promote_to_bright(n: NamedColor) -> NamedColor {
    match n {
        NamedColor::Black => NamedColor::BrightBlack,
        NamedColor::Red => NamedColor::BrightRed,
        NamedColor::Green => NamedColor::BrightGreen,
        NamedColor::Yellow => NamedColor::BrightYellow,
        NamedColor::Blue => NamedColor::BrightBlue,
        NamedColor::Magenta => NamedColor::BrightMagenta,
        NamedColor::Cyan => NamedColor::BrightCyan,
        NamedColor::White => NamedColor::BrightWhite,
        other => other,
    }
}

fn rgb_to_oklch(r: u8, g: u8, b: u8) -> Option<Oklch> {
    Some(Oklch::from(inazuma::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }))
}
