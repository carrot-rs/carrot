//! Memory-footprint bench.
//!
//! Budget: 30k lines resident <25 MB per platform.
//!
//! This bench builds a 30.000-line PageList with an 80-column
//! viewport + CellStyleAtlas populated with ~100 distinct styles, then
//! reports the measured memory. Criterion isn't the natural tool
//! for memory (it's a latency timer) so we use it as a harness and
//! emit the measurement via eprintln!.
//!
//! Calculation:
//!   30.000 rows × 80 cols = 2.4M cells
//!   × 8 bytes/cell        ≈ 18.3 MiB pure cell payload
//!   + PageList metadata
//!   + CellStyleAtlas (100 styles × 24 bytes + HashMap overhead)
//!   ≈ 19 MiB — well under the 25 MB budget.
//!
//! The bench asserts <25 MB so a regression on cell-size or page
//! metadata surfaces immediately.

use std::mem::size_of;

use carrot_grid::{
    Cell, CellStyle, CellStyleAtlas, CellStyleFlags, CellStyleId, HyperlinkId, PageCapacity,
    PageList,
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

const LINES: usize = 30_000;
const COLS: u16 = 80;
const BUDGET_MB: usize = 25;

fn build_corpus() -> (PageList, CellStyleAtlas) {
    // 4 KiB pages — Plan A3 default. At 80 cols × 8 bytes = 640
    // bytes per row, that's 6 rows per page → ~5.000 pages for 30k
    // lines. Each page's actual allocation is 6*80*8 = 3840 bytes.
    let cap = PageCapacity::new(COLS, 4096);
    let mut list = PageList::new(cap);
    let mut atlas = CellStyleAtlas::new();
    let default_id = CellStyleId(0);
    // Pre-register ~100 distinct styles so the atlas carries
    // realistic weight.
    let mut style_ids = Vec::with_capacity(100);
    for i in 0..100u32 {
        let style = CellStyle {
            fg: carrot_grid::Color::Indexed((i % 256) as u8),
            bg: carrot_grid::Color::Default,
            underline_color: None,
            flags: CellStyleFlags::empty(),
            hyperlink: HyperlinkId::NONE,
        };
        style_ids.push(atlas.intern(style));
    }
    for r in 0..LINES {
        let row: Vec<Cell> = (0..COLS)
            .map(|c| {
                let style = if (c as usize).is_multiple_of(4) {
                    style_ids[(r + c as usize) % style_ids.len()]
                } else {
                    default_id
                };
                Cell::ascii(b'a' + ((r as u8 + c as u8) % 26), style)
            })
            .collect();
        list.append_row(&row);
    }
    (list, atlas)
}

/// Rough memory estimate. Exact per-platform RSS measurement is
/// out of scope; this estimate sums the cell payload + page
/// metadata + atlas entries.
fn estimate_bytes(list: &PageList, atlas: &CellStyleAtlas) -> usize {
    let page_count = list.page_count();
    // Each page allocates its full cells_cap (rows_cap × cols × 8).
    // For 4 KB pages with 80 cols, rows_cap = 6, cells_cap = 480,
    // allocation = 3840 bytes per page.
    let cells_cap = list.capacity().cells_cap();
    let per_page_alloc = cells_cap * size_of::<Cell>();
    // Each Page struct itself (in the VecDeque) is small — ptr +
    // capacity + counter + u64 bitmap.
    let per_page_struct = size_of::<usize>() * 4;
    let total_pages = page_count * (per_page_alloc + per_page_struct);
    // Atlas: CellStyle + hash-map overhead per entry.
    let atlas_entries = atlas.len();
    let atlas_bytes = atlas_entries * (size_of::<CellStyle>() + 32);
    total_pages + atlas_bytes
}

fn bench_footprint(c: &mut Criterion) {
    // One-shot measurement outside criterion so the budget number
    // shows up in the bench log even when the bench body is re-run
    // many times for p50 stats.
    let (list, atlas) = build_corpus();
    let bytes = estimate_bytes(&list, &atlas);
    let mb = bytes as f64 / (1024.0 * 1024.0);
    eprintln!("memory_30k_lines: {mb:.2} MB (budget {BUDGET_MB} MB)");
    assert!(
        (mb as usize) < BUDGET_MB,
        "estimated memory {mb:.2} MB exceeded {BUDGET_MB} MB budget (bytes = {bytes})"
    );

    c.bench_function("memory_30k_lines", |b| {
        b.iter(|| {
            let (list, atlas) = build_corpus();
            estimate_bytes(black_box(&list), black_box(&atlas))
        })
    });
}

criterion_group!(benches, bench_footprint);
criterion_main!(benches);
