//! Shell-kind re-export + Carrot-specific helpers.
//!
//! The canonical `ShellKind` enum lives in `inazuma-util` with 13
//! variants (Posix, Bash, Zsh, Csh, Tcsh, Rc, Fish, PowerShell,
//! Pwsh, Nushell, Cmd, Xonsh, Elvish). carrot-cmdline does not
//! duplicate that enum — it re-exports it and adds two app-level
//! helpers on top:
//!
//! - `carrot_default_shell()` — Carrot's default is Nushell; inazuma
//!   defaults unknown programs to `Posix` so we can't rely on
//!   `ShellKind::default()` here.
//! - `display_name(kind)` — UI-facing label. Inazuma's `Display`
//!   impl renders `Nushell` as `"nu"` (program name); cmdline's
//!   chips want the product name `"nushell"`.
//!
//! Everything else (`ShellKind::new(path, is_windows)` for basename
//! detection, quoting, separator semantics) comes straight from
//! inazuma-util.

use std::path::Path;

pub use inazuma_util::shell::ShellKind;

/// Carrot's default shell posture — Nushell. Used when the PTY
/// hasn't announced a shell yet (OSC 7777 hook is pending) and no
/// `$SHELL` is available.
pub fn carrot_default_shell() -> ShellKind {
    ShellKind::Nushell
}

/// Detect the shell kind from a `$SHELL` path. Falls back to
/// [`carrot_default_shell`] for unrecognised programs — distinct
/// from `inazuma_util::ShellKind::new` which falls back to `Posix`.
pub fn shell_kind_from_path(path: &Path) -> ShellKind {
    let kind = ShellKind::new(path, cfg!(windows));
    if matches!(kind, ShellKind::Posix) && !is_posix_basename(path) {
        carrot_default_shell()
    } else {
        kind
    }
}

fn is_posix_basename(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(stem, "sh" | "dash" | "ksh" | "ash")
}

/// Product-name label for chips / logs. Nushell is "nushell" (not
/// "nu"), everything else delegates to the `Display` impl.
pub fn display_name(kind: ShellKind) -> &'static str {
    match kind {
        ShellKind::Nushell => "nushell",
        ShellKind::Bash => "bash",
        ShellKind::Zsh => "zsh",
        ShellKind::Fish => "fish",
        ShellKind::Posix => "sh",
        ShellKind::Csh => "csh",
        ShellKind::Tcsh => "tcsh",
        ShellKind::Rc => "rc",
        ShellKind::PowerShell => "powershell",
        ShellKind::Pwsh => "pwsh",
        ShellKind::Cmd => "cmd",
        ShellKind::Xonsh => "xonsh",
        ShellKind::Elvish => "elvish",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_kind_from_path_detects_bash_and_zsh_distinctly() {
        assert_eq!(
            shell_kind_from_path(Path::new("/bin/bash")),
            ShellKind::Bash,
        );
        assert_eq!(
            shell_kind_from_path(Path::new("/usr/local/bin/zsh")),
            ShellKind::Zsh,
        );
        assert_eq!(
            shell_kind_from_path(Path::new("/usr/local/bin/fish")),
            ShellKind::Fish,
        );
        assert_eq!(
            shell_kind_from_path(Path::new("/opt/homebrew/bin/nu")),
            ShellKind::Nushell,
        );
    }

    #[test]
    fn real_posix_shells_stay_posix() {
        assert_eq!(shell_kind_from_path(Path::new("/bin/sh")), ShellKind::Posix,);
        assert_eq!(
            shell_kind_from_path(Path::new("/bin/dash")),
            ShellKind::Posix,
        );
    }

    #[test]
    fn unknown_program_falls_back_to_carrot_default() {
        assert_eq!(
            shell_kind_from_path(Path::new("/usr/local/bin/totally-unknown")),
            ShellKind::Nushell,
        );
    }

    #[test]
    fn display_name_uses_product_name_for_nushell() {
        assert_eq!(display_name(ShellKind::Nushell), "nushell");
        assert_eq!(display_name(ShellKind::Bash), "bash");
    }

    #[test]
    fn carrot_default_is_nushell() {
        assert_eq!(carrot_default_shell(), ShellKind::Nushell);
    }
}
