// GitHub CLI (`gh`) detection, install-command resolution, and PR fetching.
//
// Pure data + logic — no UI rendering. The install modal UI lives in the
// consumer crate (carrot-vertical-tabs). Mirrors the `shell_install`
// pattern so users who have already seen the shell-install modal
// recognise the flow.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::shell_install::{Platform, PlatformInstall, current_platform};

/// Platform-specific ways to install `gh`. Ordered by user familiarity:
/// the native package manager first, then community managers. The caller
/// picks the first entry whose `check` command succeeds on the host.
pub const GH_CLI_INSTALL: &[PlatformInstall] = &[
    PlatformInstall {
        platform: Platform::MacOS,
        package_manager: "brew",
        command: "brew install gh",
        check: "brew --version",
    },
    PlatformInstall {
        platform: Platform::Linux,
        package_manager: "apt",
        // gh's own distro package for Debian/Ubuntu (mirrors the
        // instructions on cli.github.com). The `sudo` is intentional —
        // users running in a plain terminal will be prompted for their
        // password. Matches the shell_install flow for nushell/fish.
        command: "sudo apt install gh",
        check: "apt --version",
    },
    PlatformInstall {
        platform: Platform::Linux,
        package_manager: "dnf",
        command: "sudo dnf install gh",
        check: "dnf --version",
    },
    PlatformInstall {
        platform: Platform::Linux,
        package_manager: "pacman",
        command: "sudo pacman -S github-cli",
        check: "pacman --version",
    },
    PlatformInstall {
        platform: Platform::Windows,
        package_manager: "winget",
        command: "winget install GitHub.cli",
        check: "winget --version",
    },
    PlatformInstall {
        platform: Platform::Windows,
        package_manager: "scoop",
        command: "scoop install gh",
        check: "scoop --version",
    },
];

/// Official install / documentation URL for gh, used as the
/// last-resort fallback when no package manager is detected.
pub const GH_CLI_URL: &str = "https://cli.github.com/";

/// Return true if `gh` is on PATH. Synchronous — the caller schedules it
/// on a background executor when called from the UI render path.
pub fn check_gh_installed() -> bool {
    Command::new("sh")
        .args(["-c", "command -v gh"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return true if `gh auth status` exits cleanly (i.e. the user has
/// authenticated at least one host). False on any failure — unauthenticated,
/// network error, gh not installed. Callers that care about the difference
/// must call `check_gh_installed` first.
pub fn check_gh_authenticated() -> bool {
    Command::new("gh")
        .arg("auth")
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pick the first package manager whose `check` command succeeds on the
/// current platform. Returns None on unsupported platforms, or when the
/// user has none of the listed managers.
pub fn detect_gh_installer() -> Option<&'static PlatformInstall> {
    let current = current_platform();
    GH_CLI_INSTALL
        .iter()
        .filter(|i| i.platform == current)
        .find(|i| {
            Command::new("sh")
                .args(["-c", i.check])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
}

/// Pull-request status as reported by `gh pr list --json state`. The
/// GitHub API returns the raw upper-case strings — we parse them into
/// a small closed enum so consumers can switch without matching strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
    Draft,
}

/// Minimal info about one pull request on a branch. Only the fields the
/// vertical-tabs badge needs — number for the label, state for the icon,
/// url for the future "open in browser" action, title for the tooltip.
#[derive(Debug, Clone)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub url: String,
    pub title: String,
}

/// Fetch at most one PR associated with `branch` in the repository at
/// `cwd`. Returns `Ok(None)` when no PR exists (empty result), `Err` when
/// gh itself errored (not installed, not authenticated, network issue,
/// cwd not a repo). Synchronous shell-out — caller schedules in background.
pub fn fetch_pr_for_branch(branch: &str, cwd: &Path) -> Result<Option<PrInfo>, String> {
    let output = Command::new("gh")
        .arg("pr")
        .arg("list")
        .arg("--head")
        .arg(branch)
        .arg("--limit")
        .arg("1")
        .arg("--json")
        .arg("number,state,url,title,isDraft")
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to spawn gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr list failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_pr_list_json(stdout.as_ref())
}

/// Parse the JSON array gh emits for `pr list --json`. Kept private and
/// hand-rolled with serde_json so the upstream gh CLI's field names stay
/// the single source of truth (no local schema to drift).
fn parse_pr_list_json(raw: &str) -> Result<Option<PrInfo>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Ok(None);
    }

    #[derive(serde::Deserialize)]
    struct RawPr {
        number: u64,
        state: String,
        url: String,
        title: String,
        #[serde(default)]
        #[serde(rename = "isDraft")]
        is_draft: bool,
    }

    let prs: Vec<RawPr> = serde_json::from_str(trimmed)
        .map_err(|e| format!("gh pr list returned invalid json: {e}"))?;

    let Some(raw) = prs.into_iter().next() else {
        return Ok(None);
    };

    // gh returns MERGED / CLOSED / OPEN. A draft is OPEN + isDraft=true,
    // so we collapse that into our Draft variant before the caller sees
    // it — saves every consumer from duplicating the draft-branch check.
    let state = match raw.state.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ if raw.is_draft => PrState::Draft,
        _ => PrState::Open,
    };

    Ok(Some(PrInfo {
        number: raw.number,
        state,
        url: raw.url,
        title: raw.title,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_pr() {
        let raw = r#"[{"number":42,"state":"OPEN","url":"https://github.com/x/y/pull/42","title":"Fix x","isDraft":false}]"#;
        let pr = parse_pr_list_json(raw).unwrap().unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.title, "Fix x");
    }

    #[test]
    fn parses_draft() {
        let raw = r#"[{"number":7,"state":"OPEN","url":"u","title":"t","isDraft":true}]"#;
        assert_eq!(
            parse_pr_list_json(raw).unwrap().unwrap().state,
            PrState::Draft
        );
    }

    #[test]
    fn parses_merged() {
        let raw = r#"[{"number":7,"state":"MERGED","url":"u","title":"t","isDraft":false}]"#;
        assert_eq!(
            parse_pr_list_json(raw).unwrap().unwrap().state,
            PrState::Merged
        );
    }

    #[test]
    fn empty_array_is_none() {
        assert!(parse_pr_list_json("[]").unwrap().is_none());
        assert!(parse_pr_list_json("").unwrap().is_none());
    }
}
