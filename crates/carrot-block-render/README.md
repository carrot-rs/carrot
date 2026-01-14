# carrot-block-render

**Layer 4** of Carrot's Ultimate Block System — GPU-bound rendering for terminal blocks.

Reads cells + styles directly from [`carrot-grid`](../carrot-grid) (no intermediate snapshot), applies per-cell damage tracking, composes text + decorations + cursor + images into a single frame, and emits draws through an [`inazuma::Element`](../inazuma) wrapper for composition into Layer 5.

## What's inside

| Module | Purpose |
|--------|---------|
| `render` (lib.rs root) | O(N_visible) walk over a `PageList` range. Emits per-cell `CellDraw` with resolved styles. Display-only soft-wrap via `row.chunks(viewport_cols)`. |
| `damage` | `FrameState` (previous-frame signatures), `CellSignature` (8 B per cell identity), `Damage::{Full, Partial}`, `compute_damage`. `render_block_damaged` filters draws to changed cells only. |
| `cursor` | `CursorShape::{Block, Underline, Bar}` + `CursorState` → `CursorDraw`. Blink-phase aware, wide-char stretch scaffolding. |
| `decoration` | SGR-flag interpretation: underline / strikethrough rects, reverse-video swap, `FontVariantSelector { bold, italic, dim }`, `AnimationFlags { blink, hidden }`. |
| `image_pass` | `ImageStore` → pixel-space `ImageDraw`s for inline-image protocols, with sub-cell offsets and row-range filtering. |
| `shaping` | HarfRust wrapper: `ShapingFont`, `ShapedGlyph`, `shape_run`. Complex scripts, ligatures, BiDi-ready. |
| `shape_cache` | LRU-bounded `(font, size, text) → Arc<Vec<ShapedGlyph>>` cache. Skips harfrust on hot paths. |
| `snapshot` | `SharedTerminal` wrapping `arc_swap::ArcSwap<TerminalSnapshot>` — VT thread publishes, render thread loads atomically in O(1). Stress-tested under 4 readers × 1000 loads + 100 rcu writes. |
| `frame` | Unified `build_frame(FrameInput) → Frame` composing cells + decorations + cursor + images + signatures + damage into one render-pass output. |
| `diff` | Cell-for-cell grid diff: `GridDiff` + `DiffEntry::{Changed, Added, Removed}`. Foundation for "rerun this block and highlight differences". |
| `block_element` | `inazuma::Element` wrapper that paints a block via `Window::paint_quad` + `Window::paint_glyph`. |

## Guarantees

- Rendering cost is **O(N_visible)** — scrollback size doesn't matter. 24-row viewport renders in ~9 µs whether the block holds 300 or 30 000 rows.
- `render_block_damaged` adds ~3 µs for a signature compare and in exchange shrinks GPU-bound draws from `rows × cols` to exactly the changed cells. Steady-state frames send zero cells.
- `SharedTerminal` is wait-free for readers, lock-free for the single writer. Stress test in `snapshot::tests::concurrent_load_under_store_is_consistent` proves it.
- Shape cache turns 50-200 µs shape calls into Arc clones on hot paths.
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` in production code.
- Layer invariant: depends on `carrot-grid`, `inazuma`, `arc-swap`, `harfrust`, `lru` only; no back-reference into `carrot-term`, `carrot-terminal-view`, or any app-level crate.

## Pipeline status

| Module | Status | What's in |
|--------|--------|-----------|
| Skeleton | ✓ | `render_block` with CellDraw commands |
| Damage tracking | ✓ | FrameState, Damage, CellSignature, render_block_damaged |
| BlockElement | ✓ | inazuma::Element wrapper with RenderSnapshot |
| Lock-free state | ✓ | SharedTerminal / TerminalSnapshot with arc-swap |
| MSDF + shape cache | ◐ | shaping + shape_cache groundwork landed; MSDF atlas + wgpu compute still pending |
| Image pass | ◐ | image_pass foundation landed; wgpu texture lifecycle still pending |
| Cursor | ✓ | render_cursor primitive |
| Decoration | ✓ | render_decorations, apply_reverse_video, FontVariantSelector |
| Unified Frame | ✓ | build_frame composes cells + decorations + cursor + images |

## Benchmarks

```
cargo bench -p carrot-block-render --bench render
```

Numbers on Apple M-class (2026-04):

| Scenario | Cost | GPU cells |
|----------|------|-----------|
| `render_block/visible_24 / 300 scrollback` | ~9 µs | 1920 |
| `render_block/visible_24 / 30000 scrollback` | ~9 µs | 1920 |
| `render_block_damaged/steady_state_80x24` | ~12 µs | **0** |
| `render_block_damaged/one_cell_change_80x24` | ~13 µs | **1** |
| `render_block_damaged/full_frame_80x24` | ~9 µs | 1920 |

## Tests

```
cargo test -p carrot-block-render
```

91 tests total:
- 81 lib: block_element, cursor, damage, decoration, diff, frame, image_pass, shape_cache, shaping, snapshot.
- 4 damage_pipeline multi-frame integration.
- 3 cross_layer (carrot-grid → carrot-term ActiveBlock → renderer).
- 3 spike_end_to_end smoke tests.

End-to-end shape-round-trip tests are blocked on a committed test-font fixture.

## Layer contract

`carrot-block-render` is consumed by `carrot-terminal-view` (Layer 5). The consumer either calls `BlockElement::new` with a `RenderSnapshot` (for per-frame owned data) or — preferred — loads the current `TerminalSnapshot` via `SharedTerminal::load` and feeds it plus cursor + prev FrameState into `build_frame`. The returned `Frame` carries every draw type ordered for the compositor: backgrounds → glyphs → decorations → images → cursor.

## Status

Pipeline: skeleton, damage tracking, BlockElement, lock-free snapshot, cursor, decoration, and unified-frame stages landed. Full MSDF glyph atlas + full image wgpu-texture lifecycle remain as larger follow-up work.
