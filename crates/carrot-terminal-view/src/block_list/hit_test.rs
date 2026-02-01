//! Pixel → grid hit testing on the block list.
//!
//! Reads the per-frame layout cache assembled by the render pass and
//! maps mouse positions onto `(block_index, block_id, grid point,
//! side)` tuples. Pure geometry — no terminal-lock access.

use carrot_term::BlockId;
use carrot_term::index::{Column, Line, Point, Side};
use inazuma::{Pixels, Point as GpuiPoint, px};

use crate::block_list::BlockListView;
use crate::constants::*;

impl BlockListView {
    /// Convert a pixel position (relative to this view) to a
    /// `(block_index, block_id, grid Point, Side)` tuple.
    pub(crate) fn hit_test(
        &self,
        pos: GpuiPoint<Pixels>,
        cell_width: Pixels,
        cell_height: Pixels,
    ) -> Option<(usize, BlockId, Point, Side)> {
        for entry in &self.block_layout {
            let Some(&inazuma_id) = self.list_ids.get(entry.block_index) else {
                continue;
            };
            if let Some(bounds) = self.list_state.bounds_for_block(inazuma_id)
                && bounds.contains(&pos)
            {
                let grid_height = cell_height * entry.content_rows as f32;
                let grid_start_y = entry.grid_origin_store.get().unwrap_or_else(|| {
                    bounds.origin.y + bounds.size.height - px(BLOCK_BODY_PAD_BOTTOM) - grid_height
                });
                let y_in_grid = pos.y - grid_start_y;

                log::debug!(
                    "hit_test: pos.y={:.1} grid_origin={:.1} y_in_grid={:.1} \
                     cell_h={:.1} visual_row={}",
                    f32::from(pos.y),
                    f32::from(grid_start_y),
                    f32::from(y_in_grid),
                    f32::from(cell_height),
                    (f32::from(y_in_grid) / f32::from(cell_height)) as i32,
                );

                // Click above grid = header area → block selection.
                if y_in_grid < px(0.0) {
                    return Some((
                        entry.block_index,
                        entry.block_id,
                        Point::new(Line(0), Column(0)),
                        Side::Left,
                    ));
                }

                let visual_row = (f32::from(y_in_grid) / f32::from(cell_height)) as i32;
                let max_visual_row = entry.content_rows.saturating_sub(1) as i32;
                let visual_row = visual_row.clamp(0, max_visual_row);

                let row =
                    visual_row - entry.command_row_count as i32 - entry.grid_history_size as i32;
                let col_f = (f32::from(pos.x) - BLOCK_HEADER_PAD_X) / f32::from(cell_width);
                let col = col_f.max(0.0) as usize;
                let side = if col_f.fract() < 0.5 {
                    Side::Left
                } else {
                    Side::Right
                };

                return Some((
                    entry.block_index,
                    entry.block_id,
                    Point::new(Line(row), Column(col)),
                    side,
                ));
            }
        }

        None
    }
}
