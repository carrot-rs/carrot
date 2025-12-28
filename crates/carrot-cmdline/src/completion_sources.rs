//! Concrete completion backends.
//!
//! The cmdline's typed AST decides *which* source to consult for
//! the token under the cursor. This module implements the std-only
//! backends — filesystem, env vars, history, git — each a small,
//! self-contained function. Schema-driven sources (715 completion
//! specs) and MCP providers attach behind their respective crate
//! deps.
//!
//! Each backend takes a `prefix` + optional context and returns a
//! [`Vec<CompletionCandidate>`]. Ranking across sources is the
//! caller's job; [`crate::completion::CompletionSet::sort`] does
//! the blend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use carrot_session::command_history::CommandHistory;

use crate::ast::{GitScope, Range};
use crate::completion::{CompletionCandidate, CompletionSource};

/// Walk the filesystem under `cwd` for entries whose name starts
/// with `prefix`. Returns at most `limit` candidates, alphabetical.
///
/// `prefix` may include path separators — e.g. `src/fo` splits into
/// directory `src/` + stem `fo`. Directories produce candidates
/// with a trailing `/`, which keeps typing fluid (user can keep
/// descending).
///
/// `anchor` is the byte range in the original input the candidate
/// replaces when accepted. Callers compute it from the AST
/// positional's range.
pub fn filesystem_candidates(
    cwd: &Path,
    prefix: &str,
    anchor: Range,
    limit: usize,
) -> Vec<CompletionCandidate> {
    let (dir_rel, stem) = match prefix.rsplit_once('/') {
        Some((head, tail)) => (format!("{head}/"), tail.to_string()),
        None => (String::new(), prefix.to_string()),
    };
    let dir = cwd.join(&dir_rel);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&stem) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let label = if is_dir {
            format!("{dir_rel}{name}/")
        } else {
            format!("{dir_rel}{name}")
        };
        out.push(
            CompletionCandidate::replace(CompletionSource::Filesystem, label, anchor)
                .with_icon(if is_dir { "folder" } else { "file" }),
        );
        if out.len() >= limit {
            break;
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Env-var completion: returns variable names (without the leading
/// `$`) whose name starts with `prefix`. When `prefix` begins with
/// a `$`, it's stripped before matching — calling sites vary.
pub fn envvar_candidates(prefix: &str, anchor: Range, limit: usize) -> Vec<CompletionCandidate> {
    let bare = prefix.strip_prefix('$').unwrap_or(prefix);
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|name| name.starts_with(bare))
        .collect();
    names.sort();
    names.truncate(limit);
    names
        .into_iter()
        .map(|name| {
            CompletionCandidate::replace(CompletionSource::EnvVar, format!("${name}"), anchor)
                .with_icon("variable")
        })
        .collect()
}

/// History completion: walks the recent-most history entries first,
/// returning commands that start with `prefix`. `CommandHistory`
/// already deduplicates on insert, so no extra `HashSet` needed here.
pub fn history_candidates(
    history: &CommandHistory,
    prefix: &str,
    anchor: Range,
    limit: usize,
) -> Vec<CompletionCandidate> {
    let mut out = Vec::new();
    for entry in history.entries().iter().rev() {
        if !entry.command.starts_with(prefix) {
            continue;
        }
        out.push(
            CompletionCandidate::replace(CompletionSource::History, entry.command.clone(), anchor)
                .with_icon("history"),
        );
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Convenience wrapper: take a `PathBuf` and eager-drop nonexistent
/// parents instead of erroring. Useful when the cmdline session
/// stored a CWD that has since been rm'd.
pub fn cwd_or_current(cwd: Option<&Path>) -> PathBuf {
    cwd.map(Path::to_path_buf)
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Git-source completion. Shells out to `git for-each-ref` in `cwd`
/// and returns refs matching `prefix`, scoped by `scope`:
///
/// - `Branch` → local branches (`refs/heads/*`)
/// - `Tag`    → annotated + lightweight tags (`refs/tags/*`)
/// - `Remote` → remote-tracking branches (`refs/remotes/*`)
/// - `Commit` → not returned (short SHAs live in reflog; too
///   expensive to enumerate — use `git log` downstream)
/// - `Any`    → union of the three ref categories
///
/// Returns empty when the cwd isn't a git repo, git is missing,
/// or the command fails for any reason. Never panics.
pub fn git_candidates(
    cwd: &Path,
    scope: GitScope,
    prefix: &str,
    anchor: Range,
    limit: usize,
) -> Vec<CompletionCandidate> {
    let patterns: &[&str] = match scope {
        GitScope::Branch => &["refs/heads/"],
        GitScope::Tag => &["refs/tags/"],
        GitScope::Remote => &["refs/remotes/"],
        GitScope::Commit => return Vec::new(),
        GitScope::Any => &["refs/heads/", "refs/tags/", "refs/remotes/"],
    };
    let mut args = vec![
        "for-each-ref".to_string(),
        "--format=%(refname:short)".to_string(),
    ];
    for p in patterns {
        args.push((*p).to_string());
    }
    let output = match Command::new("git").args(&args).current_dir(cwd).output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    let text = match String::from_utf8(output) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let icon = match scope {
        GitScope::Branch => "git-branch",
        GitScope::Tag => "tag",
        GitScope::Remote => "cloud",
        GitScope::Commit | GitScope::Any => "git-ref",
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let name = line.trim();
        if name.is_empty() || !name.starts_with(prefix) {
            continue;
        }
        out.push(
            CompletionCandidate::replace(CompletionSource::Git, name.to_string(), anchor)
                .with_icon(icon),
        );
        if out.len() >= limit {
            break;
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::InsertAction;
    use std::io::Write;

    fn tmpdir() -> tempdir_shim::TmpDir {
        tempdir_shim::create()
    }

    #[test]
    fn filesystem_returns_matching_files() {
        let dir = tmpdir();
        fs::File::create(dir.path().join("alpha.txt")).unwrap();
        fs::File::create(dir.path().join("beta.txt")).unwrap();
        fs::File::create(dir.path().join("other")).unwrap();
        let anchor = Range::new(0, 5);
        let out = filesystem_candidates(dir.path(), "", anchor, 10);
        assert!(out.iter().any(|c| c.label == "alpha.txt"));
        assert!(out.iter().any(|c| c.label == "beta.txt"));
        assert!(out.iter().any(|c| c.label == "other"));
    }

    #[test]
    fn filesystem_filters_by_prefix() {
        let dir = tmpdir();
        fs::File::create(dir.path().join("abcd.txt")).unwrap();
        fs::File::create(dir.path().join("efgh.txt")).unwrap();
        let anchor = Range::new(0, 5);
        let out = filesystem_candidates(dir.path(), "abc", anchor, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, "abcd.txt");
    }

    #[test]
    fn filesystem_respects_limit() {
        let dir = tmpdir();
        for i in 0..20 {
            fs::File::create(dir.path().join(format!("f{i:02}"))).unwrap();
        }
        let anchor = Range::new(0, 5);
        let out = filesystem_candidates(dir.path(), "", anchor, 5);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn filesystem_directories_get_trailing_slash() {
        let dir = tmpdir();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::File::create(dir.path().join("file")).unwrap();
        let out = filesystem_candidates(dir.path(), "", Range::new(0, 0), 10);
        let subdir = out.iter().find(|c| c.label.starts_with("subdir")).unwrap();
        assert!(subdir.label.ends_with('/'));
        let file = out.iter().find(|c| c.label == "file").unwrap();
        assert!(!file.label.ends_with('/'));
    }

    #[test]
    fn filesystem_subpath_traversal() {
        let dir = tmpdir();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::File::create(dir.path().join("nested").join("inside")).unwrap();
        let out = filesystem_candidates(dir.path(), "nested/in", Range::new(0, 0), 10);
        assert!(out.iter().any(|c| c.label == "nested/inside"));
    }

    #[test]
    fn filesystem_missing_dir_returns_empty() {
        let out =
            filesystem_candidates(Path::new("/this/does/not/exist"), "", Range::new(0, 0), 10);
        assert!(out.is_empty());
    }

    #[test]
    fn envvar_returns_current_env() {
        // SAFETY: test-local env var, no other thread can observe.
        unsafe {
            std::env::set_var("CARROT_CMDLINE_TEST_VAR", "1");
        }
        let out = envvar_candidates("CARROT_CMDLINE", Range::new(0, 0), 10);
        assert!(out.iter().any(|c| c.label == "$CARROT_CMDLINE_TEST_VAR"));
        unsafe {
            std::env::remove_var("CARROT_CMDLINE_TEST_VAR");
        }
    }

    #[test]
    fn envvar_strips_leading_dollar() {
        unsafe {
            std::env::set_var("CARROT_CMDLINE_TEST_STRIP", "x");
        }
        let out = envvar_candidates("$CARROT_CMDLINE_TEST_S", Range::new(0, 0), 10);
        assert!(out.iter().any(|c| c.label == "$CARROT_CMDLINE_TEST_STRIP"));
        unsafe {
            std::env::remove_var("CARROT_CMDLINE_TEST_STRIP");
        }
    }

    #[test]
    fn history_matches_prefix_newest_first() {
        let mut h = CommandHistory::new();
        h.push("git pull".to_string());
        h.push("ls -la".to_string());
        h.push("git push".to_string());
        let out = history_candidates(&h, "git", Range::new(0, 0), 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label, "git push");
        assert_eq!(out[1].label, "git pull");
    }

    #[test]
    fn history_deduplicates() {
        let mut h = CommandHistory::new();
        h.push("ls".to_string());
        h.push("ls".to_string());
        let out = history_candidates(&h, "ls", Range::new(0, 0), 10);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn history_respects_limit() {
        let mut h = CommandHistory::new();
        for i in 0..8 {
            h.push(format!("cmd{i}"));
        }
        let out = history_candidates(&h, "cmd", Range::new(0, 0), 3);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn cwd_or_current_falls_back_when_missing() {
        let p = cwd_or_current(Some(Path::new("/this/does/not/exist")));
        assert!(p.exists() || p == Path::new("."));
    }

    #[test]
    fn git_candidates_empty_on_non_repo() {
        let tmp = tmpdir();
        let out = git_candidates(tmp.path(), GitScope::Branch, "", Range::new(0, 0), 10);
        // Non-repo → git for-each-ref exits non-zero → empty.
        assert!(out.is_empty());
    }

    #[test]
    fn git_candidates_commit_scope_returns_empty_by_design() {
        // We don't enumerate commits — too expensive, use `git log`
        // downstream.
        let tmp = tmpdir();
        let out = git_candidates(tmp.path(), GitScope::Commit, "abc", Range::new(0, 0), 10);
        assert!(out.is_empty());
    }

    #[test]
    fn git_candidates_in_real_repo() {
        let tmp = tmpdir();
        // Initialise a bare repo so `git for-each-ref` has something
        // to report. Skip the test if `git` isn't on PATH.
        let init = std::process::Command::new("git")
            .args(["init", "-q", "--initial-branch=main"])
            .current_dir(tmp.path())
            .status();
        let Ok(status) = init else {
            return; // git missing → skip
        };
        if !status.success() {
            return;
        }
        // Write an empty commit so HEAD / refs/heads/main exists.
        let _ = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ])
            .current_dir(tmp.path())
            .status();
        // Tag it so refs/tags has content too.
        let _ = std::process::Command::new("git")
            .args(["tag", "v0.1.0"])
            .current_dir(tmp.path())
            .status();

        let branches = git_candidates(tmp.path(), GitScope::Branch, "", Range::new(0, 0), 10);
        assert!(branches.iter().any(|c| c.label == "main"));

        let tags = git_candidates(tmp.path(), GitScope::Tag, "", Range::new(0, 0), 10);
        assert!(tags.iter().any(|c| c.label == "v0.1.0"));

        let any = git_candidates(tmp.path(), GitScope::Any, "", Range::new(0, 0), 10);
        assert!(any.iter().any(|c| c.label == "main"));
        assert!(any.iter().any(|c| c.label == "v0.1.0"));

        // Prefix filter narrows.
        let filtered = git_candidates(tmp.path(), GitScope::Any, "v", Range::new(0, 0), 10);
        assert!(filtered.iter().all(|c| c.label.starts_with('v')));
    }

    #[test]
    fn candidate_insertion_replaces_anchor() {
        let dir = tmpdir();
        fs::File::create(dir.path().join("x")).unwrap();
        let anchor = Range::new(4, 9);
        let out = filesystem_candidates(dir.path(), "x", anchor, 10);
        assert_eq!(out.len(), 1);
        match &out[0].action {
            InsertAction::Replace { range, replacement } => {
                assert_eq!(*range, anchor);
                assert_eq!(replacement, "x");
            }
            _ => panic!("expected Replace action"),
        }
    }

    // Local implementation to avoid a `tempfile` dep — tests only.
    mod tempdir_shim {
        use std::fs;
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TmpDir {
            path: PathBuf,
        }

        impl TmpDir {
            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TmpDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }

        pub fn create() -> TmpDir {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("carrot-cmdline-test-{pid}-{now}-{n}"));
            fs::create_dir_all(&path).unwrap();
            TmpDir { path }
        }
    }

    // Dummy import so rustc doesn't complain about the `Write`
    // import above; keep the module useful later.
    #[allow(dead_code)]
    fn _touch(p: &Path) {
        let _ = fs::File::create(p).and_then(|mut f| f.write_all(b""));
    }
}
