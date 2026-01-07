// Scrollback search compute shader.
//
// Scans the GPU-resident cell buffer for cells matching a needle
// character and writes match offsets into an output buffer.
// Dispatched one workgroup per 1024 cells; within a workgroup each
// invocation checks one cell.
//
// At 80 cols × 1M lines = 80M cells and a workgroup size of 1024,
// we dispatch 80k workgroups. On a modern GPU (~50 GB/s memory
// bandwidth) scanning 80M × 8 bytes (640 MB) takes ~13 ms.
//
// Buffer bindings:
//   @binding(0) cells:    storage<u64>            — read-only cell words
//   @binding(1) needle:   uniform<u32>            — needle character (ASCII/codepoint)
//   @binding(2) matches:  storage<atomic<u32>>    — write-only match count + offsets
//   @binding(3) params:   uniform<SearchParams>
//
// SearchParams layout:
//   total_cells: u32    — cell count in `cells`
//   max_matches: u32    — capacity of `matches` (first slot is count, rest are offsets)
//
// Matches output layout:
//   matches[0]     = atomic counter of matches found
//   matches[1..N]  = byte offsets into `cells` of matched cells

struct SearchParams {
    total_cells: u32,
    max_matches: u32,
}

@group(0) @binding(0) var<storage, read> cells: array<u32>;
@group(0) @binding(1) var<uniform> needle: u32;
@group(0) @binding(2) var<storage, read_write> matches: array<atomic<u32>>;
@group(0) @binding(3) var<uniform> params: SearchParams;

// Mask that selects the codepoint (content) portion of a Cell's u32
// lower word. Must stay in sync with `carrot_grid::cell::CONTENT_BITS`.
const CONTENT_MASK: u32 = 0x001fffffu;

@compute @workgroup_size(1024)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.total_cells) {
        return;
    }

    // Sample the low word (content). ASCII fast path: cell.content
    // byte matches needle directly; we don't decode the tag here to
    // keep the shader branchless. Upstream caller only dispatches
    // this kernel for ASCII / Codepoint needles.
    let cell_content = cells[index] & CONTENT_MASK;
    if (cell_content == needle) {
        let count_slot = atomicAdd(&matches[0], 1u);
        let write_index = count_slot + 1u;
        if (write_index < params.max_matches) {
            atomicStore(&matches[write_index], index);
        }
    }
}
