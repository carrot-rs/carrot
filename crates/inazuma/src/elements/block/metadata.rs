use super::BlockKind;
use std::time::Instant;

/// Metadata attached to a single block.
#[derive(Clone, Debug, Default)]
pub struct BlockMetadata {
    /// What kind of block this is.
    pub kind: BlockKind,
    /// Exit code of the command, if finished.
    pub exit_code: Option<i32>,
    /// Measured duration in milliseconds, if finished.
    pub duration_ms: Option<u64>,
    /// Working directory at command start.
    pub cwd: Option<String>,
    /// Git branch at command start, if any.
    pub git_branch: Option<String>,
    /// Instant the block started running.
    pub started_at: Option<Instant>,
    /// Instant the block finished.
    pub finished_at: Option<Instant>,
    /// Semantic tags assigned after `CommandEnd` (Wager B3).
    /// Produced by a small local model or user rules; consumed by
    /// filters and natural-language search in the UI.
    pub tags: Vec<SemanticTag>,
}

/// Predefined categories for semantic block tagging. `Custom` holds a
/// user-supplied label so projects can define their own buckets.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SemanticTag {
    /// Command ended with a non-zero exit code or emitted an error
    /// diagnostic.
    Error,
    /// Command surfaced a warning without failing.
    Warning,
    /// Deployment-related command (`terraform apply`, `kubectl apply`,
    /// cloud-provider CLIs, etc.).
    Deploy,
    /// Test-runner invocation (`cargo test`, `pytest`, `jest`, …).
    TestRun,
    /// Build-pipeline invocation (`cargo build`, `make`, `bazel build`, …).
    Build,
    /// Any git operation (`git …`).
    GitOperation,
    /// Package or dependency install (`cargo install`, `npm install`,
    /// `pip install`, `apt install`, …).
    Install,
    /// Search or query command (`grep`, `rg`, `find`, `fd`).
    Search,
    /// Caller-supplied label — lets projects introduce their own
    /// buckets without modifying this enum.
    Custom(String),
}

impl SemanticTag {
    /// Human-readable label suitable for chip rendering.
    pub fn label(&self) -> &str {
        match self {
            SemanticTag::Error => "error",
            SemanticTag::Warning => "warning",
            SemanticTag::Deploy => "deploy",
            SemanticTag::TestRun => "test",
            SemanticTag::Build => "build",
            SemanticTag::GitOperation => "git",
            SemanticTag::Install => "install",
            SemanticTag::Search => "search",
            SemanticTag::Custom(s) => s.as_str(),
        }
    }

    /// Pre-built tag sets derived from exit code + command line. The
    /// LLM-based tagger (future) would replace this heuristic but the
    /// shape stays stable.
    pub fn heuristic(exit_code: Option<i32>, command: &str) -> Vec<SemanticTag> {
        let mut tags = Vec::new();
        let lower = command.trim_start().to_ascii_lowercase();
        if let Some(code) = exit_code
            && code != 0
        {
            tags.push(SemanticTag::Error);
        }
        let first = lower.split_ascii_whitespace().next().unwrap_or("");
        match first {
            "git" => tags.push(SemanticTag::GitOperation),
            "cargo" | "make" | "ninja" | "bazel" | "go" => tags.push(SemanticTag::Build),
            "pytest" | "jest" | "vitest" | "mocha" => tags.push(SemanticTag::TestRun),
            "npm" | "yarn" | "pnpm" | "bun" | "pip" | "brew" | "apt" | "dnf" | "pacman" => {
                tags.push(SemanticTag::Install);
            }
            "docker" | "kubectl" | "helm" | "terraform" | "ansible" | "fly" | "flyctl" => {
                tags.push(SemanticTag::Deploy);
            }
            "rg" | "grep" | "ag" | "ack" | "find" | "fd" => tags.push(SemanticTag::Search),
            _ => {}
        }
        // Secondary patterns.
        if lower.contains("test") && !tags.iter().any(|t| matches!(t, SemanticTag::TestRun)) {
            tags.push(SemanticTag::TestRun);
        }
        tags
    }
}

impl BlockMetadata {
    /// Convenience: attach the heuristic tag set and return self.
    pub fn with_heuristic_tags(mut self, command: &str) -> Self {
        self.tags
            .extend(SemanticTag::heuristic(self.exit_code, command));
        self
    }

    /// Check whether any of the stored tags matches `tag`.
    pub fn has_tag(&self, tag: &SemanticTag) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_every_variant() {
        assert_eq!(SemanticTag::Error.label(), "error");
        assert_eq!(SemanticTag::TestRun.label(), "test");
        assert_eq!(SemanticTag::Custom("hotfix".into()).label(), "hotfix");
    }

    #[test]
    fn heuristic_flags_non_zero_exit_as_error() {
        let tags = SemanticTag::heuristic(Some(1), "cargo test");
        assert!(tags.contains(&SemanticTag::Error));
        assert!(tags.contains(&SemanticTag::Build));
    }

    #[test]
    fn heuristic_detects_git_operations() {
        let tags = SemanticTag::heuristic(Some(0), "git push origin main");
        assert!(tags.contains(&SemanticTag::GitOperation));
        assert!(!tags.contains(&SemanticTag::Error));
    }

    #[test]
    fn heuristic_detects_install_commands() {
        for cmd in ["npm install", "brew install rg", "pip install foo"] {
            let tags = SemanticTag::heuristic(Some(0), cmd);
            assert!(tags.contains(&SemanticTag::Install), "cmd = {cmd}");
        }
    }

    #[test]
    fn heuristic_detects_deploy_stack() {
        for cmd in ["docker build .", "kubectl apply -f x", "terraform plan"] {
            let tags = SemanticTag::heuristic(Some(0), cmd);
            assert!(tags.contains(&SemanticTag::Deploy), "cmd = {cmd}");
        }
    }

    #[test]
    fn heuristic_detects_search_tools() {
        for cmd in ["rg foo", "grep -r bar ."] {
            let tags = SemanticTag::heuristic(Some(0), cmd);
            assert!(tags.contains(&SemanticTag::Search), "cmd = {cmd}");
        }
    }

    #[test]
    fn heuristic_secondary_test_pattern() {
        // "cargo test" already flagged as Build; "test" in args adds
        // TestRun once.
        let tags = SemanticTag::heuristic(Some(0), "make test");
        assert!(tags.contains(&SemanticTag::Build));
        assert!(tags.contains(&SemanticTag::TestRun));
    }

    #[test]
    fn with_heuristic_tags_populates_metadata() {
        let md = BlockMetadata {
            exit_code: Some(0),
            ..Default::default()
        }
        .with_heuristic_tags("cargo build");
        assert!(md.has_tag(&SemanticTag::Build));
        assert!(!md.has_tag(&SemanticTag::Error));
    }

    #[test]
    fn empty_command_produces_no_tags() {
        let tags = SemanticTag::heuristic(Some(0), "");
        assert!(tags.is_empty());
    }

    #[test]
    fn custom_tag_equals_itself() {
        let a = SemanticTag::Custom("hotfix".into());
        let b = SemanticTag::Custom("hotfix".into());
        assert_eq!(a, b);
    }

    #[test]
    fn has_tag_distinguishes_custom_values() {
        let md = BlockMetadata {
            tags: vec![SemanticTag::Custom("hotfix".into())],
            ..Default::default()
        };
        assert!(md.has_tag(&SemanticTag::Custom("hotfix".into())));
        assert!(!md.has_tag(&SemanticTag::Custom("feature".into())));
    }
}
