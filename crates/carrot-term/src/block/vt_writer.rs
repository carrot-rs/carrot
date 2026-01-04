//! VT-to-ActiveBlock writer.
//!
//! Implements [`vte::ansi::Handler`] against a borrowed
//! [`ActiveBlock`]. This is the bridge that lets the vte parser
//! drive cell writes into the new Layer-1/Layer-2 data model
//! (carrot-grid `PageList` + per-block [`CellStyleAtlas`] +
//! [`HyperlinkStore`]).
//!
//! # Cursor state
//!
//! Cursor state is not a field of `ActiveBlock` — it lives on the
//! writer. `VtWriter` tracks cursor position, SGR state, scroll
//! region, tabs, modes, and charset mappings.
//!
//! # Scope
//!
//! Klasse A + C of the handler surface is complete:
//!
//! - Cursor motion (goto, move_up/down/forward/backward, save/restore).
//! - SGR colors + attributes via [`vt_color`] (Oklch).
//! - Erase-in-line / erase-chars / insert-blank / delete-chars
//!   (operate on the current-row buffer).
//! - Tabs (8-column default, configurable stops).
//! - Mode / private-mode state tracking (Insert, Origin, Autowrap,
//!   AltScreen signalled via [`VtReport`] channel to the Term owner).
//! - Charset designation + activation.
//! - VT reports via [`VtReportSink`] (DA, DSR, mode reports).
//! - OSC 8 hyperlinks interned into the block's [`HyperlinkStore`].
//! - Title announcements via an optional callback.
//!
//! Klasse B (scroll regions, `scroll_up/down`, `insert_blank_lines`,
//! `delete_lines`) needs a `row_mut` API on `PageList` — the current
//! writer commits per row and cannot reach back into committed
//! history. Those methods are no-ops for now; the legacy router
//! remains authoritative for TUI output while we wire the new API
//! in [`crate::term::Term`].

use std::mem;

use carrot_grid::{Cell, CellStyle, CellStyleFlags, CellStyleId, Color, HyperlinkId, TabStops};
use unicode_width::UnicodeWidthChar;

use super::active::ActiveBlock;
use super::vt_color;
use super::vt_report::{ModeReportState, VtReport, VtReportSink};
use crate::term::emoji::emoji_presentation;
use crate::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, LineClearMode,
    Mode, NamedMode, NamedPrivateMode, PrivateMode, Rgb, StandardCharset, TabulationClearMode,
};

/// Saved cursor + SGR snapshot for DECSC / DECRC.
#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: u16,
    fg: Color,
    bg: Color,
    flags: CellStyleFlags,
    underline_color: Option<Color>,
    hyperlink: HyperlinkId,
    active_charset: CharsetIndex,
}

/// Bitflag set for VT modes the writer tracks locally. Anything not
/// modelled explicitly is silently ignored — the bits still land in
/// this struct so callers can query the current state.
#[derive(Debug, Clone, Copy, Default)]
pub struct VtModes {
    /// Insert mode (IRM, SM/RM 4). When set, `input()` shifts the
    /// row right by one before writing.
    pub insert: bool,
    /// Line-feed / new-line mode (LNM). When set, LF acts as CR+LF.
    /// Default on in Carrot (matches shell conventions).
    pub lnm: bool,
    /// DEC origin mode (DECOM). When set, `goto` is scroll-region
    /// relative.
    pub origin: bool,
    /// DEC autowrap mode (DECAWM). When reset, writing past the
    /// right edge overwrites the last column instead of wrapping.
    pub autowrap: bool,
    /// DEC cursor-visible mode (DECTCEM).
    pub cursor_visible: bool,
    /// DEC alternate-screen mode (1049). Signalled to the session
    /// via [`VtReport`]; the writer itself keeps writing to its
    /// `ActiveBlock` — the `Term` layer swaps the target.
    pub alt_screen: bool,
    /// Application cursor keys (DECCKM).
    pub app_cursor: bool,
    /// Application keypad mode (DECPAM).
    pub app_keypad: bool,
    /// Reverse-video mode (DECSCNM).
    pub reverse_video: bool,
    /// Bracketed paste mode (2004).
    pub bracketed_paste: bool,
    /// Mouse protocols (1000..1006). Tracked as a u8 bitmask.
    pub mouse_modes: u8,
}

/// Persistent VT writer state — everything that must survive across
/// multiple `advance()` calls on the same terminal. A `Term` owns one
/// of these and lends it to short-lived [`VtWriter`] wrappers along
/// with the currently-active [`ActiveBlock`].
///
/// The block itself is **not** stored here because the active-block
/// pointer changes whenever OSC 133 starts a new command block.
/// Everything else (SGR, cursor, modes, partial row buffer) keeps
/// accumulating across those transitions.
#[derive(Debug, Clone)]
pub struct VtWriterState {
    cursor_row: usize,
    cursor_col: u16,
    cols: u16,
    rows: u16,
    fg: Color,
    bg: Color,
    flags: CellStyleFlags,
    underline_color: Option<Color>,
    hyperlink: HyperlinkId,
    row_buf: Vec<Cell>,
    row_dirty: bool,
    tabstops: TabStops,
    charsets: [StandardCharset; 4],
    active_charset: CharsetIndex,
    saved_cursor: Option<SavedCursor>,
    modes: VtModes,
    report_sink: Option<VtReportSink>,
    title: Option<String>,
    scroll_top: u16,
    scroll_bot: u16,
}

impl VtWriterState {
    /// Fresh state sized for `cols` × `rows`. `rows` is the viewport
    /// height — used for `clear_screen(Below)` and scroll-region
    /// defaults. Cursor starts at the origin, SGR at default.
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut row_buf = Vec::with_capacity(cols as usize);
        row_buf.resize(cols as usize, Cell::EMPTY);
        Self {
            cursor_row: 0,
            cursor_col: 0,
            cols,
            rows,
            fg: Color::DEFAULT_FG,
            bg: Color::DEFAULT_BG,
            flags: CellStyleFlags::empty(),
            underline_color: None,
            hyperlink: HyperlinkId::NONE,
            row_buf,
            row_dirty: false,
            tabstops: TabStops::new(cols as usize),
            charsets: [StandardCharset::Ascii; 4],
            active_charset: CharsetIndex::G0,
            saved_cursor: None,
            modes: VtModes {
                lnm: true,
                autowrap: true,
                cursor_visible: true,
                ..Default::default()
            },
            report_sink: None,
            title: None,
            scroll_top: 0,
            scroll_bot: rows.saturating_sub(1),
        }
    }

    /// Attach a [`VtReportSink`] for DA/DSR/mode-report replies.
    pub fn with_report_sink(mut self, sink: VtReportSink) -> Self {
        self.report_sink = Some(sink);
        self
    }

    /// Current cursor as `(row, col)`.
    pub fn cursor(&self) -> (usize, u16) {
        (self.cursor_row, self.cursor_col)
    }

    /// Active mode state.
    pub fn modes(&self) -> VtModes {
        self.modes
    }

    /// Window title last announced via OSC 0/2, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Viewport row count configured at construction.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Viewport column count configured at construction.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Dev-only: read the in-flight row buffer. Used by the dual-
    /// routing parity probe to surface partial-row content that
    /// hasn't been committed to the grid yet (commit happens on
    /// LF / autowrap / scroll-region ops). The row is always
    /// exactly `cols` cells wide.
    pub fn row_buf(&self) -> &[Cell] {
        &self.row_buf
    }

    /// Dev-only: `true` when the row buffer holds uncommitted
    /// writes. Paired with [`Self::row_buf`].
    pub fn row_dirty(&self) -> bool {
        self.row_dirty
    }
}

/// Short-lived wrapper binding a [`VtWriterState`] to an
/// [`ActiveBlock`] for one `Processor::advance` pass. Construct fresh
/// for every chunk of PTY bytes; the owned state lives on the
/// terminal and survives every block lifecycle transition.
pub struct VtWriter<'a> {
    block: &'a mut ActiveBlock,
    state: &'a mut VtWriterState,
}

impl<'a> VtWriter<'a> {
    /// Bind `state` to `block` for one parser pass. `state.cols` must
    /// match the block's grid capacity.
    pub fn new_in(state: &'a mut VtWriterState, block: &'a mut ActiveBlock) -> Self {
        let cap = block.grid().capacity().cols;
        debug_assert!(
            state.cols == cap,
            "VtWriter cols ({}) must match block grid cols ({cap})",
            state.cols,
        );
        Self { block, state }
    }

    /// Commit pending row-buffer changes to the underlying block.
    pub fn commit_row(&mut self) {
        if !self.state.row_dirty {
            return;
        }
        self.block.append_row(&self.state.row_buf);
        for cell in self.state.row_buf.iter_mut() {
            *cell = Cell::EMPTY;
        }
        self.state.row_dirty = false;
    }

    /// End the parser pass. No longer commits the partial row —
    /// `row_buf` lives on [`VtWriterState`] and is carried across
    /// PTY chunks so mid-row splits don't surface as spurious grid
    /// rows. Only an explicit LF / NEL / autowrap / scroll-region
    /// op calls [`Self::commit_row`]. Callers that need to flush on
    /// shutdown (replay, snapshot) invoke `commit_row` directly.
    pub fn finalize(self) {
        // Intentionally left empty — see doc above.
        let _ = self;
    }

    /// Current cursor as (row, col).
    pub fn cursor(&self) -> (usize, u16) {
        (self.state.cursor_row, self.state.cursor_col)
    }

    /// Configured viewport row count. Used by consumers that need to
    /// know how many rows the active viewport spans (for `clear_screen`
    /// extents, cursor clamping, and scroll-region defaults).
    pub fn rows(&self) -> u16 {
        self.state.rows
    }

    /// Active mode state.
    pub fn modes(&self) -> VtModes {
        self.state.modes
    }

    /// Window title last announced via OSC 0/2, if any.
    pub fn title(&self) -> Option<&str> {
        self.state.title.as_deref()
    }

    /// Interned id of the currently-active style. Calling this also
    /// registers the style in the block's atlas if not already present.
    pub fn current_style(&mut self) -> CellStyleId {
        let value = self.current_style_value();
        self.block.atlas_mut().intern(value)
    }

    /// Absolute row index the cursor currently occupies. Layer-1
    /// operations (scroll region edits) use absolute indices; the
    /// cursor row is already absolute, but this makes the intent
    /// clear at call sites.
    fn cursor_row_absolute(&self) -> usize {
        self.state.cursor_row
    }

    /// Scroll region as absolute `(top, bot)` indices into the
    /// underlying `PageList`. Converts the writer's local region
    /// (relative to the current viewport) into grid-row coordinates.
    fn scroll_region_rows(&self) -> (usize, usize) {
        let base = self
            .block
            .grid()
            .total_rows()
            .saturating_sub(self.state.rows as usize);
        (
            base + self.state.scroll_top as usize,
            base + self.state.scroll_bot as usize,
        )
    }

    /// Origin row (`(0,0)` in origin mode) as an absolute row index.
    fn scroll_region_origin_row(&self) -> usize {
        let base = self
            .block
            .grid()
            .total_rows()
            .saturating_sub(self.state.rows as usize);
        base + self.state.scroll_top as usize
    }

    fn current_style_value(&self) -> CellStyle {
        CellStyle {
            fg: self.state.fg,
            bg: self.state.bg,
            underline_color: self.state.underline_color,
            flags: self.state.flags,
            hyperlink: self.state.hyperlink,
        }
    }

    fn write_char_to_buffer(&mut self, c: char) {
        let translated = self.state.charsets[charset_ix(self.state.active_charset)].map(c);
        // UAX#11 width; emoji presentation coerces width-1 chars to 2.
        let width = match UnicodeWidthChar::width(translated) {
            Some(0) => {
                // Zero-width combiner attaches to the previous
                // grapheme cluster via the block's GraphemeStore.
                self.attach_combiner(translated);
                return;
            }
            None => return,
            Some(w) => w,
        };
        let width = if width == 1 && emoji_presentation(translated) {
            2
        } else {
            width
        };
        // Autowrap: if the incoming cell (possibly 2 wide) would spill,
        // finish this row first.
        if self.state.cursor_col as usize + width > self.state.row_buf.len() {
            if self.state.modes.autowrap {
                self.commit_row();
                self.state.cursor_col = 0;
                self.state.cursor_row = self.state.cursor_row.saturating_add(1);
            } else {
                // Autowrap off — overwrite last cell(s) of the row.
                self.state.cursor_col = self.state.cols.saturating_sub(width as u16);
            }
        }
        let style_id = self.current_style();
        let primary = if translated.is_ascii() {
            Cell::ascii(translated as u8, style_id)
        } else {
            Cell::codepoint(translated, style_id)
        };
        // Insert mode shifts cells right before writing. For a wide
        // char, shift twice so the ghost cell also lands cleanly.
        if self.state.modes.insert {
            for _ in 0..width {
                let idx = self.state.cursor_col as usize;
                if idx < self.state.row_buf.len() {
                    self.state.row_buf[idx..].rotate_right(1);
                    if let Some(last) = self.state.row_buf.last_mut() {
                        *last = Cell::EMPTY;
                    }
                }
            }
        }
        self.state.row_buf[self.state.cursor_col as usize] = primary;
        if width == 2 {
            // Ghost-cell immediately after a wide char — the renderer
            // skips this slot and reads the primary instead.
            let ghost_ix = self.state.cursor_col as usize + 1;
            if ghost_ix < self.state.row_buf.len() {
                self.state.row_buf[ghost_ix] = Cell::wide_2nd(style_id);
            }
        }
        self.state.row_dirty = true;
        self.state.cursor_col += width as u16;
    }

    /// Attach a zero-width combiner to the prior cell on the current
    /// row. The combined grapheme cluster is interned on the block's
    /// [`carrot_grid::GraphemeStore`] and the prior cell's tag is
    /// flipped to [`carrot_grid::CellTag::Grapheme`] so the renderer
    /// resolves the cluster via the store.
    ///
    /// If the prior cell is itself a `Wide2nd` ghost, this walks back
    /// one more column so combiners ride on the wide char's primary
    /// cell. Combiners at column 0 with no prior character are
    /// silently dropped — there is nothing to attach them to.
    fn attach_combiner(&mut self, combiner: char) {
        // Find prior cell index, skipping a Wide2nd ghost if present.
        let mut ix = match (self.state.cursor_col as usize).checked_sub(1) {
            Some(ix) => ix,
            None => return,
        };
        if ix >= self.state.row_buf.len() {
            return;
        }
        if matches!(self.state.row_buf[ix].tag(), carrot_grid::CellTag::Wide2nd) {
            ix = match ix.checked_sub(1) {
                Some(ix) => ix,
                None => return,
            };
        }
        let prior = self.state.row_buf[ix];
        let style = prior.style();
        // Reconstruct the base cluster text from the prior cell.
        let mut cluster = match prior.tag() {
            carrot_grid::CellTag::Ascii => {
                let byte = prior.content() as u8;
                if byte == 0 {
                    return;
                }
                String::from(byte as char)
            }
            carrot_grid::CellTag::Codepoint => {
                let cp = prior.content();
                match char::from_u32(cp) {
                    Some(c) => String::from(c),
                    None => return,
                }
            }
            carrot_grid::CellTag::Grapheme => {
                let idx = carrot_grid::GraphemeIndex(prior.content());
                match self.block.graphemes().get(idx) {
                    Some(s) => s.to_string(),
                    None => return,
                }
            }
            // No base to attach to (Wide2nd already skipped above).
            _ => return,
        };
        cluster.push(combiner);
        let id = self.block.graphemes_mut().intern(&cluster);
        if id.is_valid() {
            self.state.row_buf[ix] = Cell::grapheme(id, style);
            self.state.row_dirty = true;
        }
    }

    fn clamp_col(&self, col: usize) -> u16 {
        (col.min(self.state.cols.saturating_sub(1) as usize)) as u16
    }

    fn set_cursor(&mut self, row: usize, col: u16) {
        // Origin mode not yet applied — the writer has no scroll
        // region baseline yet. Left as a straight assignment until
        // Klasse B lands.
        self.state.cursor_row = row;
        self.state.cursor_col = self.clamp_col(col as usize);
    }

    fn send_report(&self, report: VtReport) {
        if let Some(sink) = &self.state.report_sink {
            sink.push(report);
        }
    }

    fn apply_attr(&mut self, attr: Attr) {
        match attr {
            Attr::Reset => {
                self.state.fg = Color::DEFAULT_FG;
                self.state.bg = Color::DEFAULT_BG;
                self.state.underline_color = None;
                self.state.flags = CellStyleFlags::empty();
                // Hyperlink is independent of SGR reset per spec.
            }
            Attr::Bold => self.state.flags = self.state.flags.insert(CellStyleFlags::BOLD),
            Attr::Dim => self.state.flags = self.state.flags.insert(CellStyleFlags::DIM),
            Attr::Italic => self.state.flags = self.state.flags.insert(CellStyleFlags::ITALIC),
            Attr::Underline
            | Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline => {
                self.state.flags = self.state.flags.insert(CellStyleFlags::UNDERLINE)
            }
            Attr::BlinkSlow | Attr::BlinkFast => {
                self.state.flags = self.state.flags.insert(CellStyleFlags::BLINK)
            }
            Attr::Reverse => self.state.flags = self.state.flags.insert(CellStyleFlags::REVERSE),
            Attr::Hidden => self.state.flags = self.state.flags.insert(CellStyleFlags::HIDDEN),
            Attr::Strike => {
                self.state.flags = self.state.flags.insert(CellStyleFlags::STRIKETHROUGH)
            }
            Attr::CancelBold => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::BOLD)
            }
            Attr::CancelBoldDim => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::BOLD);
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::DIM);
            }
            Attr::CancelItalic => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::ITALIC)
            }
            Attr::CancelUnderline => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::UNDERLINE)
            }
            Attr::CancelBlink => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::BLINK)
            }
            Attr::CancelReverse => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::REVERSE)
            }
            Attr::CancelHidden => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::HIDDEN)
            }
            Attr::CancelStrike => {
                self.state.flags = clear_flag(self.state.flags, CellStyleFlags::STRIKETHROUGH)
            }
            Attr::Foreground(c) => self.state.fg = vt_color::from_vte(c),
            Attr::Background(c) => self.state.bg = vt_color::from_vte(c),
            Attr::UnderlineColor(opt) => self.state.underline_color = opt.map(vt_color::from_vte),
        }
    }

    fn erase_cols(&mut self, start: u16, end: u16) {
        let start = start as usize;
        let end = (end as usize).min(self.state.row_buf.len());
        if start >= end {
            return;
        }
        for cell in &mut self.state.row_buf[start..end] {
            *cell = Cell::EMPTY;
        }
        self.state.row_dirty = true;
    }

    fn snapshot_cursor(&self) -> SavedCursor {
        SavedCursor {
            row: self.state.cursor_row,
            col: self.state.cursor_col,
            fg: self.state.fg,
            bg: self.state.bg,
            flags: self.state.flags,
            underline_color: self.state.underline_color,
            hyperlink: self.state.hyperlink,
            active_charset: self.state.active_charset,
        }
    }

    fn restore_snapshot(&mut self, s: SavedCursor) {
        self.state.cursor_row = s.row;
        self.state.cursor_col = s.col;
        self.state.fg = s.fg;
        self.state.bg = s.bg;
        self.state.flags = s.flags;
        self.state.underline_color = s.underline_color;
        self.state.hyperlink = s.hyperlink;
        self.state.active_charset = s.active_charset;
    }
}

fn charset_ix(ix: CharsetIndex) -> usize {
    match ix {
        CharsetIndex::G0 => 0,
        CharsetIndex::G1 => 1,
        CharsetIndex::G2 => 2,
        CharsetIndex::G3 => 3,
    }
}

fn clear_flag(flags: CellStyleFlags, flag: CellStyleFlags) -> CellStyleFlags {
    CellStyleFlags(flags.bits() & !flag.bits())
}

impl Handler for VtWriter<'_> {
    fn input(&mut self, c: char) {
        self.write_char_to_buffer(c);
    }

    fn linefeed(&mut self) {
        self.commit_row();
        if self.state.modes.lnm {
            self.state.cursor_col = 0;
        }
        self.state.cursor_row = self.state.cursor_row.saturating_add(1);
    }

    fn newline(&mut self) {
        self.commit_row();
        self.state.cursor_col = 0;
        self.state.cursor_row = self.state.cursor_row.saturating_add(1);
    }

    fn carriage_return(&mut self) {
        self.state.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.state.cursor_col > 0 {
            self.state.cursor_col -= 1;
        }
    }

    fn bell(&mut self) {
        // Audio bell is a higher-layer concern; no-op here.
    }

    fn substitute(&mut self) {
        self.write_char_to_buffer('?');
    }

    // ─── Cursor motion ──────────────────────────────────────────

    fn goto(&mut self, line: i32, col: usize) {
        let row = line.max(0) as usize;
        self.set_cursor(row, col as u16);
    }

    fn goto_line(&mut self, line: i32) {
        self.state.cursor_row = line.max(0) as usize;
    }

    fn goto_col(&mut self, col: usize) {
        self.state.cursor_col = self.clamp_col(col);
    }

    fn move_up(&mut self, n: usize) {
        self.state.cursor_row = self.state.cursor_row.saturating_sub(n);
    }

    fn move_down(&mut self, n: usize) {
        self.state.cursor_row = self.state.cursor_row.saturating_add(n);
    }

    fn move_forward(&mut self, col: usize) {
        let new = self.state.cursor_col as usize + col;
        self.state.cursor_col = self.clamp_col(new);
    }

    fn move_backward(&mut self, col: usize) {
        self.state.cursor_col = self.state.cursor_col.saturating_sub(col as u16);
    }

    fn move_up_and_cr(&mut self, row: usize) {
        self.state.cursor_row = self.state.cursor_row.saturating_sub(row);
        self.state.cursor_col = 0;
    }

    fn move_down_and_cr(&mut self, row: usize) {
        self.state.cursor_row = self.state.cursor_row.saturating_add(row);
        self.state.cursor_col = 0;
    }

    fn save_cursor_position(&mut self) {
        self.state.saved_cursor = Some(self.snapshot_cursor());
    }

    fn restore_cursor_position(&mut self) {
        if let Some(s) = self.state.saved_cursor {
            self.restore_snapshot(s);
        }
    }

    // ─── Erase ──────────────────────────────────────────────────

    fn clear_line(&mut self, mode: LineClearMode) {
        match mode {
            LineClearMode::Right => self.erase_cols(self.state.cursor_col, self.state.cols),
            LineClearMode::Left => self.erase_cols(0, self.state.cursor_col.saturating_add(1)),
            LineClearMode::All => self.erase_cols(0, self.state.cols),
        }
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        match mode {
            ClearMode::Below | ClearMode::All => {
                // Clear from cursor to end of line; rows below are a
                // no-op until Klasse B wires row_mut.
                self.erase_cols(self.state.cursor_col, self.state.cols);
            }
            ClearMode::Above => {
                self.erase_cols(0, self.state.cursor_col.saturating_add(1));
            }
            ClearMode::Saved => {
                // Clearing scrollback — not yet wired to PageList.
            }
        }
    }

    fn erase_chars(&mut self, n: usize) {
        let end = (self.state.cursor_col as usize).saturating_add(n);
        self.erase_cols(self.state.cursor_col, end as u16);
    }

    fn delete_chars(&mut self, n: usize) {
        let start = self.state.cursor_col as usize;
        if start >= self.state.row_buf.len() {
            return;
        }
        let n = n.min(self.state.row_buf.len() - start);
        self.state.row_buf[start..].rotate_left(n);
        let len = self.state.row_buf.len();
        for cell in self.state.row_buf[len - n..].iter_mut() {
            *cell = Cell::EMPTY;
        }
        self.state.row_dirty = true;
    }

    fn insert_blank(&mut self, n: usize) {
        let start = self.state.cursor_col as usize;
        if start >= self.state.row_buf.len() {
            return;
        }
        let n = n.min(self.state.row_buf.len() - start);
        self.state.row_buf[start..].rotate_right(n);
        for cell in self.state.row_buf[start..start + n].iter_mut() {
            *cell = Cell::EMPTY;
        }
        self.state.row_dirty = true;
    }

    // ─── SGR ────────────────────────────────────────────────────

    fn terminal_attribute(&mut self, attr: Attr) {
        self.apply_attr(attr);
    }

    fn reset_state(&mut self) {
        self.state.fg = Color::DEFAULT_FG;
        self.state.bg = Color::DEFAULT_BG;
        self.state.flags = CellStyleFlags::empty();
        self.state.underline_color = None;
        self.state.hyperlink = HyperlinkId::NONE;
        self.state.modes = VtModes {
            lnm: true,
            autowrap: true,
            cursor_visible: true,
            ..Default::default()
        };
        self.state.saved_cursor = None;
        self.state.charsets = [StandardCharset::Ascii; 4];
        self.state.active_charset = CharsetIndex::G0;
        self.state.tabstops = TabStops::new(self.state.cols as usize);
        self.state.cursor_row = 0;
        self.state.cursor_col = 0;
        for cell in &mut self.state.row_buf {
            *cell = Cell::EMPTY;
        }
        self.state.row_dirty = false;
    }

    // ─── Tabs ───────────────────────────────────────────────────

    fn set_horizontal_tabstop(&mut self) {
        let idx = self.state.cursor_col as usize;
        if idx < self.state.tabstops.len() {
            self.state.tabstops[idx] = true;
        }
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        match mode {
            TabulationClearMode::Current => {
                let idx = self.state.cursor_col as usize;
                if idx < self.state.tabstops.len() {
                    self.state.tabstops[idx] = false;
                }
            }
            TabulationClearMode::All => {
                self.state.tabstops.clear_all();
            }
        }
    }

    fn set_tabs(&mut self, interval: u16) {
        let interval = interval.max(1) as usize;
        self.state.tabstops.clear_all();
        for i in (interval..self.state.tabstops.len()).step_by(interval) {
            self.state.tabstops[i] = true;
        }
    }

    fn put_tab(&mut self, count: u16) {
        for _ in 0..count {
            let mut next = self.state.cursor_col as usize + 1;
            while next < self.state.tabstops.len() && !self.state.tabstops[next] {
                next += 1;
            }
            self.state.cursor_col = self.clamp_col(next);
        }
    }

    fn move_backward_tabs(&mut self, count: u16) {
        for _ in 0..count {
            let mut prev = self.state.cursor_col as usize;
            if prev == 0 {
                break;
            }
            prev -= 1;
            while prev > 0 && !self.state.tabstops[prev] {
                prev -= 1;
            }
            self.state.cursor_col = prev as u16;
        }
    }

    fn move_forward_tabs(&mut self, count: u16) {
        self.put_tab(count);
    }

    // ─── Charsets ───────────────────────────────────────────────

    fn set_active_charset(&mut self, idx: CharsetIndex) {
        self.state.active_charset = idx;
    }

    fn configure_charset(&mut self, idx: CharsetIndex, cs: StandardCharset) {
        self.state.charsets[charset_ix(idx)] = cs;
    }

    // ─── Modes ──────────────────────────────────────────────────

    fn set_mode(&mut self, mode: Mode) {
        if let Mode::Named(NamedMode::Insert) = mode {
            self.state.modes.insert = true;
        }
        if let Mode::Named(NamedMode::LineFeedNewLine) = mode {
            self.state.modes.lnm = true;
        }
    }

    fn unset_mode(&mut self, mode: Mode) {
        if let Mode::Named(NamedMode::Insert) = mode {
            self.state.modes.insert = false;
        }
        if let Mode::Named(NamedMode::LineFeedNewLine) = mode {
            self.state.modes.lnm = false;
        }
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if let PrivateMode::Named(named) = mode {
            match named {
                NamedPrivateMode::CursorKeys => self.state.modes.app_cursor = true,
                NamedPrivateMode::Origin => self.state.modes.origin = true,
                NamedPrivateMode::LineWrap => self.state.modes.autowrap = true,
                NamedPrivateMode::ShowCursor => self.state.modes.cursor_visible = true,
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.state.modes.alt_screen = true;
                }
                NamedPrivateMode::ReportMouseClicks => self.state.modes.mouse_modes |= 1 << 0,
                NamedPrivateMode::ReportCellMouseMotion => self.state.modes.mouse_modes |= 1 << 1,
                NamedPrivateMode::ReportAllMouseMotion => self.state.modes.mouse_modes |= 1 << 2,
                NamedPrivateMode::ReportFocusInOut => self.state.modes.mouse_modes |= 1 << 3,
                NamedPrivateMode::Utf8Mouse => self.state.modes.mouse_modes |= 1 << 4,
                NamedPrivateMode::SgrMouse => self.state.modes.mouse_modes |= 1 << 5,
                NamedPrivateMode::BracketedPaste => self.state.modes.bracketed_paste = true,
                _ => {}
            }
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if let PrivateMode::Named(named) = mode {
            match named {
                NamedPrivateMode::CursorKeys => self.state.modes.app_cursor = false,
                NamedPrivateMode::Origin => self.state.modes.origin = false,
                NamedPrivateMode::LineWrap => self.state.modes.autowrap = false,
                NamedPrivateMode::ShowCursor => self.state.modes.cursor_visible = false,
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                    self.state.modes.alt_screen = false;
                }
                NamedPrivateMode::ReportMouseClicks => self.state.modes.mouse_modes &= !(1 << 0),
                NamedPrivateMode::ReportCellMouseMotion => {
                    self.state.modes.mouse_modes &= !(1 << 1)
                }
                NamedPrivateMode::ReportAllMouseMotion => self.state.modes.mouse_modes &= !(1 << 2),
                NamedPrivateMode::ReportFocusInOut => self.state.modes.mouse_modes &= !(1 << 3),
                NamedPrivateMode::Utf8Mouse => self.state.modes.mouse_modes &= !(1 << 4),
                NamedPrivateMode::SgrMouse => self.state.modes.mouse_modes &= !(1 << 5),
                NamedPrivateMode::BracketedPaste => self.state.modes.bracketed_paste = false,
                _ => {}
            }
        }
    }

    fn report_mode(&mut self, mode: Mode) {
        let (code, state) = match mode {
            Mode::Named(NamedMode::Insert) => (4, bool_to_state(self.state.modes.insert)),
            Mode::Named(NamedMode::LineFeedNewLine) => (20, bool_to_state(self.state.modes.lnm)),
            _ => (0, ModeReportState::NotRecognised),
        };
        self.send_report(VtReport::ModeReport {
            mode: code,
            state,
            private: false,
        });
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        let state = if let PrivateMode::Named(n) = mode {
            bool_to_state(match n {
                NamedPrivateMode::CursorKeys => self.state.modes.app_cursor,
                NamedPrivateMode::Origin => self.state.modes.origin,
                NamedPrivateMode::LineWrap => self.state.modes.autowrap,
                NamedPrivateMode::ShowCursor => self.state.modes.cursor_visible,
                NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.state.modes.alt_screen,
                NamedPrivateMode::BracketedPaste => self.state.modes.bracketed_paste,
                _ => return,
            })
        } else {
            ModeReportState::NotRecognised
        };
        let code = match mode {
            PrivateMode::Named(NamedPrivateMode::CursorKeys) => 1,
            PrivateMode::Named(NamedPrivateMode::Origin) => 6,
            PrivateMode::Named(NamedPrivateMode::LineWrap) => 7,
            PrivateMode::Named(NamedPrivateMode::ShowCursor) => 25,
            PrivateMode::Named(NamedPrivateMode::SwapScreenAndSetRestoreCursor) => 1049,
            PrivateMode::Named(NamedPrivateMode::BracketedPaste) => 2004,
            _ => 0,
        };
        self.send_report(VtReport::ModeReport {
            mode: code,
            state,
            private: true,
        });
    }

    fn set_keypad_application_mode(&mut self) {
        self.state.modes.app_keypad = true;
    }

    fn unset_keypad_application_mode(&mut self) {
        self.state.modes.app_keypad = false;
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let rows = self.state.rows.max(1);
        let top = (top as u16).saturating_sub(1).min(rows.saturating_sub(1));
        let bot = bottom
            .map(|b| (b as u16).saturating_sub(1))
            .unwrap_or(rows.saturating_sub(1))
            .min(rows.saturating_sub(1));
        if top >= bot {
            // Degenerate region — reset to full screen per VT spec.
            self.state.scroll_top = 0;
            self.state.scroll_bot = rows.saturating_sub(1);
        } else {
            self.state.scroll_top = top;
            self.state.scroll_bot = bot;
        }
        // DECSTBM parks the cursor at home.
        self.state.cursor_row = self.scroll_region_origin_row();
        self.state.cursor_col = 0;
    }

    // ─── Reports ────────────────────────────────────────────────

    fn identify_terminal(&mut self, _intermediate: Option<char>) {
        self.send_report(VtReport::IdentifyTerminal);
    }

    fn device_status(&mut self, arg: usize) {
        match arg {
            5 => self.send_report(VtReport::TerminalStatus),
            6 => {
                // DSR 6 reports cursor pos — VT spec is 1-based.
                self.send_report(VtReport::CursorPosition {
                    row: (self.state.cursor_row as u16).saturating_add(1),
                    col: self.state.cursor_col.saturating_add(1),
                });
            }
            _ => {}
        }
    }

    // ─── Hyperlinks ────────────────────────────────────────────

    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        match hyperlink {
            Some(h) if !h.uri.is_empty() => {
                self.state.hyperlink = self.block.hyperlinks_mut().intern(&h.uri);
            }
            _ => self.state.hyperlink = HyperlinkId::NONE,
        }
    }

    // ─── Title + cursor appearance ─────────────────────────────

    fn set_title(&mut self, title: Option<String>) {
        self.state.title = title;
    }

    fn set_cursor_style(&mut self, _style: Option<CursorStyle>) {
        // Cursor appearance is consumer-side — no-op here.
    }

    fn set_cursor_shape(&mut self, _shape: CursorShape) {
        // Cursor appearance is consumer-side — no-op here.
    }

    // ─── Klasse B — scroll region edits via PageList ───────────

    fn scroll_up(&mut self, lines: usize) {
        self.commit_row();
        let (top, bot) = self.scroll_region_rows();
        self.block.grid_mut().scroll_region_up(top, bot, lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.commit_row();
        let (top, bot) = self.scroll_region_rows();
        self.block.grid_mut().scroll_region_down(top, bot, lines);
    }

    fn insert_blank_lines(&mut self, lines: usize) {
        self.commit_row();
        let (_, bot) = self.scroll_region_rows();
        let top = self.cursor_row_absolute();
        if top > bot {
            return;
        }
        self.block.grid_mut().scroll_region_down(top, bot, lines);
    }

    fn delete_lines(&mut self, lines: usize) {
        self.commit_row();
        let (_, bot) = self.scroll_region_rows();
        let top = self.cursor_row_absolute();
        if top > bot {
            return;
        }
        self.block.grid_mut().scroll_region_up(top, bot, lines);
    }

    fn reverse_index(&mut self) {
        self.commit_row();
        let (top, bot) = self.scroll_region_rows();
        if (self.cursor_row_absolute()) == top {
            self.block.grid_mut().scroll_region_down(top, bot, 1);
        } else {
            self.state.cursor_row = self.state.cursor_row.saturating_sub(1);
        }
    }

    // ─── Palette ops (no-op; palette lives in theme layer) ─────

    fn set_color(&mut self, _idx: usize, _rgb: Rgb) {}
    fn dynamic_color_sequence(&mut self, _prefix: String, _idx: usize, _suffix: &str) {}
    fn reset_color(&mut self, _idx: usize) {}
}

fn bool_to_state(b: bool) -> ModeReportState {
    if b {
        ModeReportState::Set
    } else {
        ModeReportState::Reset
    }
}

// Needed to satisfy the unused-import lint — `mem` is used by future
// extensions (row swap for scroll regions).
#[allow(dead_code)]
fn _keep_mem() -> usize {
    mem::size_of::<SavedCursor>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vte::ansi::{Processor, StdSyncHandler};

    fn feed(block: &mut ActiveBlock, cols: u16, bytes: &[u8]) -> (usize, u16) {
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(cols, 24);
        let mut writer = VtWriter::new_in(&mut state, block);
        processor.advance(&mut writer, bytes);
        let cursor = writer.cursor();
        // Finalize is a no-op in production; tests that inspect the
        // underlying grid need an explicit commit. The parity probe
        // overlays the uncommitted row_buf so end users never see a
        // lost row regardless of this.
        writer.commit_row();
        writer.finalize();
        cursor
    }

    #[test]
    fn plain_ascii_writes_one_row() {
        let mut block = ActiveBlock::new(10);
        let (_r, c) = feed(&mut block, 10, b"hello");
        assert_eq!(c, 5);
        assert_eq!(block.total_rows(), 1);
    }

    #[test]
    fn carriage_return_overwrites_from_col_zero() {
        let mut block = ActiveBlock::new(10);
        feed(&mut block, 10, b"hello\rxx");
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].content(), b'x' as u32);
        assert_eq!(row[1].content(), b'x' as u32);
        assert_eq!(row[2].content(), b'l' as u32);
    }

    #[test]
    fn backspace_retreats_cursor() {
        let mut block = ActiveBlock::new(10);
        feed(&mut block, 10, b"abc\x08\x08X");
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].content(), b'a' as u32);
        assert_eq!(row[1].content(), b'X' as u32);
    }

    #[test]
    fn sgr_red_foreground_applies_to_cells() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"\x1b[31mab");
        assert_ne!(writer.state.fg, Color::DEFAULT_FG);
        writer.finalize();
    }

    #[test]
    fn sgr_reset_restores_default_colors() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"\x1b[31mred\x1b[0m");
        assert_eq!(writer.state.fg, Color::DEFAULT_FG);
        assert_eq!(writer.state.bg, Color::DEFAULT_BG);
        writer.finalize();
    }

    #[test]
    fn cursor_goto_positions_cursor() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        // CSI 3 ; 5 H — goto row 3, col 5 (1-based in VT → 2, 4).
        processor.advance(&mut writer, b"\x1b[3;5H");
        let (row, col) = writer.cursor();
        assert_eq!(row, 2);
        assert_eq!(col, 4);
        writer.finalize();
    }

    #[test]
    fn save_and_restore_cursor_position() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"abc\x1b[s\x1b[10;1Hxx\x1b[u");
        let (_r, col) = writer.cursor();
        assert_eq!(col, 3);
        writer.finalize();
    }

    #[test]
    fn erase_chars_wipes_current_row_slice() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"abcdefg\x1b[4D\x1b[3X");
        // After 7 chars cursor is at col 7. `\x1b[4D` moves back 4 → col 3.
        // `\x1b[3X` erases 3 chars starting at col 3.
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].content(), b'a' as u32);
        assert_eq!(row[2].content(), b'c' as u32);
        assert_eq!(row[3].content(), 0); // erased
        assert_eq!(row[4].content(), 0); // erased
        assert_eq!(row[5].content(), 0); // erased
        assert_eq!(row[6].content(), b'g' as u32);
    }

    #[test]
    fn clear_line_right_leaves_left_intact() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"abcde\x1b[3D\x1b[K");
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].content(), b'a' as u32);
        assert_eq!(row[1].content(), b'b' as u32);
        assert_eq!(row[2].content(), 0);
        assert_eq!(row[3].content(), 0);
    }

    #[test]
    fn insert_blank_shifts_cells_right() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"abcd\x1b[4D\x1b[2@");
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].content(), 0);
        assert_eq!(row[1].content(), 0);
        assert_eq!(row[2].content(), b'a' as u32);
        assert_eq!(row[3].content(), b'b' as u32);
    }

    #[test]
    fn device_status_6_sends_cursor_position_report() {
        use super::super::vt_report::bounded;
        let mut block = ActiveBlock::new(10);
        let (sink, rx) = bounded();
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24).with_report_sink(sink);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"abc\x1b[6n");
        writer.finalize();
        let reports: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            reports.first(),
            Some(VtReport::CursorPosition { row: 1, col: 4 })
        ));
    }

    #[test]
    fn set_private_mode_toggles_autowrap() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"\x1b[?7l");
        assert!(!writer.state.modes.autowrap);
        processor.advance(&mut writer, b"\x1b[?7h");
        assert!(writer.state.modes.autowrap);
        writer.finalize();
    }

    #[test]
    fn osc_8_hyperlink_interns_into_block_store() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(
            &mut writer,
            b"\x1b]8;;https://example.org\x1b\\click\x1b]8;;\x1b\\",
        );
        writer.finalize();
        // Block store holds one hyperlink after the span.
        assert_eq!(block.hyperlinks().len(), 1);
    }

    #[test]
    fn tab_advances_to_next_stop() {
        let mut block = ActiveBlock::new(20);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(20, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"a\tb");
        let (_r, col) = writer.cursor();
        assert_eq!(col, 9);
        writer.finalize();
    }

    #[test]
    fn reset_state_clears_sgr_and_cursor() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"\x1b[31mabc\x1b[5;5H\x1bc");
        assert_eq!(writer.state.fg, Color::DEFAULT_FG);
        assert_eq!(writer.cursor(), (0, 0));
        writer.finalize();
    }

    #[test]
    fn window_title_is_captured() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"\x1b]0;carrot\x07");
        assert_eq!(writer.title(), Some("carrot"));
        writer.finalize();
    }

    #[test]
    fn decstbm_sets_scroll_region_and_resets_cursor() {
        let mut block = ActiveBlock::new(10);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(10, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        // CSI 5 ; 10 r — scroll region rows 5..10 (1-based → 4..9).
        processor.advance(&mut writer, b"\x1b[5;10r");
        assert_eq!(writer.state.scroll_top, 4);
        assert_eq!(writer.state.scroll_bot, 9);
        writer.finalize();
    }

    #[test]
    fn scroll_up_shifts_rows_inside_region() {
        // Prime a block with 5 rows, then run CSI S to scroll the full
        // region (1..=5) up by 1 row.
        let mut block = ActiveBlock::new(4);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(4, 5);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"aaaa\r\nbbbb\r\ncccc\r\ndddd\r\neeee");
        writer.commit_row();
        // Now scroll the full screen up by 1. Row 0 ('aaaa') must be
        // discarded inside the region, row 1 ('bbbb') now at row 0.
        processor.advance(&mut writer, b"\x1b[S");
        writer.finalize();
        let row0 = block.grid().row(0).expect("row 0");
        assert_eq!(row0[0].content(), b'b' as u32);
    }

    #[test]
    fn cjk_ideograph_writes_wide_cell_plus_ghost() {
        let mut block = ActiveBlock::new(6);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(6, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        // 你 is width=2 per UAX#11.
        processor.advance(&mut writer, "你好".as_bytes());
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        // col 0 holds the codepoint for 你
        assert_eq!(row[0].content(), '你' as u32);
        // col 1 is a Wide2nd ghost cell.
        assert_eq!(row[1].tag(), carrot_grid::CellTag::Wide2nd);
        // col 2 is the start of 好 (also width 2).
        assert_eq!(row[2].content(), '好' as u32);
        assert_eq!(row[3].tag(), carrot_grid::CellTag::Wide2nd);
    }

    #[test]
    fn ideograph_at_row_end_wraps_to_new_row() {
        // 5 cols wide; write three wide chars → the third (width 2)
        // doesn't fit on row 0 and must wrap instead of getting
        // shoved partially into col 4.
        let mut block = ActiveBlock::new(5);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(5, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, "你好世".as_bytes());
        writer.commit_row();
        let row0 = block.grid().row(0).expect("row 0");
        // First row holds 你 + ghost + 好 + ghost + empty.
        assert_eq!(row0[0].content(), '你' as u32);
        assert_eq!(row0[2].content(), '好' as u32);
        assert_eq!(row0[4].content(), 0);
        // 世 should have wrapped to row 1 column 0.
        let row1 = block.grid().row(1).expect("row 1");
        assert_eq!(row1[0].content(), '世' as u32);
    }

    #[test]
    fn combiner_attaches_to_prior_cell_as_grapheme() {
        // `a` + U+0301 → "á" as a two-codepoint cluster.
        let mut block = ActiveBlock::new(4);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(4, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, "a\u{0301}".as_bytes());
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        assert_eq!(row[0].tag(), carrot_grid::CellTag::Grapheme);
        let idx = carrot_grid::GraphemeIndex(row[0].content());
        assert_eq!(block.graphemes().get(idx), Some("a\u{0301}"));
        assert_eq!(row[1].content(), 0);
    }

    #[test]
    fn combiner_on_wide_char_walks_through_ghost() {
        // 你 (width 2) + U+0301 — the combiner attaches to 你, not
        // the ghost cell that follows it.
        let mut block = ActiveBlock::new(4);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(4, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, "你\u{0301}".as_bytes());
        writer.commit_row();
        let row = block.grid().row(0).expect("row 0");
        // Primary cell now references the grapheme store.
        assert_eq!(row[0].tag(), carrot_grid::CellTag::Grapheme);
        // Ghost cell tag is untouched.
        assert_eq!(row[1].tag(), carrot_grid::CellTag::Wide2nd);
    }

    #[test]
    fn combiner_at_start_of_row_is_dropped() {
        // No prior cell → combiner has nothing to attach to. The row
        // must stay empty rather than panic or accept a free-floating
        // grapheme.
        let mut block = ActiveBlock::new(4);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(4, 24);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, "\u{0301}".as_bytes());
        assert!(block.graphemes().is_empty());
    }

    #[test]
    fn reverse_index_at_top_scrolls_region_down() {
        // Park cursor at top of the scroll region, then RI — the region
        // should shift down, blanking the top row.
        let mut block = ActiveBlock::new(4);
        let mut processor = Processor::<StdSyncHandler>::new();
        let mut state = VtWriterState::new(4, 5);
        let mut writer = VtWriter::new_in(&mut state, &mut block);
        processor.advance(&mut writer, b"aaaa\r\nbbbb\r\ncccc\r\ndddd\r\neeee");
        writer.commit_row();
        // Move cursor to (1,1) — top of default scroll region. RI then
        // scrolls region down, which blanks the new top.
        processor.advance(&mut writer, b"\x1b[1;1H\x1bM");
        writer.finalize();
        let row0 = block.grid().row(0).expect("row 0");
        assert_eq!(row0[0].content(), 0);
    }
}
