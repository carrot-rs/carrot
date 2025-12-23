/// Shell metadata received via OSC 7777;carrot-precmd.
///
/// Deserialized from hex-encoded JSON sent by the shell precmd hook.
/// Contains environment context for updating UI chips and block headers.
///
/// Extensible with `#[serde(default)]` — new fields can be added without
/// breaking shells that don't send them yet.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ShellMetadataPayload {
    pub cwd: String,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub git_root: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_duration_ms: Option<u64>,
    pub shell: Option<String>,
}

/// TUI-mode hint received via OSC 7777;carrot-tui-hint.
///
/// Emitted by the shell preexec hook immediately before a known TUI
/// command starts, so the terminal can activate a live-frame region on
/// the upcoming block before the first output byte arrives. Avoids the
/// "first frame stacks once before the heuristic arms" window.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct TuiHintPayload {
    /// Whether this command should be treated as a TUI redraw target.
    pub tui_mode: Option<bool>,
    /// Reserved for future use: whether the shell hints the TUI would
    /// prefer alt-screen rendering. Currently informational only.
    pub prefer_alt_screen: Option<bool>,
}
