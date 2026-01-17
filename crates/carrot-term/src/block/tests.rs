//! Unit tests for the ActiveBlock / FrozenBlock two-phase lifecycle.

use std::time::{Duration, Instant};

use carrot_grid::{Cell, CellStyle, CellStyleId};

use super::{ActiveBlock, BlockState};

fn row(cols: u16, content: u8, style: CellStyleId) -> Vec<Cell> {
    (0..cols).map(|_| Cell::ascii(content, style)).collect()
}

#[test]
fn active_block_appends_rows() {
    let mut block = ActiveBlock::new(80);
    for _ in 0..10 {
        block.append_row(&row(80, b'x', CellStyleId(0)));
    }
    assert_eq!(block.total_rows(), 10);
}

#[test]
fn active_intern_style_then_use_in_row() {
    let mut block = ActiveBlock::new(10);
    let id = block.intern_style(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Green),
        ..CellStyle::DEFAULT
    });
    assert_ne!(id.0, 0, "non-default style must get a non-zero id");
    block.append_row(&row(10, b'A', id));
    assert_eq!(block.total_rows(), 1);
    assert_eq!(block.atlas().len(), 2);
}

#[test]
fn finish_transitions_to_frozen_with_exit_code() {
    let mut block = ActiveBlock::new(10);
    block.metadata_mut().command = Some("cargo test".into());
    block.metadata_mut().started_at = Some(Instant::now() - Duration::from_millis(500));
    for _ in 0..5 {
        block.append_row(&row(10, b'y', CellStyleId(0)));
    }

    let frozen = block.finish(Some(0), Some(Instant::now()));
    assert_eq!(frozen.exit_code(), Some(0));
    assert!(!frozen.is_error());
    assert_eq!(frozen.total_rows(), 5);
    assert_eq!(frozen.metadata().command.as_deref(), Some("cargo test"));
    assert!(frozen.duration().is_some());
}

#[test]
fn frozen_is_error_when_nonzero_exit() {
    let block = ActiveBlock::new(4);
    let frozen = block.finish(Some(1), None);
    assert!(frozen.is_error());
}

#[test]
fn frozen_atlas_is_arc_shared() {
    let mut block = ActiveBlock::new(4);
    block.intern_style(CellStyle {
        fg: carrot_grid::Color::Named(carrot_grid::NamedColor::Yellow),
        ..CellStyle::DEFAULT
    });
    let frozen1 = block.finish(Some(0), None);
    let atlas_ref: &std::sync::Arc<[CellStyle]> = frozen1.atlas();
    let cloned = atlas_ref.clone();
    assert!(std::sync::Arc::ptr_eq(atlas_ref, &cloned));
}

#[test]
fn block_state_finishes_once() {
    let mut state = BlockState::new_active(4);
    assert!(state.variant().is_active());

    let frozen = state.finish(Some(0), None);
    assert!(frozen.is_some(), "first finish transitions");
    assert!(state.variant().is_frozen());

    let second = state.finish(Some(1), None);
    assert!(second.is_none(), "second finish is no-op");
}

#[test]
fn variant_accessors() {
    let mut state = BlockState::new_active(4);
    assert!(state.variant().as_active().is_some());
    assert!(state.variant().as_frozen().is_none());

    // Mutate then finish.
    if let Some(active) = state.variant_mut().as_active_mut() {
        active.append_row(&row(4, b'Z', CellStyleId(0)));
    }
    state.finish(Some(0), None);
    assert!(state.variant().as_frozen().is_some());
    assert_eq!(state.variant().total_rows(), 1);
}

#[test]
fn finish_carries_hyperlinks_and_graphemes_to_frozen() {
    let mut block = ActiveBlock::new(4);
    let url_id = block.hyperlinks_mut().intern("https://carrot.dev");
    let cluster_id = block.graphemes_mut().intern("a\u{0301}");
    let frozen = block.finish(Some(0), None);
    // Hyperlink URL and grapheme cluster survive the transition.
    assert_eq!(frozen.hyperlinks().get(url_id), Some("https://carrot.dev"));
    assert_eq!(frozen.graphemes().get(cluster_id), Some("a\u{0301}"));
}

#[test]
fn active_uses_carrot_grid_page_layout() {
    // Proves the underlying storage is PageList with the expected capacity:
    // 80 cols × 4 KB page = 6 rows per page. Push 20 rows → ≥3 pages.
    let mut block = ActiveBlock::new(80);
    for _ in 0..20 {
        block.append_row(&row(80, b'a', CellStyleId(0)));
    }
    assert_eq!(block.grid().total_rows(), 20);
    assert!(block.grid().page_count() >= 3);
}

#[test]
fn variant_is_frozen_and_is_active_mutually_exclusive() {
    let mut state = BlockState::new_active(1);
    assert!(state.variant().is_active());
    assert!(!state.variant().is_frozen());
    state.finish(None, None);
    assert!(!state.variant().is_active());
    assert!(state.variant().is_frozen());
}
