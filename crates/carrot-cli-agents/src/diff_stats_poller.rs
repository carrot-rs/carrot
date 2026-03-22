//! Periodically poll `git diff --shortstat HEAD` for each tracked
//! terminal and expose the result to the UI layer.
//!
//! Interim source — once the `FileChanged` plugin hook ships, this
//! poller will be replaced by a hook subscriber for agent-accurate
//! file-change detection. Until then, Vertical Tabs and any other
//! consumer that wants `+N -M` numbers per agent session can read
//! them via [`CliAgentSessionManager::diff_stats`].
//!
//! Granularity caveat: the shortstat is session-CWD-wide, so it
//! also counts user edits (not only agent edits). That is a
//! deliberate trade-off for having *some* usable UI signal today.
//!
//! Cadence: one poll every `POLL_INTERVAL` per registered
//! terminal. Polls run on the background executor and cache their
//! result on the manager so UI reads are zero-cost. Terminals
//! without a `.git` directory in or above their CWD produce
//! `None` — the UI hides the slot.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How often to refresh the shortstat per terminal. Kept at 5 s to
/// match the plan's interim-source clause; anything shorter would
/// hammer Git on large worktrees.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Snapshot of `git diff --shortstat HEAD` for a single session CWD.
/// All three fields can be zero (working tree clean, but we still
/// return `Some` so the UI can render "±0" rather than hiding the
/// badge on a clean-but-tracked repo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

impl DiffStats {
    pub fn is_zero(&self) -> bool {
        self.files_changed == 0 && self.insertions == 0 && self.deletions == 0
    }
}

/// Parse the single-line output of `git diff --shortstat HEAD`.
///
/// Expected shapes (Git stable across 2.x/3.x):
///   * `" 2 files changed, 10 insertions(+), 3 deletions(-)"`
///   * `" 1 file changed, 5 insertions(+)"`
///   * `" 1 file changed, 2 deletions(-)"`
///   * empty line → clean working tree → `DiffStats::default()`
///
/// Git sometimes pluralises ("files") and sometimes not ("file"),
/// and omits the insertions or deletions clause when zero — the
/// parser tolerates both. Unknown shapes return `None` so callers
/// can fall back silently rather than lying with zeros.
pub fn parse_shortstat(output: &str) -> Option<DiffStats> {
    let line = output.trim();
    if line.is_empty() {
        return Some(DiffStats::default());
    }

    let mut stats = DiffStats::default();
    let mut any_field_parsed = false;

    for part in line.split(',') {
        let part = part.trim();
        // Snip the leading number off the segment.
        let (num_str, rest) = match part.split_once(' ') {
            Some((n, r)) => (n, r.trim()),
            None => continue,
        };
        let Ok(n) = num_str.parse::<u32>() else {
            continue;
        };

        if rest.starts_with("file") {
            stats.files_changed = n;
            any_field_parsed = true;
        } else if rest.starts_with("insertion") {
            stats.insertions = n;
            any_field_parsed = true;
        } else if rest.starts_with("deletion") {
            stats.deletions = n;
            any_field_parsed = true;
        }
    }

    if any_field_parsed { Some(stats) } else { None }
}

/// Run `git diff --shortstat HEAD` in `cwd` synchronously and
/// return the parsed result. Returns `None` if the command fails
/// (cwd is not a repo, git is not installed, disk I/O error).
///
/// The caller is expected to run this from the background
/// executor (e.g. from inside a `cx.spawn`) — it does blocking
/// `Command::output()` IO.
pub fn run_shortstat_blocking(cwd: &Path) -> Option<DiffStats> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("diff")
        .arg("--shortstat")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    parse_shortstat(stdout)
}

/// Helper: decide whether a directory contains (or is inside) a
/// Git worktree. Cheap — walks upward looking for `.git`.
pub fn has_git_worktree(cwd: &Path) -> bool {
    let mut candidate: Option<&Path> = Some(cwd);
    while let Some(p) = candidate {
        if p.join(".git").exists() {
            return true;
        }
        candidate = p.parent();
    }
    false
}

/// Convenience wrapper for callers that want a single "update my
/// shortstat snapshot" poll. Returns `None` when the directory is
/// not a Git repo (UI should hide the slot).
pub fn poll_once(cwd: &Path) -> Option<DiffStats> {
    if !has_git_worktree(cwd) {
        return None;
    }
    run_shortstat_blocking(cwd)
}

/// Spawn a long-running poll loop for a single CWD and invoke
/// `on_update` whenever the stats change (never on unchanged
/// results — keeps consumer renders cheap).
///
/// Lives as a free fn rather than on the manager so consumers that
/// want their own cadence or scoping (per-tab preview, etc.) can
/// reuse the loop without a full manager. The cli-agents
/// `SessionManager` wraps this for each registered terminal.
pub fn spawn_poll_loop<F>(
    cwd: PathBuf,
    mut on_update: F,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()>
where
    F: FnMut(Option<DiffStats>) + Send + 'static,
{
    use std::sync::atomic::Ordering;
    std::thread::Builder::new()
        .name("carrot-diff-stats-poll".into())
        .spawn(move || {
            let mut last: Option<Option<DiffStats>> = None;
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let now = poll_once(&cwd);
                if last.as_ref() != Some(&now) {
                    on_update(now);
                    last = Some(now);
                }
                // Sleep in short slices so `stop` is honoured
                // quickly after pane/session teardown.
                let slice = Duration::from_millis(250);
                let mut remaining = POLL_INTERVAL;
                while remaining > Duration::ZERO {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let step = remaining.min(slice);
                    std::thread::sleep(step);
                    remaining = remaining.saturating_sub(step);
                }
            }
        })
        .expect("spawn diff-stats poller thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortstat_parses_full_three_field_line() {
        let out = " 2 files changed, 10 insertions(+), 3 deletions(-)\n";
        let stats = parse_shortstat(out).unwrap();
        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.insertions, 10);
        assert_eq!(stats.deletions, 3);
    }

    #[test]
    fn shortstat_parses_singular_file() {
        let out = " 1 file changed, 5 insertions(+)\n";
        let stats = parse_shortstat(out).unwrap();
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.insertions, 5);
        assert_eq!(stats.deletions, 0);
    }

    #[test]
    fn shortstat_parses_deletions_only() {
        let out = " 1 file changed, 2 deletions(-)\n";
        let stats = parse_shortstat(out).unwrap();
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 2);
    }

    #[test]
    fn shortstat_empty_input_means_clean_tree() {
        assert_eq!(parse_shortstat(""), Some(DiffStats::default()));
        assert_eq!(parse_shortstat("\n"), Some(DiffStats::default()));
        assert_eq!(parse_shortstat("   "), Some(DiffStats::default()));
    }

    #[test]
    fn shortstat_unknown_shape_returns_none() {
        assert_eq!(parse_shortstat("totally unexpected"), None);
        assert_eq!(parse_shortstat("nothing"), None);
    }

    #[test]
    fn shortstat_is_zero_helper() {
        assert!(DiffStats::default().is_zero());
        assert!(
            !DiffStats {
                files_changed: 1,
                insertions: 0,
                deletions: 0
            }
            .is_zero()
        );
    }

    #[test]
    fn has_git_worktree_detects_self_and_ancestors() {
        // Our own repo root always has .git.
        let self_cwd = std::env::current_dir().unwrap();
        assert!(has_git_worktree(&self_cwd));
    }

    #[test]
    fn has_git_worktree_returns_false_in_tmp() {
        let tmp = std::env::temp_dir();
        // /tmp is never a git repo on any CI we care about.
        assert!(!has_git_worktree(&tmp));
    }

    #[test]
    fn poll_once_in_non_git_dir_returns_none() {
        let tmp = std::env::temp_dir();
        assert_eq!(poll_once(&tmp), None);
    }

    #[test]
    fn shortstat_handles_leading_whitespace() {
        // Git indents the output with a leading space; we already
        // trim, but double-check extra whitespace variants.
        let out = "    3 files changed, 15 insertions(+), 7 deletions(-)";
        let stats = parse_shortstat(out).unwrap();
        assert_eq!(stats.files_changed, 3);
        assert_eq!(stats.insertions, 15);
        assert_eq!(stats.deletions, 7);
    }
}
