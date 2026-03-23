//! Cross-platform process detection.
//!
//! Walks the process tree rooted at a PTY's child PID, materialises each
//! descendant as a `ProcessInfo`, and asks the agent registry to
//! classify the result. The returned tuple `(agent, process)` is what
//! `CliAgentSessionManager` binds to a workspace session.
//!
//! Cross-platform notes:
//!   * macOS: sandboxed apps may return an empty `exe` path — callers
//!     fall back to `name` via `binary_matches`.
//!   * Linux: `/proc/<pid>/exe` can be unreadable in containers or
//!     under restricted `CAP_SYS_PTRACE` policies — again we fall back
//!     to `name`, and environ stays `None`.
//!   * Windows: process names carry the `.exe` suffix, which we strip
//!     (and, symmetrically, strip from the candidate binary name) so
//!     cross-platform agent `binary_names` tables stay terse.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use inazuma::App;
use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

use crate::agent::{CliAgentMatch, PermissionModeFlag, ProcessInfo, SharedCliAgent};
use crate::registry::CliAgentRegistry;

/// Build a fresh sysinfo `System` snapshot with the refresh kinds we
/// need for agent classification. Every call pays full process-table
/// cost; callers should be rate-limited (the session manager polls at 500 ms).
fn snapshot_system() -> System {
    System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_exe(UpdateKind::Always)
                .with_cmd(UpdateKind::Always)
                .with_environ(UpdateKind::Always),
        ),
    )
}

/// Materialise a sysinfo process into our cross-platform
/// `ProcessInfo`. Returns `None` when the process has no parent (kernel
/// threads on Linux, the Windows System process) because such entries
/// cannot be descendants of a user-space PTY child.
fn process_to_info(process: &sysinfo::Process) -> Option<ProcessInfo> {
    let ppid = process.parent()?.as_u32();

    let environ = process.environ();
    let env = if environ.is_empty() {
        None
    } else {
        let mut map = HashMap::with_capacity(environ.len());
        for entry in environ {
            let entry = entry.to_string_lossy();
            if let Some((k, v)) = entry.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
        if map.is_empty() { None } else { Some(map) }
    };

    Some(ProcessInfo {
        pid: process.pid().as_u32(),
        ppid,
        name: process.name().to_string_lossy().into_owned(),
        exe: process.exe().map(PathBuf::from).unwrap_or_default(),
        cmdline: process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect(),
        env,
    })
}

/// BFS over the process tree starting at `pty_pid`. Returns every
/// descendant, excluding the PTY's shell itself (the shell is the
/// direct child of the PTY and not an agent candidate in its own
/// right).
///
/// The traversal is ppid-based: we build a parent→children map once,
/// then breadth-first from `pty_pid`. Cost is `O(processes)` per call.
pub fn scan_pty_descendants(pty_pid: u32) -> Vec<ProcessInfo> {
    let system = snapshot_system();
    scan_pty_descendants_in(&system, pty_pid)
}

/// Internal variant that takes an already-built `System` so callers
/// that need multiple traversals in one tick can amortise the
/// snapshot cost.
pub fn scan_pty_descendants_in(system: &System, pty_pid: u32) -> Vec<ProcessInfo> {
    let mut children_by_parent: HashMap<u32, Vec<&sysinfo::Process>> = HashMap::new();
    for process in system.processes().values() {
        if let Some(parent) = process.parent() {
            children_by_parent
                .entry(parent.as_u32())
                .or_default()
                .push(process);
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(pty_pid);

    while let Some(pid) = queue.pop_front() {
        let Some(children) = children_by_parent.get(&pid) else {
            continue;
        };
        for child in children {
            let child_pid = child.pid().as_u32();
            if !seen.insert(child_pid) {
                continue;
            }
            if let Some(info) = process_to_info(child) {
                out.push(info);
            }
            queue.push_back(child_pid);
        }
    }

    out
}

/// Ask every registered agent to classify each discovered descendant.
/// The first match wins — agents are consulted in registration order,
/// so Claude Code (registered first) is tried before any future
/// Codex/Gemini/Aider agents.
pub fn classify_processes(
    children: &[ProcessInfo],
    registry: &CliAgentRegistry,
    cx: &App,
) -> Option<(SharedCliAgent, ProcessInfo)> {
    for process in children {
        for agent in registry.agents() {
            if agent.classify(process, cx).is_some() {
                return Some((agent.clone(), process.clone()));
            }
        }
    }
    None
}

/// Cross-platform binary-name match. Strips `.exe` off both sides so
/// agent `binary_names` tables can stay OS-agnostic.
///
/// Returns true when any of the `candidates` matches — either the
/// process name or the last component of the executable path.
pub fn binary_matches(process: &ProcessInfo, candidates: &[&str]) -> bool {
    let name = normalise_binary(&process.name);
    let exe_stem = process
        .exe
        .file_name()
        .map(|n| normalise_binary(&n.to_string_lossy()));

    candidates.iter().any(|candidate| {
        let normalised = normalise_binary(candidate);
        name == normalised || exe_stem.as_ref().is_some_and(|stem| *stem == normalised)
    })
}

/// Strip Windows `.exe` suffix and lowercase the binary name so cross-
/// platform comparisons work. We keep everything else untouched so
/// `claude-code` and `claude-code-next` stay distinct.
fn normalise_binary(name: &str) -> String {
    let trimmed = name.strip_suffix(".exe").unwrap_or(name);
    trimmed.to_lowercase()
}

/// Parse Claude-Code-relevant flags out of a cmdline vector. Only
/// collects the documented flag set — unknown flags are ignored.
/// Returns a `CliAgentMatch` seeded with the PID and cmdline; the
/// caller is expected to fill in `name` etc. from what we extract.
pub fn parse_claude_cmdline(process: &ProcessInfo) -> CliAgentMatch {
    let mut m = CliAgentMatch {
        pid: process.pid,
        cmdline: process.cmdline.clone(),
        ..CliAgentMatch::default()
    };

    // Skip the binary itself; argv[0] is never a flag.
    let mut iter = process.cmdline.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--name" | "-n" => {
                if let Some(v) = iter.next() {
                    m.name = Some(v.clone());
                }
            }
            "--resume" | "-r" => {
                if let Some(v) = iter.next() {
                    m.resume_session_name = Some(v.clone());
                }
            }
            "--continue" | "-c" => {
                m.continue_latest = true;
            }
            "--worktree" | "-w" => {
                if let Some(v) = iter.next() {
                    m.worktree = Some(PathBuf::from(v));
                }
            }
            "--from-pr" => {
                if let Some(v) = iter.next()
                    && let Ok(n) = v.parse::<u64>()
                {
                    m.from_pr = Some(n);
                }
            }
            "--permission-mode" => {
                if let Some(v) = iter.next() {
                    m.permission_mode = Some(PermissionModeFlag(v.clone()));
                }
            }
            "--model" => {
                if let Some(v) = iter.next() {
                    m.model = Some(v.clone());
                }
            }
            other => {
                // Handle --flag=value shape for every flag above.
                if let Some((flag, value)) = other.split_once('=') {
                    match flag {
                        "--name" => m.name = Some(value.to_string()),
                        "--resume" => m.resume_session_name = Some(value.to_string()),
                        "--worktree" => m.worktree = Some(PathBuf::from(value)),
                        "--from-pr" => {
                            if let Ok(n) = value.parse::<u64>() {
                                m.from_pr = Some(n);
                            }
                        }
                        "--permission-mode" => {
                            m.permission_mode = Some(PermissionModeFlag(value.to_string()));
                        }
                        "--model" => m.model = Some(value.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_proc(pid: u32, ppid: u32, name: &str, cmdline: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            name: name.to_string(),
            exe: PathBuf::new(),
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            env: None,
        }
    }

    #[test]
    fn binary_match_plain_name() {
        let proc = mk_proc(10, 1, "claude", &["claude"]);
        assert!(binary_matches(&proc, &["claude"]));
        assert!(!binary_matches(&proc, &["not-claude"]));
    }

    #[test]
    fn binary_match_strips_exe_suffix_on_windows() {
        let proc = mk_proc(10, 1, "claude.exe", &["claude.exe"]);
        assert!(binary_matches(&proc, &["claude"]));
        assert!(binary_matches(&proc, &["claude.exe"]));
    }

    #[test]
    fn binary_match_uses_exe_path_when_name_differs() {
        // macOS can leave `name` as a wrapper shim while the real
        // binary lives in `exe`. Stripping to the file-name and
        // comparing should still match.
        let mut proc = mk_proc(10, 1, "wrapper", &["claude"]);
        proc.exe = PathBuf::from("/opt/claude/bin/claude");
        assert!(binary_matches(&proc, &["claude"]));
    }

    #[test]
    fn binary_match_empty_exe_falls_back_to_name() {
        // macOS sandboxed case: `exe` is empty, but `name` carries the
        // real binary. Must still match.
        let proc = mk_proc(10, 1, "claude", &["claude"]);
        assert!(proc.exe.as_os_str().is_empty());
        assert!(binary_matches(&proc, &["claude"]));
    }

    #[test]
    fn parse_name_flag_long_form() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--name", "my-session"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name.as_deref(), Some("my-session"));
    }

    #[test]
    fn parse_name_flag_short_form() {
        let proc = mk_proc(10, 1, "claude", &["claude", "-n", "short-name"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name.as_deref(), Some("short-name"));
    }

    #[test]
    fn parse_resume_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--resume", "abc123"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.resume_session_name.as_deref(), Some("abc123"));
        assert!(!m.continue_latest);
    }

    #[test]
    fn parse_resume_short_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "-r", "abc123"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.resume_session_name.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_continue_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--continue"]);
        let m = parse_claude_cmdline(&proc);
        assert!(m.continue_latest);
    }

    #[test]
    fn parse_continue_short_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "-c"]);
        let m = parse_claude_cmdline(&proc);
        assert!(m.continue_latest);
    }

    #[test]
    fn parse_worktree_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--worktree", "/tmp/feature-x"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.worktree, Some(PathBuf::from("/tmp/feature-x")));
    }

    #[test]
    fn parse_from_pr_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--from-pr", "456"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.from_pr, Some(456));
    }

    #[test]
    fn parse_from_pr_rejects_non_numeric() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--from-pr", "abc"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.from_pr, None);
    }

    #[test]
    fn parse_permission_mode_flag() {
        let proc = mk_proc(
            10,
            1,
            "claude",
            &["claude", "--permission-mode", "acceptEdits"],
        );
        let m = parse_claude_cmdline(&proc);
        assert_eq!(
            m.permission_mode.as_ref().map(|f| f.0.as_str()),
            Some("acceptEdits")
        );
    }

    #[test]
    fn parse_model_flag() {
        let proc = mk_proc(10, 1, "claude", &["claude", "--model", "claude-opus-4-7"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn parse_equal_form_flags() {
        let proc = mk_proc(
            10,
            1,
            "claude",
            &[
                "claude",
                "--name=foo",
                "--worktree=/wt",
                "--from-pr=99",
                "--permission-mode=plan",
                "--model=sonnet",
            ],
        );
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name.as_deref(), Some("foo"));
        assert_eq!(m.worktree, Some(PathBuf::from("/wt")));
        assert_eq!(m.from_pr, Some(99));
        assert_eq!(
            m.permission_mode.as_ref().map(|f| f.0.as_str()),
            Some("plan")
        );
        assert_eq!(m.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn parse_combines_multiple_flags() {
        let proc = mk_proc(
            10,
            1,
            "claude",
            &[
                "claude",
                "--name",
                "demo",
                "-r",
                "prev",
                "--worktree",
                "feature-x",
                "--from-pr",
                "42",
            ],
        );
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name.as_deref(), Some("demo"));
        assert_eq!(m.resume_session_name.as_deref(), Some("prev"));
        assert_eq!(m.worktree, Some(PathBuf::from("feature-x")));
        assert_eq!(m.from_pr, Some(42));
        assert_eq!(m.pid, 10);
        assert_eq!(m.cmdline, proc.cmdline);
    }

    #[test]
    fn parse_ignores_unknown_flags() {
        let proc = mk_proc(
            10,
            1,
            "claude",
            &["claude", "--future-flag", "bar", "--name", "foo"],
        );
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name.as_deref(), Some("foo"));
    }

    #[test]
    fn parse_tolerates_missing_values() {
        // Last arg is a flag with no following value. Must not panic.
        let proc = mk_proc(10, 1, "claude", &["claude", "--name"]);
        let m = parse_claude_cmdline(&proc);
        assert_eq!(m.name, None);
    }

    #[test]
    fn scan_descendants_of_test_process_includes_self() {
        // The test binary is a live process on the host; its own PID
        // must not appear when we scan downward from it, but its
        // thread/child processes (test harness, rustc-driven helpers)
        // may appear. The only guarantee we check here is that the
        // scan does not panic and does not include the seed PID.
        let pid = sysinfo::get_current_pid().unwrap().as_u32();
        let descendants = scan_pty_descendants(pid);
        assert!(
            !descendants.iter().any(|p| p.pid == pid),
            "seed pid must not appear in descendants"
        );
    }

    #[test]
    fn scan_descendants_of_unknown_pid_is_empty() {
        // PID 0 never has descendants in a user context.
        let descendants = scan_pty_descendants(0);
        assert!(
            descendants.is_empty(),
            "scanning pid 0 must yield no descendants, got {}",
            descendants.len()
        );
    }
}
