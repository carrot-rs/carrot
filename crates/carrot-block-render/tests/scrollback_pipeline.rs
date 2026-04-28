//! Scrollback consumer pipeline blueprint.
//!
//! Simulates the terminal-view rendering path end-to-end:
//!
//!   PTY bytes ──► VtWriter ──► ActiveBlock ──► BlockSnapshot::from_pages
//!                                             ──► BlockElement (render)
//!
//!   + PromptStart/CommandStart/CommandEnd via the BlockRouter lifecycle.
//!   + finish() hands an Arc<FrozenBlock> to the scrollback list.
//!   + FrozenBlock → BlockSnapshot::from_pages → BlockElement.
//!
//! This mirrors what `carrot-terminal-view::block_list.rs` does:
//! walk `term.block_router().entries()`, render each with
//! BlockElement.
//!
//! Lives in carrot-block-render tests because it covers the consumer-
//! facing contract of this crate (not the terminal behaviour).

use carrot_grid::BlockSnapshot;
use carrot_term::block::{ActiveBlock, BlockRouter, VtWriter, VtWriterState};
use carrot_term::vte::ansi::{Processor, StdSyncHandler};

fn feed(state: &mut VtWriterState, block: &mut ActiveBlock, bytes: &[u8]) {
    let mut processor = Processor::<StdSyncHandler>::new();
    let mut writer = VtWriter::new_in(state, block);
    processor.advance(&mut writer, bytes);
    writer.commit_row();

    writer.finalize();
}

#[test]
fn active_block_is_renderable_through_snapshot() {
    // Construct a live block + writer state, feed some bytes. Keep
    // the input short enough that autowrap doesn't split the line.
    let mut block = ActiveBlock::new(20);
    let mut state = VtWriterState::new(20, 24);
    feed(&mut state, &mut block, b"hello world");

    // Build the snapshot the renderer consumes.
    let atlas = block.atlas().as_slice();
    let snap = BlockSnapshot::from_pages(block.grid(), atlas);

    assert_eq!(snap.rows.len(), 1, "single-line input produces 1 row");
    assert_eq!(snap.rows[0][0].content(), b'h' as u32);
    assert_eq!(snap.rows[0][10].content(), b'd' as u32);
    assert!(
        snap.atlas.len() >= 1,
        "atlas holds at least the default style",
    );
}

#[test]
fn router_frozen_block_is_renderable_via_snapshot() {
    // Walk the full OSC 133 lifecycle through the v2 router.
    let mut router = BlockRouter::new(10);
    router.on_prompt_start();
    let _id = router.on_command_start();

    // Feed some output bytes into the live block.
    {
        let mut target = router.active();
        let block = target.as_active_mut();
        let mut state = VtWriterState::new(10, 24);
        feed(&mut state, block, b"line1\r\nline2");
    }

    // CommandEnd → frozen block pops out.
    let frozen = router.on_command_end(0).expect("frozen block");
    let snap = BlockSnapshot::from_pages(frozen.grid(), frozen.atlas());

    assert!(snap.rows.len() >= 2, "two output lines land as rows");
    assert_eq!(frozen.exit_code(), Some(0));
    assert!(!frozen.is_error());
}

#[test]
fn router_frozen_entries_iterator_feeds_scrollback_list() {
    // Simulate three finished blocks, then render them all via
    // the scrollback idiom: iterate frozen_entries,
    // build BlockSnapshot, hand off to BlockElement.
    let mut router = BlockRouter::new(10);
    for i in 0..3u8 {
        router.on_prompt_start();
        router.on_command_start();
        {
            let mut target = router.active();
            let block = target.as_active_mut();
            let mut state = VtWriterState::new(10, 24);
            let bytes = [b'a' + i, b'a' + i, b'a' + i];
            feed(&mut state, block, &bytes);
        }
        router.on_command_end(0);
    }

    let snapshots: Vec<_> = router
        .frozen_entries()
        .filter_map(|entry| entry.variant.as_frozen())
        .map(|frozen| BlockSnapshot::from_pages(frozen.grid(), frozen.atlas()))
        .collect();

    assert_eq!(snapshots.len(), 3, "three commands → three snapshots");
    for (i, snap) in snapshots.iter().enumerate() {
        let first_cell = &snap.rows[0][0];
        assert_eq!(first_cell.content(), (b'a' + i as u8) as u32);
    }
}
