//! Emoji-presentation heuristic.
//!
//! UAX #11 reports width-1 for many characters that real terminals
//! render double-wide because they're emoji. The v2 VT writer calls
//! [`emoji_presentation`] to coerce those to width-2 so layout
//! matches the rendered glyph.
//!
//! The heuristic is deliberately conservative: it only returns
//! `true` for scalars that are already double-width in every major
//! terminal's emoji presentation. Non-emoji characters return
//! `false` untouched.

/// `true` when `c` is in a Unicode block where the default
/// presentation is a double-width emoji.
#[inline]
pub fn emoji_presentation(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        // Misc symbols (☀..☯) — selectively 2-wide
        0x2600..=0x26FF
        // Dingbats
        | 0x2700..=0x27BF
        // Emoticons
        | 0x1F600..=0x1F64F
        // Misc symbols + pictographs
        | 0x1F300..=0x1F5FF
        // Transport + map
        | 0x1F680..=0x1F6FF
        // Supplemental symbols + pictographs
        | 0x1F900..=0x1F9FF
        // Symbols + pictographs extended-A
        | 0x1FA70..=0x1FAFF
        // Regional indicator pairs (flag emoji)
        | 0x1F1E6..=0x1F1FF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_is_not_emoji() {
        assert!(!emoji_presentation('A'));
        assert!(!emoji_presentation(' '));
        assert!(!emoji_presentation('\0'));
    }

    #[test]
    fn grinning_face_is_emoji() {
        assert!(emoji_presentation('\u{1F600}'));
    }

    #[test]
    fn regional_indicators_are_emoji() {
        assert!(emoji_presentation('\u{1F1E9}'));
    }

    #[test]
    fn cjk_ideograph_is_not_emoji_via_this_heuristic() {
        // Wide CJK characters are handled by unicode-width, not this.
        assert!(!emoji_presentation('你'));
    }
}
