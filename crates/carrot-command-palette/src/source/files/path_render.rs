//! Helpers for rendering result rows for the files source.
//!
//! The fuzzy matcher returns character positions against the combined
//! `title + " " + subtitle` haystack. This module splits those positions
//! back into the two segments so the modal's `HighlightedLabel` widgets
//! can bold the matched bytes inline in each label independently.

/// Splits a flat `positions` slice (byte offsets into `title + " " + subtitle`)
/// into the subset that falls inside `title` and the subset that falls inside
/// `subtitle` (with its offset normalized back to zero).
pub(crate) fn split_path_positions(
    positions: &[usize],
    title_len: usize,
) -> (Vec<usize>, Vec<usize>) {
    let title = positions
        .iter()
        .filter(|&&p| p < title_len)
        .copied()
        .collect();
    let subtitle = positions
        .iter()
        .filter(|&&p| p > title_len)
        .map(|p| p - title_len - 1)
        .collect();
    (title, subtitle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_positions_across_title_and_subtitle() {
        // haystack = "main.rs src/foo"
        //             0123456789...
        // title_len = 7 → subtitle starts at 8
        let title_len = 7;
        let positions = vec![0, 1, 8, 9, 12];
        let (t, s) = split_path_positions(&positions, title_len);
        assert_eq!(t, vec![0, 1]);
        assert_eq!(s, vec![0, 1, 4]);
    }

    #[test]
    fn empty_positions_yields_empty_splits() {
        let (t, s) = split_path_positions(&[], 5);
        assert!(t.is_empty());
        assert!(s.is_empty());
    }
}
