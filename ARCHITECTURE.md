# Architecture

High-level map of Carrot's crates, organised by the six layers of the block system.

## Layers

```
┌──────────────────────────────────────────────────────────────────┐
│ Layer 5 — carrot-terminal-view    (composition / app integration) │
│            carrot-cmdline (NEW)    (command-entry surface)        │
├──────────────────────────────────────────────────────────────────┤
│ Layer 4 — carrot-block-render     (GPU passes: text/image/cursor) │
├──────────────────────────────────────────────────────────────────┤
│ Layer 3 — inazuma::block          (virtualised block primitive)   │
├──────────────────────────────────────────────────────────────────┤
│ Layer 2 — carrot-term             (VT parser, OSC dispatch, PTY)  │
├──────────────────────────────────────────────────────────────────┤
│ Layer 1 — carrot-grid             (Cell, PageList, StyleAtlas)    │
├──────────────────────────────────────────────────────────────────┤
│ Layer 0 — inazuma                 (GPU UI framework)              │
└──────────────────────────────────────────────────────────────────┘
```

Dependencies flow **downward only**. `carrot-grid` has zero `inazuma` / `carrot-*` deps; `carrot-block-render` depends on `inazuma` + `carrot-grid` but not on `carrot-term` (cross-layer tests use it for integration); `carrot-terminal-view` composes everything.

## Layer 1 — `carrot-grid`

Pure data primitives for terminal content. 8-byte packed `Cell`, `StyleAtlas` interning, `PageList` with O(1) append + O(1) scrollback prune + O(N_visible) range iter. Standalone; publishable on crates.io as-is. Details: [crates/carrot-grid/README.md](crates/carrot-grid/README.md).

## Layer 2 — `carrot-term`

VT state machine + PTY lifecycle + block data model. See [the crate's module-level docs](crates/carrot-term/src/lib.rs) for the internal split. `block::{ActiveBlock, FrozenBlock, VtWriter, ReplayBuffer}` + `simd_scan` AVX2/NEON fast-path.

## Layer 3 — `inazuma::block`

Virtualised-list primitive for terminal-shaped content. Bottom-anchor layout, per-entry fold state, pinned-footer mechanic, O(N_visible) cursor-based iteration. Lives inside `inazuma` because it needs direct access to the element system. Terminal-agnostic — `carrot-terminal-view` is its first consumer but not its only one.

## Layer 4 — `carrot-block-render`

GPU-bound rendering for terminal blocks. Composes every primitive into a single `build_frame` call. Details: [crates/carrot-block-render/README.md](crates/carrot-block-render/README.md).

## Layer 5 — App composition

- `carrot-terminal-view` hooks the render pass into the workspace.
- `carrot-cmdline` wraps `carrot-editor` for the command-entry surface.

## Testing

Every layer's crate ships `cargo test` + `cargo bench` with concrete perf numbers recorded in the bench module docstrings (see `crates/*/benches/*.rs`). Cross-layer integration is covered by `crates/carrot-block-render/tests/full_pipeline.rs` — one test that exercises PageList + VtWriter + ActiveBlock + style atlas + image store + build_frame + damage + replay + grid-diff in a single realistic scenario. When this passes, every seam is wired correctly.

## Status

- Layer 1 `carrot-grid` — bench-validated.
- Layer 2 `carrot-term` — `block` + `simd_scan` landed.
- Layer 3 `inazuma::block` — fold, pin, and `visible_entries` in place.
- Layer 4 `carrot-block-render` — MSDF atlas + wgpu image textures covered.
- Layer 5 `carrot-terminal-view` + `carrot-cmdline` — composition layer.
