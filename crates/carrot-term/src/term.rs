//! Slim `Term<T>` — v2-native terminal core.
//!
//! After the Phase-G migration the legacy `BlockGridRouter` +
//! `Grid<Cell>` + `Handler` stack is gone. `Term<T>` now holds
//! exactly what the block pipeline needs:
//!
//! - [`block::BlockRouter`] for per-command block lifecycle
//! - [`block::VtWriterState`] + a dedicated `Processor` for the
//!   VT state machine
//! - Shared colour palette + terminal config
//! - Event proxy for wake-ups / clipboard / title changes
//!
//! There is no longer an "inactive grid" (alt-screen is a v2 concept
//! via [`block::AltScreenState`] + TUI-detector), no separate
//! cursor field (A10: cursor lives in `carrot-cmdline` for shell
//! blocks and in `VtWriterState` for TUI blocks), no scrollback-
//! history counter (the router tracks frozen blocks directly).

use std::time::Instant;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::block::{BlockRouter, VtWriter, VtWriterState};
use crate::event::EventListener;
use crate::vte::ansi::{Processor, StdSyncHandler};

pub mod color;
pub mod emoji;
pub mod mode;

pub use mode::TermMode;

/// Clipboard target (OSC 52). Re-exported at the crate root so event
/// consumers can match on `Clipboard` / `Selection` without reaching
/// into this module.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ClipboardType {
    /// System clipboard (`c`).
    Clipboard,
    /// Primary selection (`p` / `s`).
    Selection,
}

/// Minimum number of columns a terminal can be resized to.
pub const MIN_COLUMNS: usize = 2;

/// Minimum number of visible lines.
pub const MIN_SCREEN_LINES: usize = 1;

/// TUI-awareness policy. Controls which signals flip the live-frame
/// detector. Kept identical in shape to the block variant so
/// settings that predate the migration still load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum TuiAwareness {
    /// DEC 2026 + shell hint + cursor-up heuristic.
    #[default]
    Full,
    /// DEC 2026 + shell hint only.
    StrictProtocol,
    /// All signals disabled.
    Off,
}

impl TuiAwareness {
    pub fn protocol_enabled(self) -> bool {
        matches!(self, Self::Full | Self::StrictProtocol)
    }
    pub fn shell_hint_enabled(self) -> bool {
        matches!(self, Self::Full | Self::StrictProtocol)
    }
    pub fn heuristic_enabled(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Configuration for a [`Term`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Upper bound on the router's frozen-block retention.
    pub scrolling_history: usize,
    /// OSC 52 behaviour.
    pub osc52: Osc52,
    /// TUI-awareness policy.
    pub tui_awareness: TuiAwareness,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scrolling_history: 10_000,
            osc52: Osc52::default(),
            tui_awareness: TuiAwareness::default(),
        }
    }
}

/// OSC 52 handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum Osc52 {
    Disabled,
    #[default]
    OnlyCopy,
    OnlyPaste,
    CopyPaste,
}

/// Trait describing the terminal's visible shape. Kept here (post
/// legacy-grid deletion) because [`Term::resize`] and the PTY bridge
/// both want to accept anything that can report cols / rows.
///
/// The convenience methods (`last_column`, `topmost_line`,
/// `bottommost_line`) mirror the legacy `grid::Dimensions` surface
/// so consumers that translate between viewport-relative and
/// absolute coordinates continue to compile.
pub trait Dimensions {
    fn total_lines(&self) -> usize;
    fn screen_lines(&self) -> usize;
    fn columns(&self) -> usize;
    fn history_size(&self) -> usize {
        self.total_lines().saturating_sub(self.screen_lines())
    }
    fn last_column(&self) -> crate::index::Column {
        crate::index::Column(self.columns().saturating_sub(1))
    }
    fn topmost_line(&self) -> crate::index::Line {
        crate::index::Line(-(self.history_size() as i32))
    }
    fn bottommost_line(&self) -> crate::index::Line {
        crate::index::Line(self.screen_lines() as i32 - 1)
    }
}

#[cfg(test)]
impl Dimensions for (usize, usize) {
    fn total_lines(&self) -> usize {
        self.0
    }
    fn screen_lines(&self) -> usize {
        self.0
    }
    fn columns(&self) -> usize {
        self.1
    }
}

/// V2-native terminal core.
pub struct Term<T> {
    /// Block router — owns frozen + active blocks + prompt buffer.
    pub(crate) block_router: BlockRouter,
    /// Persistent VT writer state (cursor, SGR, modes, tabs).
    pub(crate) vt_state: VtWriterState,
    /// VT parser for the writer.
    pub(crate) vt_parser: Processor<StdSyncHandler>,
    /// Palette for ANSI colour resolution.
    pub(crate) colors: color::Colors,
    /// Global terminal modes (SHOW_CURSOR, ALT_SCREEN, SYNC_UPDATE,
    /// keyboard protocol bits, etc).
    pub(crate) mode: TermMode,
    /// Event proxy for wake-ups / title changes / clipboard.
    pub(crate) event_proxy: T,
    /// Current window title as set by OSC 0 / 2.
    pub(crate) title: Option<String>,
    /// When the current synchronized update (DEC 2026) began.
    /// Read by callers that want to time out long-running frames.
    #[allow(dead_code)]
    pub(crate) sync_update_started_at: Option<Instant>,
    /// Pending exit code held back during a synchronized update so
    /// CommandEnd arriving mid-frame finalises at ESU, not inline.
    pub(crate) pending_finalize: Option<i32>,
    /// Config — scrollback cap, OSC 52, TUI policy.
    pub(crate) config: Config,
    /// Viewport cols / rows (cached for `resize` diff).
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    /// Whether the terminal has window focus (drives cursor blink +
    /// focus-in/out VT reports).
    pub is_focused: bool,
}

impl<T> Term<T> {
    /// Construct a terminal sized for `dimensions`.
    pub fn new<D: Dimensions>(config: Config, dimensions: &D, event_proxy: T) -> Term<T> {
        let cols = dimensions.columns() as u16;
        let rows = dimensions.screen_lines() as u16;
        Term {
            block_router: BlockRouter::new(cols),
            vt_state: VtWriterState::new(cols, rows),
            vt_parser: Processor::<StdSyncHandler>::new(),
            colors: color::Colors::default(),
            mode: TermMode::default(),
            event_proxy,
            title: None,
            sync_update_started_at: None,
            pending_finalize: None,
            config,
            cols,
            rows,
            is_focused: false,
        }
    }

    /// Drive the v2 VT pipeline forward by one chunk of PTY bytes.
    pub fn advance(&mut self, bytes: &[u8]) {
        let mut target = self.block_router.active();
        let block = target.as_active_mut();
        let mut writer = VtWriter::new_in(&mut self.vt_state, block);
        self.vt_parser.advance(&mut writer, bytes);
        writer.finalize();
    }

    /// Viewport columns.
    pub fn columns(&self) -> usize {
        self.cols as usize
    }

    /// Viewport rows.
    pub fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    /// Current mode flags.
    #[inline]
    pub fn mode(&self) -> &TermMode {
        &self.mode
    }

    /// Current colour palette (modifiable by OSC 4 / OSC 10 / …).
    pub fn colors(&self) -> &color::Colors {
        &self.colors
    }

    /// Config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Event proxy.
    pub fn event_proxy(&self) -> &T {
        &self.event_proxy
    }

    /// Read-only router access for consumers that render + search.
    pub fn block_router(&self) -> &BlockRouter {
        &self.block_router
    }

    /// Mutable router access — used by OSC 133 dispatch + UI glue.
    pub fn block_router_mut(&mut self) -> &mut BlockRouter {
        &mut self.block_router
    }

    /// Convenience wrappers dispatched by higher layers around OSC 133.
    pub fn route_to_prompt(&mut self) {
        self.block_router.on_prompt_start();
    }

    pub fn route_to_new_block(&mut self, command: String) {
        self.block_router.set_pending_command(command);
        let _ = self.block_router.on_command_start();
    }

    pub fn route_finalize_block(&mut self, exit_code: i32) {
        if self.mode.contains(TermMode::SYNC_UPDATE) {
            self.pending_finalize = Some(exit_code);
            return;
        }
        let _ = self.block_router.on_command_end(exit_code);
    }

    /// Queue a command for the next block-start (UI → Enter).
    pub fn set_pending_block_command(&mut self, command: String) {
        self.block_router.set_pending_command(command);
    }

    /// Lock-once snapshot for the renderer.
    pub fn render_view(&self) -> crate::block::RenderView {
        crate::block::RenderView::build(
            &self.block_router,
            &self.vt_state,
            self.block_router.display_state(),
            (self.cols, self.rows),
        )
    }

    /// Resize the terminal to match `size`. Both the router and the
    /// VT state rebuild at the new width; frozen blocks stay at their
    /// original width (no data-reflow per plan A5 — soft-wrap is
    /// display-only).
    pub fn resize<S: Dimensions>(&mut self, size: S) {
        let new_cols = size.columns() as u16;
        let new_rows = size.screen_lines() as u16;
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }
        self.cols = new_cols;
        self.rows = new_rows;
        self.block_router.resize(new_cols);
        self.vt_state = VtWriterState::new(new_cols, new_rows);
    }

    /// Apply a new config. Live blocks are not reflowed.
    pub fn set_options(&mut self, options: Config)
    where
        T: EventListener,
    {
        self.config = options;
    }

    /// Terminal title last set by OSC 0 / 2.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Called when the PTY's child process exits. Closes any open
    /// block cleanly so the final command's output isn't stuck in
    /// "running" state forever.
    pub fn exit(&mut self) {
        let _ = self.block_router.on_command_end(0);
    }
}
