//! Custom render plugin registry.
//!
//! `CellTag::CustomRender` lets a block reserve a cell range for a
//! user-provided renderer — inline charts, markdown previews, syntax-
//! highlighted code blocks, whatever the plugin authors want. This
//! module owns the **protocol**: the trait every plugin implements
//! and the registry the renderer consults to dispatch.
//!
//! The eventual Wasm integration (via `wasmtime`) wraps each loaded
//! module in a [`CustomRenderer`] impl that forwards `render_region`
//! to the guest's exported `render` function. Until that lands, the
//! registry still works end-to-end with in-process Rust plugins
//! (typical for built-ins: mermaid, markdown, table formatter).
//!
//! # What this module is NOT
//!
//! - Not a wasmtime loader — that lives behind the `carrot-extension-host`
//!   gate when the extension rewrite lands.
//! - Not a layout engine — plugins describe draw commands; the host
//!   renderer still owns layout + clipping.
//! - Not a sandbox — input validation / resource limits are enforced
//!   by the future wasmtime wrapper, not by this registry.

use crate::cell::ShapedRunIndex;

/// Opaque handle for a plugin registration. Stored inside cell content
/// via `CellTag::CustomRender`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CustomRenderIndex(pub u32);

impl CustomRenderIndex {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// Bounding rectangle a plugin is asked to render into. Cells are the
/// unit; the renderer converts to pixels. `rows = 0` or `cols = 0`
/// means the plugin has nothing to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomRenderRect {
    pub start_row: u32,
    pub start_col: u16,
    pub rows: u32,
    pub cols: u16,
}

impl CustomRenderRect {
    pub fn is_empty(self) -> bool {
        self.rows == 0 || self.cols == 0
    }
}

/// Draw primitive a plugin emits. Host renderer translates to wgpu
/// calls. Keeping the vocabulary small means the plugin contract is
/// stable across wasmtime ABI bumps.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomDraw {
    /// Paint a solid rectangle in OKLCH. `(l, c, h, a)` quadruplet.
    FillRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    /// Draw a line of text at the given origin, inheriting the host
    /// font + size. Colour is OKLCH.
    Text {
        x: f32,
        y: f32,
        text: String,
        color: [f32; 4],
    },
    /// Reuse a shaped-run index the host has already cached. Used by
    /// plugins that need full font-fallback behaviour without owning
    /// the shaper.
    ShapedRun {
        x: f32,
        y: f32,
        run: ShapedRunIndex,
        color: [f32; 4],
    },
}

/// Every plugin implements this. The registry dispatches by index —
/// multiple plugins can be registered in the same block.
pub trait CustomRenderer: Send + Sync {
    /// Plugin name for debug / diagnostics.
    fn name(&self) -> &str;

    /// Emit draw commands for the given rectangle. The host passes
    /// cell coordinates; the plugin computes pixels based on the
    /// supplied `cell_width` / `cell_height`.
    fn render_region(
        &self,
        rect: CustomRenderRect,
        cell_width: f32,
        cell_height: f32,
    ) -> Vec<CustomDraw>;
}

/// Shared plugin registry per block. Insertion returns a fresh
/// `CustomRenderIndex` suitable for stashing into `Cell::custom()`.
#[derive(Default)]
pub struct CustomRenderRegistry {
    entries: Vec<Box<dyn CustomRenderer>>,
}

impl CustomRenderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn register(&mut self, renderer: Box<dyn CustomRenderer>) -> CustomRenderIndex {
        let ix = CustomRenderIndex(self.entries.len() as u32);
        self.entries.push(renderer);
        ix
    }

    pub fn get(&self, ix: CustomRenderIndex) -> Option<&dyn CustomRenderer> {
        self.entries.get(ix.0 as usize).map(|b| b.as_ref())
    }

    /// Render a rectangle via the plugin at `ix`. Returns an empty
    /// vector when the plugin is missing (defensive — avoids panics
    /// if a stale index survives a registry reset).
    pub fn render(
        &self,
        ix: CustomRenderIndex,
        rect: CustomRenderRect,
        cell_width: f32,
        cell_height: f32,
    ) -> Vec<CustomDraw> {
        match self.get(ix) {
            Some(plugin) => plugin.render_region(rect, cell_width, cell_height),
            None => Vec::new(),
        }
    }
}

impl std::fmt::Debug for CustomRenderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomRenderRegistry")
            .field("entries", &self.entries.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedFillPlugin {
        name: &'static str,
        color: [f32; 4],
    }

    impl CustomRenderer for FixedFillPlugin {
        fn name(&self) -> &str {
            self.name
        }
        fn render_region(
            &self,
            rect: CustomRenderRect,
            cell_w: f32,
            cell_h: f32,
        ) -> Vec<CustomDraw> {
            if rect.is_empty() {
                return Vec::new();
            }
            vec![CustomDraw::FillRect {
                x: rect.start_col as f32 * cell_w,
                y: rect.start_row as f32 * cell_h,
                w: rect.cols as f32 * cell_w,
                h: rect.rows as f32 * cell_h,
                color: self.color,
            }]
        }
    }

    #[test]
    fn empty_registry_reports_empty() {
        let r = CustomRenderRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn register_returns_sequential_indices() {
        let mut r = CustomRenderRegistry::new();
        let a = r.register(Box::new(FixedFillPlugin {
            name: "a",
            color: [0.5, 0.1, 30.0, 1.0],
        }));
        let b = r.register(Box::new(FixedFillPlugin {
            name: "b",
            color: [0.7, 0.2, 60.0, 1.0],
        }));
        assert_eq!(a.as_u32(), 0);
        assert_eq!(b.as_u32(), 1);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn get_returns_plugin_by_index() {
        let mut r = CustomRenderRegistry::new();
        let ix = r.register(Box::new(FixedFillPlugin {
            name: "mermaid",
            color: [0.5, 0.1, 30.0, 1.0],
        }));
        assert_eq!(r.get(ix).unwrap().name(), "mermaid");
    }

    #[test]
    fn unknown_index_returns_none() {
        let r = CustomRenderRegistry::new();
        assert!(r.get(CustomRenderIndex(42)).is_none());
    }

    #[test]
    fn render_dispatches_through_plugin() {
        let mut r = CustomRenderRegistry::new();
        let ix = r.register(Box::new(FixedFillPlugin {
            name: "md",
            color: [0.8, 0.05, 120.0, 1.0],
        }));
        let draws = r.render(
            ix,
            CustomRenderRect {
                start_row: 2,
                start_col: 3,
                rows: 4,
                cols: 8,
            },
            10.0,
            20.0,
        );
        assert_eq!(draws.len(), 1);
        match &draws[0] {
            CustomDraw::FillRect { x, y, w, h, color } => {
                assert_eq!(*x, 30.0);
                assert_eq!(*y, 40.0);
                assert_eq!(*w, 80.0);
                assert_eq!(*h, 80.0);
                assert_eq!(*color, [0.8, 0.05, 120.0, 1.0]);
            }
            _ => panic!("expected FillRect"),
        }
    }

    #[test]
    fn render_with_missing_plugin_returns_empty() {
        let r = CustomRenderRegistry::new();
        let draws = r.render(
            CustomRenderIndex(99),
            CustomRenderRect {
                start_row: 0,
                start_col: 0,
                rows: 1,
                cols: 1,
            },
            10.0,
            20.0,
        );
        assert!(draws.is_empty());
    }

    #[test]
    fn empty_rect_produces_no_draws() {
        let mut r = CustomRenderRegistry::new();
        let ix = r.register(Box::new(FixedFillPlugin {
            name: "x",
            color: [1.0, 0.0, 0.0, 1.0],
        }));
        let draws = r.render(
            ix,
            CustomRenderRect {
                start_row: 0,
                start_col: 0,
                rows: 0,
                cols: 5,
            },
            10.0,
            20.0,
        );
        assert!(draws.is_empty());
    }

    #[test]
    fn custom_render_index_as_u32() {
        assert_eq!(CustomRenderIndex(7).as_u32(), 7);
    }

    #[test]
    fn draw_variants_equal() {
        let a = CustomDraw::Text {
            x: 0.0,
            y: 0.0,
            text: "hi".into(),
            color: [1.0, 0.0, 0.0, 1.0],
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn debug_output_counts_entries() {
        let mut r = CustomRenderRegistry::new();
        r.register(Box::new(FixedFillPlugin {
            name: "a",
            color: [0.0; 4],
        }));
        let s = format!("{r:?}");
        assert!(s.contains("entries"));
    }
}
