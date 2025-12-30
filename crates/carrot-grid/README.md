# carrot-grid

**Layer 1** of Carrot's Ultimate Block System — the pure data primitives for terminal content.

Zero dependencies on UI, GPU, or VT. This crate knows nothing about escape sequences, rendering, or windows; it just stores cells efficiently and lets the higher layers read them back fast.

## What's inside

| Module | Purpose |
|--------|---------|
| `cell` | 8-byte packed `Cell` with a 3-bit tag discriminating ASCII / Codepoint / Grapheme / Wide2nd / Image / ShapedRun / CustomRender / Reserved. `u16` `StyleId`, per-cell flag bits (dirty, wrap, protected, hyperlink). Compile-time asserted at 8 bytes. |
| `cell_id` | Stable `CellId { origin: u64, col: u16 }` that survives scrollback pruning. `CellIdRow::row_span()` iterator. Foundation for a future CRDT / multi-player layer. |
| `style` | `StyleAtlas` with u16 interning. Cells carry only a style id; resolved Oklch colors + flags + underline color + font-family override live here. `DEFAULT` is always id 0. |
| `page` | `Page` + `PageCapacity`. Fixed 4 KB-aligned contiguous Cell buffer, per-row dirty bitmap in u64 (rows_cap clamped at 64). Zero-alloc recycling via `reset`. |
| `page_list` | `PageList`: `VecDeque<Page>` with "non-tail pages are always full" invariant. O(1) amortized append, O(1) prune via page recycling, O(1) random row access via page divmod, O(N_visible) range iteration. |
| `image` | `ImageStore` per-block table of decoded image bytes referenced by `Cell::image`. Arc-shared so frozen-block clones are cheap. |
| `compress` | `CompressedCells` + `compress` / `decompress` via zstd. 12-20× ratio on log-like content; decompress stays sub-ms even for 10k rows. |
| `search` | `search_cells(pages, needle, options)`. Returns `Vec<SearchMatch>` tagged with stable `CellId`s. Options: case-insensitive, whole-word, allow-overlap. UTF-8 aware; rows searched independently. |

## Guarantees

- `sizeof::<Cell>() == 8` (compile-time asserted).
- No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` in production code. Allocation failures go through `std::alloc::handle_alloc_error` (stdlib idiom).
- Append stays O(1) amortized across unbounded scrollback.
- Prune stays O(1) — the oldest page is reset and recycled, no memmove, no realloc.
- Range iteration visits exactly the requested rows.
- `CellId` stays stable across scrollback prune — a pruned id returns `None` from resolvers, never silently points at a different cell.
- Compression round-trip is byte-for-byte faithful (tested across every interesting Cell flag combination).

## Benchmarks

```
cargo bench -p carrot-grid --bench grid
```

Numbers on Apple M-class (2026-04):

| Scenario | Cost | Thrpt |
|----------|------|-------|
| `append_row / 100k rows / 80 cols` | 3.3 ms | **17-18 GiB/s** |
| `prune_head / 16k pages` | 718 ns | — (O(1)) |
| `rows_iter / visible 24 in 10k total` | 24 ns | — |
| `rows_iter / visible 24 in 100k total` | 24 ns (identical!) | — |
| `atlas_intern / hit` | 7 ns | — |
| `compress / 10k rows × 80` | 1.30 ms | 4.60 GiB/s |
| `decompress / 10k rows × 80` | 504 µs | 11.8 GiB/s |

Perf targets:
- `append_row` ≥ 1 GB/s → hit **17-18 GiB/s** (17×).
- `prune_head` O(1) → sub-µs constant-class confirmed.
- `rows(range)` O(N_visible) → identical cost at 10k vs 100k scrollback.
- Compressed cold pages ≥ 10× → **12-20×** on log content.

## Tests

```
cargo test -p carrot-grid
```

61 tests covering: cell bit layout round-trips, style atlas interning, page capacity math + row readback + reset semantics, page-list append/prune invariants + O(1) page divmod, cell-id stability across prune, compression byte-for-byte round-trip + ratios, image-store Arc sharing, CPU scrollback search (case / overlap / whole-word / UTF-8).

## Layer contract

`carrot-grid` is consumed by:

- `carrot-term` (Layer 2) — VT state machine writes cells to a `PageList`.
- `carrot-block-render` (Layer 4) — GPU pipeline reads rows directly, no intermediate snapshot.

It does **not** depend on `inazuma`, `carrot-term`, `carrot-terminal-view`, or any app-level crate. This is deliberate: the crate is a pure library that could be published to crates.io unchanged.

## Status

Layer 1 is production-ready: search, cold compression, and stable `CellId`s are in place.
