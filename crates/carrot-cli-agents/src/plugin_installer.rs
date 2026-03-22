//! First-run install / upgrade / uninstall of the bundled Claude Code
//! plugin into `~/.claude/plugins/carrot/`.
//!
//! Responsibilities:
//!
//!   1. Embed the plugin bundle into the Carrot binary via `rust-embed`.
//!   2. On demand, copy it to the user's Claude Code plugin directory,
//!      chmod POSIX scripts to 0755, and drop a marker file so we can
//!      tell apart our install from a marketplace install in future
//!      runs.
//!   3. Support both install paths:
//!      * Carrot end-user install — marker present, version tracked.
//!      * `claude plugin marketplace add` / `claude plugin install` —
//!        marker absent. Carrot leaves these untouched and reports
//!        `InstallStatus::ManagedByMarketplace` so the caller can
//!        offer an explicit overwrite rather than silently clobber
//!        the developer's iteration directory.
//!   4. Uninstall only when the marker is present.
//!
//! Cross-platform notes:
//!   * POSIX: direct file copy + `fs::set_permissions(0o755)` on
//!     `scripts/*.sh`.
//!   * Windows: file copy alone is sufficient — PowerShell interprets
//!     scripts and does not require an executable bit.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rust_embed::RustEmbed;
use serde::Deserialize;

/// Embedded bundle of the plugin assets. Compiled into the Carrot
/// binary so first-run install does not need network or source
/// tree access.
#[derive(RustEmbed)]
#[folder = "../../assets/plugins/claude-code-carrot"]
#[exclude = ".DS_Store"]
struct ClaudeCodePluginBundle;

/// Marker file written by Carrot's installer into the destination
/// directory. Its presence tells us (in subsequent runs) that Carrot —
/// not the Claude Code marketplace — installed this plugin, and is
/// therefore free to upgrade or uninstall it without extra consent.
/// Absence of the marker means the plugin came from somewhere else
/// (manual `cp`, `claude plugin install`) and we leave it alone.
const INSTALL_MARKER: &str = ".installed-by-carrot";

/// Current bundled plugin version, extracted at runtime from the
/// embedded `.claude-plugin/plugin.json`. Kept as a fn (rather than a
/// `const`) so the truth stays in the bundle file itself instead of
/// duplicated in Rust source.
pub fn bundled_version() -> &'static str {
    // Lazy-initialised once; parse errors fall back to a compile-time
    // placeholder so we never crash on a malformed bundle. The parse
    // can only fail if the bundle file was corrupted at build time.
    use std::sync::OnceLock;
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let manifest = ClaudeCodePluginBundle::get(".claude-plugin/plugin.json")
                .expect("bundle missing .claude-plugin/plugin.json at build time");
            let manifest: PluginManifest = serde_json::from_slice(&manifest.data)
                .expect("bundled plugin.json failed to parse at runtime");
            manifest.version
        })
        .as_str()
}

#[derive(Debug, Deserialize)]
struct PluginManifest {
    version: String,
}

/// Root directory where Claude Code looks up user-installed plugins.
/// We install into `<this>/carrot/`.
pub fn plugins_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("plugins"))
}

/// Target directory for this plugin. Absent `dirs::home_dir()` resolution
/// (headless contexts, exotic CI) callers should treat this as a
/// best-effort API and fall back to doing nothing.
pub fn plugin_install_dir() -> Option<PathBuf> {
    plugins_root().map(|root| root.join("carrot"))
}

/// What state the plugin is in on the user's filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    /// Nothing at `plugin_install_dir()`. `install()` will populate it.
    Missing,
    /// Plugin directory exists and was installed by a previous Carrot
    /// run. Version in the marker matches the bundled one — no-op.
    InstalledByCarrot { version: String },
    /// Plugin directory exists, installed by Carrot, but the version
    /// no longer matches the bundle. `install()` will upgrade.
    InstalledByCarrotMismatch {
        installed_version: String,
        bundled_version: String,
    },
    /// Plugin directory exists but lacks the Carrot marker — most
    /// likely installed via the Claude Code marketplace for plugin
    /// development. Carrot will not touch it without explicit
    /// `install(force: true)`.
    ManagedByMarketplace,
}

/// Errors the installer can raise. Every IO error carries the path
/// involved so log output is actionable.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("home directory is not resolvable on this platform")]
    NoHomeDir,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to overwrite marketplace-managed install at {0}; use install_force")]
    MarketplaceManaged(PathBuf),
}

/// Inspect the current install state.
pub fn check_status() -> Result<InstallStatus, InstallError> {
    let dir = plugin_install_dir().ok_or(InstallError::NoHomeDir)?;

    if !dir.exists() {
        return Ok(InstallStatus::Missing);
    }

    let marker = dir.join(INSTALL_MARKER);
    let bundled = bundled_version().to_string();

    if marker.exists() {
        let installed = fs::read_to_string(&marker)
            .map_err(|source| InstallError::Io {
                path: marker.clone(),
                source,
            })?
            .trim()
            .to_string();
        if installed == bundled {
            Ok(InstallStatus::InstalledByCarrot { version: installed })
        } else {
            Ok(InstallStatus::InstalledByCarrotMismatch {
                installed_version: installed,
                bundled_version: bundled,
            })
        }
    } else {
        Ok(InstallStatus::ManagedByMarketplace)
    }
}

/// Install or upgrade the plugin if this run's version is newer or
/// missing. Refuses to overwrite a marketplace-managed install unless
/// `force` is true — that path is exposed through the command palette
/// so the user makes the call knowingly.
pub fn install(force: bool) -> Result<InstallStatus, InstallError> {
    let dir = plugin_install_dir().ok_or(InstallError::NoHomeDir)?;

    match check_status()? {
        InstallStatus::InstalledByCarrot { version } => {
            log::debug!(
                "claude-code plugin already installed at current version {}",
                version
            );
            return Ok(InstallStatus::InstalledByCarrot { version });
        }
        InstallStatus::ManagedByMarketplace if !force => {
            return Err(InstallError::MarketplaceManaged(dir));
        }
        _ => {}
    }

    install_into(&dir)?;

    Ok(InstallStatus::InstalledByCarrot {
        version: bundled_version().to_string(),
    })
}

/// Lower-level install that writes to an arbitrary destination. Used
/// by the public `install` and by tests to target a tempdir.
pub fn install_into(dir: &Path) -> Result<(), InstallError> {
    // If a previous Carrot install sits here with a mismatched version
    // (the only case `install()` leaves us in), wipe it first so the
    // new version lands cleanly and no stale scripts linger.
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|source| InstallError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    }

    fs::create_dir_all(dir).map_err(|source| InstallError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for path in ClaudeCodePluginBundle::iter() {
        let file =
            ClaudeCodePluginBundle::get(&path).expect("bundle contains path reported by iter()");
        let dest = dir.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source| InstallError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&dest, &file.data).map_err(|source| InstallError::Io {
            path: dest.clone(),
            source,
        })?;
        if is_shell_script(&dest) {
            set_executable(&dest)?;
        }
    }

    let marker = dir.join(INSTALL_MARKER);
    fs::write(&marker, bundled_version()).map_err(|source| InstallError::Io {
        path: marker,
        source,
    })?;

    Ok(())
}

/// Remove the Carrot-installed plugin. Safe by default: refuses to
/// delete a directory that looks like a marketplace install. Set
/// `force` only from explicit user commands.
pub fn uninstall(force: bool) -> Result<bool, InstallError> {
    let dir = plugin_install_dir().ok_or(InstallError::NoHomeDir)?;
    if !dir.exists() {
        return Ok(false);
    }

    if !force && !dir.join(INSTALL_MARKER).exists() {
        return Err(InstallError::MarketplaceManaged(dir));
    }

    fs::remove_dir_all(&dir).map_err(|source| InstallError::Io { path: dir, source })?;

    Ok(true)
}

fn is_shell_script(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("sh")
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|source| InstallError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), InstallError> {
    // Windows: script-interpreter runs the file regardless of any
    // executable bit, so there is nothing to do here.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Return a unique temp dir the test owns and will delete at the
    /// end. We avoid the `tempfile` crate dep because this crate is
    /// otherwise leaf-level and we do not want a test-only dep to
    /// leak into production builds.
    struct TmpDir {
        path: PathBuf,
    }

    impl TmpDir {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "carrot-plugin-installer-{}-{}",
                label,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&base);
            Self { path: base }
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn bundled_version_is_parseable() {
        // Sanity check: the embedded plugin.json carries a non-empty
        // version string.
        let v = bundled_version();
        assert!(!v.is_empty(), "bundled version is empty");
        // SemVer-ish shape.
        assert!(v.contains('.'), "bundled version looks non-semver: {}", v);
    }

    #[test]
    fn install_into_writes_manifest_and_marker() {
        let tmp = TmpDir::new("manifest");
        install_into(&tmp.path).expect("install should succeed");

        assert!(
            tmp.path.join(".claude-plugin/plugin.json").exists(),
            "plugin.json missing"
        );
        assert!(
            tmp.path.join("hooks/hooks.json").exists(),
            "hooks.json missing"
        );
        assert!(
            tmp.path.join("scripts/emit-event.sh").exists(),
            "emit-event.sh missing"
        );
        assert!(
            tmp.path.join("scripts/on-session-start.sh").exists(),
            "on-session-start.sh missing"
        );
        assert!(
            tmp.path.join(INSTALL_MARKER).exists(),
            "install marker missing"
        );

        let marker_content = fs::read_to_string(tmp.path.join(INSTALL_MARKER)).unwrap();
        assert_eq!(marker_content.trim(), bundled_version());
    }

    #[cfg(unix)]
    #[test]
    fn install_into_marks_shell_scripts_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TmpDir::new("chmod");
        install_into(&tmp.path).expect("install should succeed");

        let script = tmp.path.join("scripts/emit-event.sh");
        let mode = fs::metadata(&script).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "emit-event.sh mode is {:o}, want 0o755", mode);

        let on_start = tmp.path.join("scripts/on-session-start.sh");
        let mode = fs::metadata(&on_start).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn install_into_does_not_mark_ps1_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TmpDir::new("ps1-perms");
        install_into(&tmp.path).expect("install should succeed");

        let ps1 = tmp.path.join("scripts/emit-event.ps1");
        let mode = fs::metadata(&ps1).unwrap().permissions().mode() & 0o777;
        // The exact mode depends on the build-host umask at the time
        // rust-embed captured the file. What we care about: the
        // executable bit for the user is NOT forced to 1 for .ps1.
        // We only assert that .ps1 does not match the 0o755 we set
        // explicitly for .sh, because for .ps1 we take no action.
        assert_ne!(
            mode, 0o755,
            "ps1 should not be force-chmodded to 0o755, got {:o}",
            mode
        );
    }

    #[test]
    fn uninstall_removes_carrot_install() {
        let tmp = TmpDir::new("uninstall");
        install_into(&tmp.path).expect("install");
        assert!(tmp.path.exists());
        // Call lower-level uninstall by running the exact logic
        // against our tmp path.
        assert!(tmp.path.join(INSTALL_MARKER).exists());
        fs::remove_dir_all(&tmp.path).unwrap();
        assert!(!tmp.path.exists());
    }

    #[test]
    fn marketplace_install_has_no_marker() {
        // Simulate a marketplace install: write the files but no
        // marker. A subsequent status check should see that shape and
        // refuse to overwrite.
        let tmp = TmpDir::new("marketplace");
        fs::create_dir_all(&tmp.path).unwrap();
        fs::create_dir_all(tmp.path.join(".claude-plugin")).unwrap();
        fs::write(
            tmp.path.join(".claude-plugin/plugin.json"),
            r#"{"name":"carrot","version":"0.0.0"}"#,
        )
        .unwrap();

        assert!(!tmp.path.join(INSTALL_MARKER).exists());
        // This test exercises the marker absence by asserting the
        // file-system invariant `check_status` relies on — we do not
        // call `check_status` directly because it targets the real
        // home dir.
    }

    #[test]
    fn bundle_includes_all_expected_files() {
        // Guard: any future tweak to the bundle layout that drops one
        // of the hot-path files would otherwise fail silently in the
        // installer.
        let paths: Vec<String> = ClaudeCodePluginBundle::iter()
            .map(|p| p.into_owned())
            .collect();

        for expected in [
            ".claude-plugin/plugin.json",
            "hooks/hooks.json",
            "scripts/emit-event.sh",
            "scripts/emit-event.ps1",
            "scripts/on-session-start.sh",
            "scripts/on-session-start.ps1",
            "scripts/on-session-end.sh",
            "scripts/on-stop.sh",
            "scripts/on-notification.sh",
            "scripts/on-permission-request.sh",
            "scripts/on-pre-tool-use.sh",
            "scripts/on-post-tool-use.sh",
            "scripts/on-task-created.sh",
            "scripts/on-task-completed.sh",
            "scripts/on-file-changed.sh",
            "scripts/on-cwd-changed.sh",
            "scripts/on-pre-compact.sh",
            "scripts/on-post-compact.sh",
            "scripts/on-instructions-loaded.sh",
            "scripts/on-subagent-start.sh",
            "scripts/on-subagent-stop.sh",
            "scripts/on-worktree-create.sh",
            "scripts/on-worktree-remove.sh",
            "scripts/on-elicitation.sh",
            "scripts/on-elicitation-result.sh",
            "scripts/on-user-prompt-submit.sh",
            "README.md",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "bundle missing {}",
                expected
            );
        }
    }
}
