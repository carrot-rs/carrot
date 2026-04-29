use std::process::Command;

use crate::metadata::ShellMetadataPayload;

/// Git diff statistics (insertions, deletions, changed files).
#[derive(Clone)]
pub struct GitStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Shell context information gathered from the environment.
///
/// Provides CWD, git branch, hostname, username, and other metadata for display
/// in the terminal's context chips area. Updated dynamically via OSC 7777 metadata.
#[derive(Clone)]
pub struct ShellContext {
    pub cwd: String,
    pub cwd_short: String,
    pub hostname: String,
    pub username: String,
    pub git_branch: Option<String>,
    pub git_stats: Option<GitStats>,
    /// Absolute path of the git repository root (working tree root).
    /// Comes from the shell hook's `git rev-parse --show-toplevel`.
    /// Populated opportunistically — `None` when outside a git repo
    /// or before the first prompt fires with metadata.
    pub git_root: Option<String>,
    /// Latest command line surfaced via `ShellMetadataPayload.command`
    /// at preexec time. The OSC 133;C dispatch consumes this with
    /// `take()` into `RouterBlockMetadata.command`, then resets to
    /// `None` so the next prompt cycle starts clean.
    pub command: Option<String>,
}

impl ShellContext {
    /// Gather shell context for the given working directory.
    pub fn gather_for(cwd_path: &std::path::Path) -> Self {
        let cwd = cwd_path.to_string_lossy().to_string();

        let cwd_short = shorten_path(&cwd);
        let git_branch = detect_git_branch(cwd_path);
        let git_stats = if git_branch.is_some() {
            detect_git_stats(cwd_path)
        } else {
            None
        };
        let git_root = detect_git_root(cwd_path);
        let hostname = detect_hostname();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_string());

        Self {
            cwd,
            cwd_short,
            hostname,
            username,
            git_branch,
            git_stats,
            git_root,
            command: None,
        }
    }

    /// Update context dynamically from shell metadata (OSC 7777).
    ///
    /// Field-by-field optional merge: every payload field that is
    /// `Some` overwrites the corresponding context field; `None`
    /// fields are left untouched. This lets two emits per command
    /// cycle layer correctly — precmd fills cwd/git/user/host,
    /// preexec fills command, neither clobbers the other.
    ///
    /// Boolean / structured fields without an Option (`git_stats`)
    /// follow the same rule by treating "unspecified" as preserve.
    pub fn update_from_metadata(&mut self, payload: &ShellMetadataPayload) {
        if let Some(ref cwd) = payload.cwd {
            self.cwd = cwd.clone();
            self.cwd_short = shorten_path(cwd);
        }
        if let Some(ref u) = payload.username {
            self.username = u.clone();
        }
        if let Some(ref h) = payload.hostname {
            self.hostname = h.clone();
        }
        if payload.git_branch.is_some() {
            self.git_branch = payload.git_branch.clone();
        }
        // `git_dirty` is the only signal the shell sends about repo
        // state; absence means "no opinion this emit", so leave the
        // existing `git_stats` untouched. Presence with `false` means
        // "explicitly clean", which clears any prior dirty flag.
        if let Some(dirty) = payload.git_dirty {
            self.git_stats = if dirty {
                Some(GitStats {
                    files_changed: 1,
                    insertions: 0,
                    deletions: 0,
                })
            } else {
                None
            };
        }
        if payload.git_root.is_some() {
            self.git_root = payload.git_root.clone();
        }
        if payload.command.is_some() {
            self.command = payload.command.clone();
        }
    }
}

impl Default for ShellContext {
    fn default() -> Self {
        Self {
            cwd: "~".to_string(),
            cwd_short: "~".to_string(),
            hostname: "localhost".to_string(),
            username: "user".to_string(),
            git_branch: None,
            git_stats: None,
            git_root: None,
            command: None,
        }
    }
}

pub fn shorten_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if path.starts_with(home.as_ref()) {
            return format!("~{}", &path[home.len()..]);
        }
    }
    path.to_string()
}

fn detect_hostname() -> String {
    // Try gethostname first (no subprocess)
    if let Ok(name) = hostname::get() {
        return name.to_string_lossy().to_string();
    }
    "localhost".to_string()
}

fn detect_git_branch(cwd: &std::path::Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn detect_git_root(cwd: &std::path::Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() { None } else { Some(root) }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn ctx_with(cwd: &str, branch: Option<&str>) -> ShellContext {
        ShellContext {
            cwd: cwd.into(),
            cwd_short: cwd.into(),
            hostname: "host".into(),
            username: "user".into(),
            git_branch: branch.map(str::to_string),
            git_stats: None,
            git_root: None,
            command: None,
        }
    }

    #[test]
    fn partial_command_payload_preserves_cwd_and_git() {
        // The preexec emit carries only `command`. Ensure cwd / git
        // / user fields captured at precmd survive intact.
        let mut ctx = ctx_with("/proj", Some("main"));
        let payload = ShellMetadataPayload {
            command: Some("ls -la".into()),
            ..Default::default()
        };
        ctx.update_from_metadata(&payload);
        assert_eq!(ctx.cwd, "/proj");
        assert_eq!(ctx.git_branch.as_deref(), Some("main"));
        assert_eq!(ctx.command.as_deref(), Some("ls -la"));
    }

    #[test]
    fn precmd_then_preexec_layers_correctly() {
        // Real flow: precmd emit fills cwd/git, preexec emit fills
        // command. Both survive, neither clobbers the other.
        let mut ctx = ctx_with("/old", None);
        ctx.update_from_metadata(&ShellMetadataPayload {
            cwd: Some("/new".into()),
            git_branch: Some("main".into()),
            ..Default::default()
        });
        ctx.update_from_metadata(&ShellMetadataPayload {
            command: Some("git status".into()),
            ..Default::default()
        });
        assert_eq!(ctx.cwd, "/new");
        assert_eq!(ctx.git_branch.as_deref(), Some("main"));
        assert_eq!(ctx.command.as_deref(), Some("git status"));
    }

    #[test]
    fn cwd_only_payload_does_not_clear_command() {
        // Edge case: a metadata emit between two CommandStart cycles
        // (e.g. shell internal that re-fires precmd) must not erase
        // a command we already have queued.
        let mut ctx = ctx_with("/x", None);
        ctx.command = Some("echo first".into());
        ctx.update_from_metadata(&ShellMetadataPayload {
            cwd: Some("/y".into()),
            ..Default::default()
        });
        assert_eq!(ctx.cwd, "/y");
        assert_eq!(ctx.command.as_deref(), Some("echo first"));
    }
}

fn detect_git_stats(cwd: &std::path::Path) -> Option<GitStats> {
    let output = Command::new("git")
        .args(["diff", "--stat", "--shortstat", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // Parse: "3 files changed, 454 insertions(+), 110 deletions(-)"
    let mut files_changed = 0u32;
    let mut insertions = 0u32;
    let mut deletions = 0u32;

    for part in text.split(',') {
        let part = part.trim();
        if part.contains("file") {
            if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok()) {
                files_changed = n;
            }
        } else if part.contains("insertion") {
            if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok()) {
                insertions = n;
            }
        } else if part.contains("deletion") {
            if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok()) {
                deletions = n;
            }
        }
    }

    Some(GitStats {
        files_changed,
        insertions,
        deletions,
    })
}
