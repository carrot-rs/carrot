//! Known TUI commands that benefit from live-frame rendering.
//!
//! These commands either (a) render in non-alt-screen mode with the
//! log-update redraw pattern (cursor-up + clear + reprint) or (b) are
//! reliably worth a pre-emptive live-frame activation because they cover
//! the viewport. The terminal consumes this list to:
//!
//! 1. Emit the list to the spawned shell as the `CARROT_KNOWN_TUIS` env
//!    var (colon-separated). The shell preexec hook emits an OSC 7777
//!    `carrot-tui-hint` payload when the user runs one of these, and the
//!    terminal activates a `LiveFrameRegion` with source `ShellHint`
//!    before the first output byte arrives.
//! 2. Keep one source of truth for the list — instead of hardcoding it in
//!    four shell scripts (zsh, bash, fish, nushell).

/// The list of known TUI command names. Matching is performed on the
/// first whitespace-separated token of the command line, after stripping
/// any leading path (so `vim`, `/usr/bin/vim`, and `./vim` all match).
pub const KNOWN_TUI_COMMANDS: &[&str] = &[
    // Carrot-first: AI agents render their interactive shells in-place.
    "claude",
    "codex",
    "gemini",
    "aider",
    "cline",
    "goose",
    // Editors
    "vim",
    "nvim",
    "neovim",
    "vi",
    "helix",
    "hx",
    "emacs",
    "nano",
    "micro",
    "kakoune",
    "kak",
    // System monitors
    "htop",
    "btop",
    "btm",
    "glances",
    "iotop",
    "iftop",
    "nvtop",
    "atop",
    // Pagers
    "less",
    "most",
    "moar",
    // Git TUIs
    "lazygit",
    "tig",
    "gitui",
    // Kubernetes / container TUIs
    "k9s",
    "lazydocker",
    "ctop",
    // File managers
    "ranger",
    "yazi",
    "nnn",
    "lf",
    "mc",
    "vifm",
    // Fuzzy finders / launchers (full-screen by default)
    "fzf",
    // Database TUIs
    "pgcli",
    "mycli",
    "litecli",
    "usql",
    // Network / monitoring
    "bandwhich",
    "bmon",
    "gping",
    // Multiplexers (treat as TUIs when run interactively)
    "tmux",
    "zellij",
    "screen",
    // Misc
    "taskwarrior-tui",
    "oha",
    "hyperfine",
];

/// Returns `true` when the given command line's first token matches a
/// known TUI. The path prefix (if any) is stripped before comparison.
///
/// Examples:
/// - `is_known_tui("claude")` → true
/// - `is_known_tui("/usr/local/bin/vim foo.txt")` → true
/// - `is_known_tui("./nvim")` → true
/// - `is_known_tui("cargo build")` → false
pub fn is_known_tui(command_line: &str) -> bool {
    let first = command_line.split_whitespace().next().unwrap_or("");
    let bare = first.rsplit('/').next().unwrap_or(first);
    KNOWN_TUI_COMMANDS.contains(&bare)
}

/// Returns the colon-separated list of known TUI commands, suitable for
/// export as the `CARROT_KNOWN_TUIS` environment variable.
pub fn known_tuis_env_value() -> String {
    KNOWN_TUI_COMMANDS.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_bare_command() {
        assert!(is_known_tui("claude"));
        assert!(is_known_tui("vim"));
        assert!(is_known_tui("htop"));
    }

    #[test]
    fn matches_with_path_prefix() {
        assert!(is_known_tui("/usr/local/bin/vim foo.txt"));
        assert!(is_known_tui("./nvim"));
        assert!(is_known_tui("/opt/homebrew/bin/claude --continue"));
    }

    #[test]
    fn matches_with_arguments() {
        assert!(is_known_tui("vim README.md"));
        assert!(is_known_tui("less -R /var/log/system.log"));
        assert!(is_known_tui("claude --continue foo bar"));
    }

    #[test]
    fn non_tui_returns_false() {
        assert!(!is_known_tui("cargo build"));
        assert!(!is_known_tui("ls -la"));
        assert!(!is_known_tui("echo hi"));
        assert!(!is_known_tui("git status"));
    }

    #[test]
    fn empty_input_returns_false() {
        assert!(!is_known_tui(""));
        assert!(!is_known_tui("   "));
    }

    #[test]
    fn env_value_is_colon_separated() {
        let value = known_tuis_env_value();
        assert!(value.contains("claude"));
        assert!(value.contains(":"));
        assert!(!value.starts_with(':'));
        assert!(!value.ends_with(':'));
    }
}
