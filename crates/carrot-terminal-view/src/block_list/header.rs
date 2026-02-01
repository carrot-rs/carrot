//! Block header view model + rendering helpers.
//!
//! Pure UI types built from block data (`RouterBlockMetadata` +
//! `LiveFrameRegion`). No dependency on the legacy `grid_snapshot`
//! types.

use std::time::Instant;

use carrot_term::block::{LiveFrameRegion, LiveFrameSource, RouterBlockMetadata};
use carrot_theme::Theme;
use inazuma::{Oklch, px};

use crate::constants::*;

/// UI-layer mirror of the live-frame region. Keeps the header
/// rendering crate-local and independent of backend types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveFrameHeader {
    pub source: LiveFrameSourceKind,
    pub reprint_count: u32,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveFrameSourceKind {
    ShellHint,
    SyncUpdate,
    Heuristic,
}

impl LiveFrameSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShellHint => "ShellHint",
            Self::SyncUpdate => "SyncUpdate",
            Self::Heuristic => "Heuristic",
        }
    }
}

impl From<&LiveFrameRegion> for LiveFrameHeader {
    fn from(lf: &LiveFrameRegion) -> Self {
        let source = match lf.source {
            LiveFrameSource::ShellHint => LiveFrameSourceKind::ShellHint,
            LiveFrameSource::SyncUpdate => LiveFrameSourceKind::SyncUpdate,
            LiveFrameSource::Heuristic => LiveFrameSourceKind::Heuristic,
        };
        Self {
            source,
            reprint_count: lf.reprint_count,
            height: lf.height,
        }
    }
}

/// View-model for one block's header. Built from v2 `RouterBlockMetadata`
/// plus an optional live-frame region from the active block.
#[derive(Clone, Debug)]
pub struct BlockHeaderView {
    pub is_error: bool,
    pub is_running: bool,
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
    pub duration_ms: Option<u64>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub live_frame: Option<LiveFrameHeader>,
}

impl BlockHeaderView {
    /// Build from v2 metadata + optional live-frame state.
    pub fn from_metadata(
        meta: &RouterBlockMetadata,
        is_running: bool,
        live_frame: Option<&LiveFrameRegion>,
    ) -> Self {
        Self {
            is_error: meta.is_error(),
            is_running,
            started_at: meta.started_at,
            finished_at: meta.finished_at,
            duration_ms: meta.duration_ms(),
            username: meta.username.clone(),
            hostname: meta.hostname.clone(),
            cwd: meta.cwd.clone(),
            git_branch: meta.git_branch.clone(),
            live_frame: live_frame.map(Into::into),
        }
    }

    /// Format the command duration for inline display.
    pub fn duration_display(&self) -> String {
        let ms = match self.duration_ms {
            Some(ms) => ms,
            None => match (self.started_at, self.finished_at) {
                (Some(s), Some(e)) => e
                    .checked_duration_since(s)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                (Some(s), None) => s.elapsed().as_millis() as u64,
                _ => return String::new(),
            },
        };
        format_duration_ms(ms)
    }

    /// Format the duration wrapped in parentheses for the metadata line.
    pub fn duration_display_parens(&self) -> String {
        let d = self.duration_display();
        if d.is_empty() {
            String::new()
        } else {
            format!("({d})")
        }
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        let s = ms as f64 / 1_000.0;
        if s < 10.0 {
            format!("{s:.2}s")
        } else {
            format!("{s:.1}s")
        }
    } else if ms < 3_600_000 {
        let m = ms / 60_000;
        let s = (ms % 60_000) / 1_000;
        format!("{m}m{s}s")
    } else {
        let h = ms / 3_600_000;
        let m = (ms % 3_600_000) / 60_000;
        format!("{h}h{m}m")
    }
}

/// Build the metadata text line for a block header.
///
/// Format: `"user  host  ~/cwd  git:(branch)  HH:MM  (duration)"`
pub fn build_metadata_text(header: &BlockHeaderView) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref user) = header.username {
        parts.push(user.clone());
    }
    if let Some(ref host) = header.hostname {
        parts.push(host.clone());
    }
    if let Some(ref cwd) = header.cwd {
        let display = if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            if cwd.starts_with(home_str.as_ref()) {
                format!("~{}", &cwd[home_str.len()..])
            } else {
                cwd.clone()
            }
        } else {
            cwd.clone()
        };
        parts.push(display);
    }
    if let Some(ref branch) = header.git_branch {
        parts.push(format!("git:({})", branch));
    }

    if let Some(started) = header.started_at {
        let elapsed = started.elapsed();
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let at_start = now - elapsed;
        let time_display = at_start
            .format(time::macros::format_description!("[hour]:[minute]"))
            .unwrap_or_else(|_| "--:--".to_string());
        parts.push(time_display);
    }

    parts.push(header.duration_display_parens());
    parts.join("  ")
}

/// Theme-coupled accent color for the "running" badge.
pub fn running_badge_color(theme: &Theme) -> Oklch {
    accent_color(theme)
}

/// Sticky-header top padding (keeps the chevron aligned on every row).
pub fn header_top_padding() -> inazuma::Pixels {
    px(4.0)
}
