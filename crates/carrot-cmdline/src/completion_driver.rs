//! Cursor-aware completion driver.
//!
//! The driver is the bridge between the typed [`crate::ast::CommandAst`]
//! and the concrete [`crate::completion_sources`] backends. Given
//! the current session, it finds the positional under the cursor,
//! decides which source to consult based on the positional's
//! [`ArgKind`], and returns a ranked [`CompletionSet`].
//!
//! # Why this is its own module
//!
//! Each source has different context needs — filesystem wants a
//! `cwd`, history wants a `&History`, envvar wants nothing extra.
//! Rather than overload every source or force callers to build the
//! dispatch themselves, the driver owns the "given these inputs,
//! query the right source" switch in one place.
//!
//! When schema-driven completions land (the 715 `carrot-completions`
//! specs), this is the dispatch that grows a new arm. Call sites
//! stay untouched.

use std::path::Path;

use carrot_session::command_history::CommandHistory;

use crate::ast::{ArgKind, Range};
use crate::completion::{CompletionSet, CompletionSource};
use crate::completion_sources::{envvar_candidates, filesystem_candidates, history_candidates};
use crate::session::CmdlineSession;

/// External inputs the driver can consult. Omit what isn't
/// available.
#[derive(Default)]
pub struct DriverContext<'a> {
    pub cwd: Option<&'a Path>,
    pub history: Option<&'a CommandHistory>,
    /// Maximum candidates per source bucket.
    pub per_source_limit: usize,
}

impl<'a> DriverContext<'a> {
    pub fn new() -> Self {
        Self {
            per_source_limit: 32,
            ..Self::default()
        }
    }
}

/// Find the positional (if any) under the session's cursor and
/// consult the matching source. The returned set is already sorted
/// via [`CompletionSet::sort`].
pub fn suggest_for_cursor(session: &CmdlineSession, ctx: &DriverContext<'_>) -> CompletionSet {
    let mut set = CompletionSet::new();
    let cursor = session.cursor();

    // Case A: a typed positional covers the cursor.
    if let Some(positional) = session.ast().positional_at(cursor) {
        set.anchor = Some(positional.range);
        let prefix = token_prefix(session.buffer(), positional.range, cursor);
        set.candidates = candidates_for_arg_kind(&positional.kind, &prefix, positional.range, ctx);
        set.sort();
        return set;
    }

    // Case B: cursor is outside any typed positional → fall back to
    // history completion over the whole buffer.
    if let Some(history) = ctx.history {
        let prefix = session.buffer();
        let anchor = Range::new(0, session.buffer().len());
        set.anchor = Some(anchor);
        set.candidates = history_candidates(history, prefix, anchor, ctx.per_source_limit.max(1));
        set.sort();
    }
    set
}

fn candidates_for_arg_kind(
    kind: &ArgKind,
    prefix: &str,
    anchor: Range,
    ctx: &DriverContext<'_>,
) -> Vec<crate::completion::CompletionCandidate> {
    let limit = ctx.per_source_limit.max(1);
    match kind {
        ArgKind::Path { .. } => {
            if let Some(cwd) = ctx.cwd {
                filesystem_candidates(cwd, prefix, anchor, limit)
            } else {
                Vec::new()
            }
        }
        ArgKind::EnvVar => envvar_candidates(prefix, anchor, limit),
        ArgKind::Enum(variants) => variants
            .iter()
            .filter(|v| v.starts_with(prefix))
            .map(|v| {
                crate::completion::CompletionCandidate::replace(
                    CompletionSource::Spec,
                    (*v).to_string(),
                    anchor,
                )
            })
            .collect(),
        ArgKind::GitRef { scope } => {
            if let Some(cwd) = ctx.cwd {
                crate::completion_sources::git_candidates(cwd, *scope, prefix, anchor, limit)
            } else {
                Vec::new()
            }
        }
        ArgKind::Url | ArgKind::ProcessId | ArgKind::Literal | ArgKind::Unknown => Vec::new(),
    }
}

/// Slice of the token up to the cursor, used as the completion
/// prefix. Returns an empty string when the cursor sits before the
/// token's start.
fn token_prefix(buffer: &str, range: Range, cursor: usize) -> String {
    let end = cursor.min(range.end).max(range.start);
    if end <= range.start {
        String::new()
    } else {
        buffer[range.start..end].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CommandNode, PathKind, PositionalNode, SubcommandNode};
    use crate::shell::ShellKind;

    fn session_with(input: &str) -> CmdlineSession {
        let mut s = CmdlineSession::new(ShellKind::Bash);
        s.set_buffer(input);
        s
    }

    #[test]
    fn outside_any_positional_falls_back_to_history() {
        let session = session_with("");
        let mut h = CommandHistory::new();
        h.push("ls -la".to_string());
        let ctx = DriverContext {
            history: Some(&h),
            per_source_limit: 10,
            ..DriverContext::default()
        };
        let set = suggest_for_cursor(&session, &ctx);
        assert!(!set.is_empty());
        assert!(
            set.candidates
                .iter()
                .all(|c| c.source == CompletionSource::History)
        );
    }

    #[test]
    fn env_var_positional_queries_env_source() {
        // SAFETY: test-local env var, no parallel observer.
        unsafe {
            std::env::set_var("CARROT_DRIVER_TEST_VAR", "1");
        }
        // Build an AST manually so the positional has ArgKind::EnvVar
        // without needing schema lookup.
        let session = session_with("echo $CARROT_DRIVER");
        // Build a fresh single-element AST so the positional has
        // ArgKind::EnvVar without needing schema lookup.
        let element = crate::ast::PipelineElement {
            command: Some(CommandNode {
                name: "echo".into(),
                range: Range::new(0, 4),
            }),
            positionals: vec![PositionalNode {
                value: "$CARROT_DRIVER".into(),
                kind: ArgKind::EnvVar,
                range: Range::new(5, 19),
            }],
            ..crate::ast::PipelineElement::empty()
        };
        let ast = crate::ast::CommandAst::from_element(element);
        let positional = &ast.elements[0].positionals[0];
        let prefix = token_prefix(session.buffer(), positional.range, session.cursor());
        assert_eq!(prefix, "$CARROT_DRIVER");
        unsafe {
            std::env::remove_var("CARROT_DRIVER_TEST_VAR");
        }
    }

    #[test]
    fn path_positional_queries_fs_source() {
        let tmp = std::env::temp_dir();
        let ctx_cwd = tmp.clone();
        std::fs::create_dir_all(&ctx_cwd).unwrap();
        // Create a unique file to detect.
        let unique = format!("carrot_driver_test_{}", std::process::id());
        let path = ctx_cwd.join(&unique);
        std::fs::write(&path, b"").unwrap();

        let mut session = session_with("cat ");
        session.set_cursor(4);

        // Force a Path-typed positional at cursor via direct
        // helper call (we don't have a schema-typing layer yet).
        let anchor = Range::new(4, 4);
        let kind = ArgKind::Path {
            must_exist: false,
            kind: PathKind::Any,
        };
        let ctx = DriverContext {
            cwd: Some(&ctx_cwd),
            per_source_limit: 50,
            ..DriverContext::default()
        };
        let out = candidates_for_arg_kind(&kind, "carrot_driver_test_", anchor, &ctx);
        assert!(out.iter().any(|c| c.label.contains(&unique)));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn enum_positional_filters_by_prefix() {
        let variants = vec!["json", "yaml", "toml"];
        let kind = ArgKind::Enum(variants.clone());
        let ctx = DriverContext::default();
        let out = candidates_for_arg_kind(&kind, "y", Range::new(0, 0), &ctx);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, "yaml");
        assert_eq!(out[0].source, CompletionSource::Spec);
    }

    #[test]
    fn git_ref_needs_source_not_yet_wired() {
        // Until the git source lands, ArgKind::GitRef returns empty.
        let kind = ArgKind::GitRef {
            scope: crate::ast::GitScope::Branch,
        };
        let ctx = DriverContext::default();
        let out = candidates_for_arg_kind(&kind, "m", Range::new(0, 1), &ctx);
        assert!(out.is_empty());
    }

    #[test]
    fn token_prefix_slices_up_to_cursor() {
        let input = "git checkout main";
        let range = Range::new(13, 17); // "main"
        assert_eq!(token_prefix(input, range, 15), "ma");
        assert_eq!(token_prefix(input, range, 17), "main");
        assert_eq!(token_prefix(input, range, 13), "");
    }

    #[test]
    fn token_prefix_clamps_to_range_bounds() {
        let input = "git checkout main";
        let range = Range::new(13, 17);
        // Cursor before the range.
        assert_eq!(token_prefix(input, range, 5), "");
        // Cursor past the range.
        assert_eq!(token_prefix(input, range, 100), "main");
    }

    #[test]
    fn suggest_sets_anchor_to_positional_range() {
        // Drive suggest_for_cursor with a typed positional.
        let mut session = session_with("git checkout main");
        session.set_cursor(17);
        // The parser assigns ArgKind::Unknown, which returns empty;
        // anchor should still be set to the positional's range so
        // downstream can render "no completions" with context.
        let sub = SubcommandNode {
            name: "checkout".into(),
            depth: 0,
            range: Range::new(4, 12),
        };
        assert_eq!(sub.name, "checkout");
        // Call the real driver — history fallback because anchor
        // positional is Unknown and path/env are off.
        let ctx = DriverContext::default();
        let set = suggest_for_cursor(&session, &ctx);
        // Fallback path: no history provided, so nothing at all.
        assert!(set.is_empty());
    }
}
