use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

/// The input mode determines whether the shell's prompt is suppressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    /// Carrot Mode: Shell prompt suppressed, replaced by context chips.
    Carrot,
    /// PS1 Mode: Shell prompt visible (Starship, P10k, etc.).
    ShellPs1,
}

/// Spawn a PTY with the user's default shell and Carrot shell integration hooks.
///
/// The hooks inject OSC 133 markers (precmd/preexec) for block boundary detection.
/// In Carrot mode, the shell's prompt is also suppressed.
/// `cwd` sets the initial working directory for the shell.
pub fn spawn_pty(
    rows: u16,
    cols: u16,
    mode: InputMode,
    cwd: &Path,
    shell_override: Option<&str>,
) -> Result<(
    Box<dyn MasterPty + Send>,
    Box<dyn Read + Send>,
    Box<dyn Write + Send>,
    Option<u32>,
)> {
    let pty_system = NativePtySystem::default();

    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = pty_system.openpty(size)?;

    let shell = shell_override
        .map(String::from)
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()));
    let shell_name = shell.rsplit('/').next().unwrap_or("zsh");

    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "carrot");

    // Set input mode env var for shell hooks
    match mode {
        InputMode::Carrot => cmd.env("CARROT_MODE", "carrot"),
        InputMode::ShellPs1 => cmd.env("CARROT_MODE", "ps1"),
    };

    // Inject shell integration hooks via shell-specific mechanisms
    let hooks_dir = find_shell_hooks_dir();
    inject_shell_hooks(&mut cmd, shell_name, &hooks_dir, mode);

    // Keep the child alive by capturing the handle; dropping the
    // handle without kill on Unix would send SIGHUP once the master
    // writer closes, which matches our existing lifecycle (Terminal
    // drop → master drop → SIGHUP to shell).
    let child = pair.slave.spawn_command(cmd)?;
    let shell_pid = child.process_id();
    // Intentionally leak the child handle: `portable_pty` already
    // ties shell lifetime to the master PTY; we only need the handle
    // long enough to read its pid. Dropping it here would not kill
    // the shell.
    std::mem::drop(child);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    Ok((pair.master, reader, writer, shell_pid))
}

/// Resize the PTY to new dimensions.
pub fn resize_pty(master: &dyn MasterPty, rows: u16, cols: u16) -> Result<()> {
    master.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    Ok(())
}

/// Find the directory containing our shell hook scripts.
///
/// Looks for `shell/carrot.zsh` relative to the executable, then falls back
/// to the source directory for development builds.
fn find_shell_hooks_dir() -> PathBuf {
    // Development: relative to crate root
    let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("shell"))
        .unwrap_or_default();

    if dev_dir.join("carrot.zsh").exists() {
        return dev_dir;
    }

    // Installed: next to executable
    if let Ok(exe) = std::env::current_exe() {
        let installed_dir = exe.parent().unwrap_or(&exe).join("shell");
        if installed_dir.join("carrot.zsh").exists() {
            return installed_dir;
        }
    }

    dev_dir
}

/// Inject shell integration hooks into the command environment.
///
/// For zsh: Uses ZDOTDIR to inject a .zshenv that sources our hooks
/// before the user's own shell configuration.
///
/// For bash: Uses --rcfile to source our hooks alongside ~/.bashrc.
///
/// For fish: Uses --init-command to source our hooks.
fn inject_shell_hooks(
    cmd: &mut CommandBuilder,
    shell_name: &str,
    hooks_dir: &PathBuf,
    mode: InputMode,
) {
    match shell_name {
        "zsh" => {
            let hook_script = hooks_dir.join("carrot.zsh");
            if !hook_script.exists() {
                log::warn!("Zsh hook script not found at {:?}", hook_script);
                return;
            }

            // Create temp ZDOTDIR with .zshenv that loads our hooks first
            let carrot_zdotdir = std::env::temp_dir().join("carrot-zsh");
            if std::fs::create_dir_all(&carrot_zdotdir).is_err() {
                log::error!("Failed to create ZDOTDIR at {:?}", carrot_zdotdir);
                return;
            }

            // Preserve original ZDOTDIR (defaults to HOME)
            let original_zdotdir = std::env::var("ZDOTDIR")
                .unwrap_or_else(|_| std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));

            // Write .zshenv that sources our hooks then restores ZDOTDIR
            let zshenv_content = format!(
                r#"# Carrot Shell Integration Loader
# Restore original ZDOTDIR so user's .zshrc/.zprofile etc. load normally
export ZDOTDIR="{original_zdotdir}"

# Source Carrot hooks (OSC 133 markers)
source "{hook_script}"

# Source user's .zshenv if it exists
[[ -f "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
"#,
                original_zdotdir = original_zdotdir,
                hook_script = hook_script.to_string_lossy(),
            );

            let zshenv_path = carrot_zdotdir.join(".zshenv");
            if let Err(e) = std::fs::write(&zshenv_path, zshenv_content) {
                log::error!("Failed to write .zshenv: {}", e);
                return;
            }

            // Redirect zsh to our ZDOTDIR
            cmd.env("_CARROT_ORIG_ZDOTDIR", &original_zdotdir);
            cmd.env("ZDOTDIR", carrot_zdotdir.to_string_lossy().as_ref());
        }

        "bash" => {
            let hook_script = hooks_dir.join("carrot.bash");
            if hook_script.exists() {
                cmd.args(["--rcfile", hook_script.to_string_lossy().as_ref()]);
            }
        }

        "fish" => {
            let hook_script = hooks_dir.join("carrot.fish");
            if hook_script.exists() {
                cmd.args([
                    "--init-command",
                    &format!("source {}", hook_script.to_string_lossy()),
                ]);
            }
        }

        "nu" => {
            // Nushell emits OSC 133 markers natively (reedline) — no marker injection needed.
            // We only inject our OSC 7777 metadata hooks via XDG_DATA_DIRS autoload.
            let nu_hooks_dir = hooks_dir.join("nushell");
            if nu_hooks_dir.join("vendor/autoload/carrot.nu").exists() {
                let existing_xdg = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
                let carrot_xdg = if existing_xdg.is_empty() {
                    nu_hooks_dir.to_string_lossy().to_string()
                } else {
                    format!("{}:{}", nu_hooks_dir.to_string_lossy(), existing_xdg)
                };
                cmd.env("XDG_DATA_DIRS", &carrot_xdg);
            }
            cmd.env("CARROT_SHELL_FEATURES", "metadata,sudo");
        }

        other => {
            log::warn!("No shell integration hooks for shell: {}", other);
        }
    }

    // Common to all shells: export the known-TUI list so preexec hooks can
    // emit OSC 7777 carrot-tui-hint for these commands. Single source of
    // truth lives in `carrot_shell_integration::known_tui`.
    cmd.env(
        "CARROT_KNOWN_TUIS",
        carrot_shell_integration::known_tuis_env_value(),
    );

    let _ = mode; // Mode is set via CARROT_MODE env var above
}
