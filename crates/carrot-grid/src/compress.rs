//! Cold-scrollback compression.
//!
//! Hot pages live in a contiguous `[Cell]` buffer — fast random access
//! but 8 bytes per cell regardless of content. For scrollback far from
//! the viewport, this is expensive: 1 million rows × 80 cols × 8 B =
//! **640 MB** just for the cell grid.
//!
//! Log-style output has enormous redundancy (repeating ASCII, single
//! style id everywhere, wide swaths of spaces). zstd compresses it
//! 10–20× in practice. A million log rows fit in **30–60 MB** instead
//! of 640.
//!
//! This module provides the compression **primitives** — an opaque
//! [`CompressedCells`] handle plus `compress` / `decompress`. The
//! policy layer (when to compact a page, how long since last access
//! to wait) is handled by `PageList` in a follow-up phase.
//!
//! # Invariants
//!
//! - `decompress(compress(cells)) == cells` byte-for-byte. Tested.
//! - The compressed payload is safe to persist (e.g. write to a
//!   disk-backed scrollback). The format is just zstd over raw Cell
//!   bytes — portable across machines of the same endianness.
//!   Cross-endian portability is a future concern (current targets
//!   — x86_64, aarch64 — are all little-endian).

use std::io::{Read, Write};

use crate::cell::Cell;

/// zstd compression level used for cold pages. Level 3 is zstd's
/// library default — a reasonable ratio-vs-speed tradeoff. Plan
/// budget (B4) calls for ≥ 10× on log workloads; level 3 clears
/// that easily. Level 19+ would squeeze more ratio but at 100×
/// compression time.
pub const COLD_COMPRESSION_LEVEL: i32 = 3;

/// Compressed byte payload of a cell slice, plus the original cell
/// count so decompression can preallocate exactly.
///
/// Opaque to the render path: readers call [`decompress`] to
/// materialise back into a `Vec<Cell>`. Cold pages stay cold until
/// a reader asks for them.
#[derive(Debug, Clone)]
pub struct CompressedCells {
    payload: Vec<u8>,
    original_cell_count: usize,
}

impl CompressedCells {
    /// Compressed byte size. What the caller pays in memory.
    pub fn len(&self) -> usize {
        self.payload.len()
    }

    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }

    /// Cell count this payload will decompress into.
    pub fn cell_count(&self) -> usize {
        self.original_cell_count
    }

    /// Raw compressed bytes — for disk persistence or IPC.
    pub fn bytes(&self) -> &[u8] {
        &self.payload
    }
}

/// Possible compression failures. These are truly exceptional
/// (OOM on allocator, corrupt payload in decompress). Callers
/// typically treat them as fatal.
#[derive(Debug)]
pub enum CompressError {
    /// The underlying zstd encoder/decoder returned an IO-level
    /// error. Wraps the original message for diagnostics.
    Io(std::io::Error),
}

impl std::fmt::Display for CompressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompressError::Io(e) => write!(f, "compress io error: {e}"),
        }
    }
}

impl std::error::Error for CompressError {}

impl From<std::io::Error> for CompressError {
    fn from(e: std::io::Error) -> Self {
        CompressError::Io(e)
    }
}

/// Compress a slice of cells.
///
/// The input is reinterpreted as a `[u8]` over the cells' backing
/// memory — zero copy into the encoder. Output is zstd-compressed at
/// [`COLD_COMPRESSION_LEVEL`].
pub fn compress(cells: &[Cell]) -> Result<CompressedCells, CompressError> {
    let input_bytes = cells_as_bytes(cells);
    let mut encoder = zstd::Encoder::new(
        Vec::with_capacity(input_bytes.len() / 8),
        COLD_COMPRESSION_LEVEL,
    )?;
    encoder.write_all(input_bytes)?;
    let payload = encoder.finish()?;
    Ok(CompressedCells {
        payload,
        original_cell_count: cells.len(),
    })
}

/// Decompress back into an owned `Vec<Cell>`. Allocates exactly
/// `compressed.cell_count()` cells.
pub fn decompress(compressed: &CompressedCells) -> Result<Vec<Cell>, CompressError> {
    let expected_bytes = compressed.original_cell_count * std::mem::size_of::<Cell>();
    let mut decoder = zstd::Decoder::new(compressed.payload.as_slice())?;
    let mut output = Vec::with_capacity(expected_bytes);
    decoder.read_to_end(&mut output)?;
    // Sanity: the decompressed byte count must match exactly.
    if output.len() != expected_bytes {
        return Err(CompressError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "decompressed byte count {} != expected {} (corrupt payload?)",
                output.len(),
                expected_bytes
            ),
        )));
    }
    Ok(bytes_to_cells(output))
}

/// Reinterpret `&[Cell]` as `&[u8]`. Safe because `Cell` is
/// `repr(transparent)` over `u64` — a POD with no padding.
fn cells_as_bytes(cells: &[Cell]) -> &[u8] {
    // SAFETY: Cell is `#[repr(transparent)]` over u64 (see cell.rs).
    // All bit patterns in a u64 are valid, so `[Cell]` ↔ `[u8]`
    // reinterpretation is sound.
    unsafe { std::slice::from_raw_parts(cells.as_ptr() as *const u8, std::mem::size_of_val(cells)) }
}

/// Convert an owned `Vec<u8>` whose contents are exactly `Cell`-
/// aligned bytes back into a `Vec<Cell>`. The buffer length must be
/// a multiple of `sizeof::<Cell>()`; callers guarantee this.
fn bytes_to_cells(mut bytes: Vec<u8>) -> Vec<Cell> {
    debug_assert_eq!(bytes.len() % std::mem::size_of::<Cell>(), 0);
    let cell_count = bytes.len() / std::mem::size_of::<Cell>();

    // Ensure the allocation has capacity that's a whole number of
    // Cells — otherwise Vec::from_raw_parts's capacity invariant
    // breaks.
    bytes.shrink_to_fit();
    let capacity_bytes = bytes.capacity();
    let capacity_cells = capacity_bytes / std::mem::size_of::<Cell>();

    let ptr = bytes.as_mut_ptr() as *mut Cell;
    std::mem::forget(bytes);
    // SAFETY: Cell is repr(transparent) over u64 with 8-byte align;
    // the original Vec<u8> was populated via zstd which keeps the
    // buffer 8-aligned in practice via the default allocator.
    // `shrink_to_fit` minimises capacity so `capacity_cells *
    // size_of::<Cell>() <= capacity_bytes`, preserving Vec's
    // capacity invariant. We reconstruct the Vec with the trimmed
    // `capacity_cells` — anything leftover was not actually owned.
    unsafe { Vec::from_raw_parts(ptr, cell_count, capacity_cells) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyleId;

    fn row(content: u8, len: u16, style: u16) -> Vec<Cell> {
        (0..len)
            .map(|_| Cell::ascii(content, CellStyleId(style)))
            .collect()
    }

    #[test]
    fn round_trip_empty_slice() {
        let compressed = compress(&[]).expect("compress empty");
        assert_eq!(compressed.cell_count(), 0);
        let back = decompress(&compressed).expect("decompress empty");
        assert!(back.is_empty());
    }

    #[test]
    fn round_trip_small_row() {
        let cells = row(b'a', 80, 0);
        let compressed = compress(&cells).expect("compress");
        let back = decompress(&compressed).expect("decompress");
        assert_eq!(cells, back);
    }

    #[test]
    fn round_trip_mixed_content() {
        let mut cells = Vec::new();
        for r in 0..50u8 {
            for c in 0..80u8 {
                let ch = b'a' + ((r + c) % 26);
                cells.push(Cell::ascii(ch, CellStyleId(r as u16 % 3)));
            }
        }
        let compressed = compress(&cells).expect("compress");
        let back = decompress(&compressed).expect("decompress");
        assert_eq!(cells, back);
    }

    #[test]
    fn log_like_repetition_compresses_well() {
        // 10 000 cells of the same style/char — should compress ~100×
        // or better. Hits the lower bound of the plan's 10× target.
        let cells = row(b'x', 10_000, 0);
        let raw_bytes = std::mem::size_of_val(cells.as_slice());
        let compressed = compress(&cells).expect("compress");
        let ratio = raw_bytes as f64 / compressed.len() as f64;
        assert!(
            ratio >= 10.0,
            "homogeneous workload should hit ≥10× compression, got {ratio:.1}× \
             ({} bytes → {} bytes)",
            raw_bytes,
            compressed.len()
        );
    }

    #[test]
    fn varied_ascii_log_hits_plan_target() {
        // More realistic log-ish content: numeric sequence with
        // variation. Target ≥ 10× per plan B4.
        let mut cells = Vec::new();
        for i in 0..10_000u32 {
            let digits = format!("{i:010}");
            for b in digits.bytes() {
                cells.push(Cell::ascii(b, CellStyleId(0)));
            }
        }
        let raw_bytes = std::mem::size_of_val(cells.as_slice());
        let compressed = compress(&cells).expect("compress");
        let ratio = raw_bytes as f64 / compressed.len() as f64;
        assert!(
            ratio >= 10.0,
            "numeric-log content should hit ≥10× compression, got {ratio:.1}× \
             ({} bytes → {} bytes)",
            raw_bytes,
            compressed.len()
        );
    }

    #[test]
    fn compressed_bytes_are_stable_across_compressions() {
        // Two compressions of identical input produce identical output —
        // zstd is deterministic at a fixed level. Useful for caching /
        // dedup logic later.
        let cells = row(b'q', 500, 2);
        let a = compress(&cells).expect("a");
        let b = compress(&cells).expect("b");
        assert_eq!(a.bytes(), b.bytes());
    }

    #[test]
    fn decompress_rejects_corrupt_payload() {
        // Hand-built bogus payload — decompress should report an
        // Io error, not silently hand back garbage or panic.
        let corrupt = CompressedCells {
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00],
            original_cell_count: 10,
        };
        let result = decompress(&corrupt);
        assert!(result.is_err(), "corrupt payload must error");
    }

    #[test]
    fn round_trip_preserves_cell_bit_layout() {
        // Every Cell is 8 bytes; we rely on that for the reinterpret.
        // Round-trip a cell with every interesting flag bit set.
        let c = Cell::ascii(b'z', CellStyleId(65_000))
            .with_dirty(true)
            .with_wrap_continuation(true)
            .with_protected(true)
            .with_hyperlink(true);
        let cells = vec![c; 3];
        let compressed = compress(&cells).expect("compress");
        let back = decompress(&compressed).expect("decompress");
        assert_eq!(back.len(), 3);
        for got in &back {
            assert_eq!(got.to_bits(), c.to_bits());
        }
    }
}
