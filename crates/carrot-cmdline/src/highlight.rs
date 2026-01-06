//! Highlight span model.
//!
//! The cmdline renders each token with a role-specific colour —
//! commands in one tone, subcommands in another, git-refs / paths /
//! URLs in a third, invalid tokens with an underline. The rendering
//! layer consumes [`HighlightSpan`]s; this module owns the mapping
//! from [`CommandAst`] to those spans.
//!
//! # Why roles, not regex
//!
//! A regex tokeniser can tell you "this is a word". The AST tells
//! you "this word is a git-ref under `checkout` — look it up in the
//! live branches source". Regex-based highlight is fine for a
//! fallback shell, but the whole point of the semantic AST is that
//! downstream layers (highlight, validation, completion, a11y) can
//! ground each span in the command's schema rather than guessing.
//!
//! # Scope
//!
//! Data types plus one transform — `highlight_ast` that turns a
//! parsed AST into a vector of spans. Colour / theme resolution
//! happens in the rendering layer; this module stops at roles.

use crate::ast::{ArgKind, CommandAst, Range};

// Re-exported just for the test module below.
#[cfg(test)]
use crate::ast::PipelineElement;

/// The semantic role of a highlighted span. Rendering layer picks a
/// theme colour per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightRole {
    Command,
    Subcommand,
    LongFlag,
    ShortFlag,
    FlagValue,
    Path,
    GitRef,
    Url,
    EnvVar,
    ProcessId,
    EnumLiteral,
    Positional,
    /// Pipeline / sequence separator (`|`, `&&`, `||`, `;`).
    Separator,
    Error,
}

impl HighlightRole {
    /// Whether this role should render with an error underline on
    /// top of the base colour. Semantic validation of live state
    /// attaches `Error` separately; this is about the role's
    /// inherent badge (e.g. a typo'd flag).
    pub fn is_error(self) -> bool {
        matches!(self, HighlightRole::Error)
    }
}

/// A contiguous byte range inside the input annotated with a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HighlightSpan {
    pub range: Range,
    pub role: HighlightRole,
}

impl HighlightSpan {
    pub fn new(range: Range, role: HighlightRole) -> Self {
        Self { range, role }
    }
}

/// Project the AST into highlight spans, ordered by byte range.
///
/// Each pipeline element contributes its own Command / Subcommand /
/// Flag / Positional spans. Separator tokens (`|` between stages)
/// are rendered separately so the theme can role-colour the pipe
/// character. Error-severity entries in `ast.errors` produce
/// `HighlightRole::Error` spans that layer on top of the role of
/// the underlying token — the rendering layer draws both the base
/// colour and the error underline.
pub fn highlight_ast(ast: &CommandAst) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();

    for element in &ast.elements {
        if let Some(sep) = &element.separator {
            spans.push(HighlightSpan::new(sep.range, HighlightRole::Separator));
        }
        if let Some(cmd) = &element.command {
            spans.push(HighlightSpan::new(cmd.range, HighlightRole::Command));
        }
        if let Some(sub) = &element.subcommand {
            spans.push(HighlightSpan::new(sub.range, HighlightRole::Subcommand));
        }
        for flag in &element.flags {
            spans.push(HighlightSpan::new(
                flag.range,
                if flag.is_long {
                    HighlightRole::LongFlag
                } else {
                    HighlightRole::ShortFlag
                },
            ));
        }
        for positional in &element.positionals {
            spans.push(HighlightSpan::new(
                positional.range,
                role_for_arg_kind(&positional.kind),
            ));
        }
    }
    for err in &ast.errors {
        if matches!(err.severity, crate::ast::ErrorSeverity::Error) {
            spans.push(HighlightSpan::new(err.range, HighlightRole::Error));
        }
    }

    spans.sort_by_key(|s| (s.range.start, s.range.end));
    spans
}

fn role_for_arg_kind(kind: &ArgKind) -> HighlightRole {
    match kind {
        ArgKind::Path { .. } => HighlightRole::Path,
        ArgKind::GitRef { .. } => HighlightRole::GitRef,
        ArgKind::Url => HighlightRole::Url,
        ArgKind::EnvVar => HighlightRole::EnvVar,
        ArgKind::ProcessId => HighlightRole::ProcessId,
        ArgKind::Enum(_) => HighlightRole::EnumLiteral,
        ArgKind::Literal | ArgKind::Unknown => HighlightRole::Positional,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        AstError, CommandNode, ErrorSeverity, GitScope, PathKind, PositionalNode, SubcommandNode,
    };
    use crate::parse::parse_simple;

    #[test]
    fn command_and_subcommand_spans_emitted() {
        let ast = parse_simple("git checkout main");
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::Command));
        assert!(spans.iter().any(|s| s.role == HighlightRole::Subcommand));
    }

    #[test]
    fn flags_classified_long_vs_short() {
        let ast = parse_simple("ls -la --color=auto");
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::ShortFlag));
        assert!(spans.iter().any(|s| s.role == HighlightRole::LongFlag));
    }

    #[test]
    fn spans_ordered_by_range() {
        let ast = parse_simple("git checkout main");
        let spans = highlight_ast(&ast);
        for win in spans.windows(2) {
            assert!(win[0].range.start <= win[1].range.start);
        }
    }

    #[test]
    fn error_severity_produces_error_span() {
        let mut ast = parse_simple("git checkout main");
        ast.errors.push(AstError {
            range: Range::new(13, 17),
            message: "no branch named main".into(),
            severity: ErrorSeverity::Error,
        });
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::Error));
    }

    #[test]
    fn warning_severity_does_not_emit_error_span() {
        let mut ast = parse_simple("ls -la");
        ast.errors.push(AstError {
            range: Range::new(3, 6),
            message: "maybe a typo".into(),
            severity: ErrorSeverity::Warning,
        });
        let spans = highlight_ast(&ast);
        assert!(!spans.iter().any(|s| s.role == HighlightRole::Error));
    }

    #[test]
    fn git_ref_arg_kind_produces_git_ref_role() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "git".into(),
                range: Range::new(0, 3),
            }),
            subcommand: Some(SubcommandNode {
                name: "checkout".into(),
                depth: 0,
                range: Range::new(4, 12),
            }),
            positionals: vec![PositionalNode {
                value: "main".into(),
                kind: ArgKind::GitRef {
                    scope: GitScope::Branch,
                },
                range: Range::new(13, 17),
            }],
            ..PipelineElement::empty()
        });
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::GitRef));
    }

    #[test]
    fn path_arg_kind_produces_path_role() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "cat".into(),
                range: Range::new(0, 3),
            }),
            positionals: vec![PositionalNode {
                value: "/etc/hosts".into(),
                kind: ArgKind::Path {
                    must_exist: true,
                    kind: PathKind::File,
                },
                range: Range::new(4, 14),
            }],
            ..PipelineElement::empty()
        });
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::Path));
    }

    #[test]
    fn empty_ast_produces_no_spans() {
        let ast = CommandAst::empty();
        let spans = highlight_ast(&ast);
        assert!(spans.is_empty());
    }

    #[test]
    fn is_error_classification() {
        assert!(HighlightRole::Error.is_error());
        assert!(!HighlightRole::Command.is_error());
        assert!(!HighlightRole::GitRef.is_error());
    }

    #[test]
    fn flag_with_value_still_emits_single_span() {
        // Current parser produces a single FlagNode with value set;
        // highlighter emits one LongFlag span over the full range.
        let ast = parse_simple("ls --color=auto");
        let flag_spans: Vec<_> = highlight_ast(&ast)
            .into_iter()
            .filter(|s| s.role == HighlightRole::LongFlag)
            .collect();
        assert_eq!(flag_spans.len(), 1);
    }

    #[test]
    fn unknown_positional_maps_to_positional_role() {
        let ast = parse_simple("git checkout main");
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::Positional));
    }

    #[test]
    fn short_flag_use_uses_short_flag_role() {
        let ast = parse_simple("rm -f");
        let spans = highlight_ast(&ast);
        // -f is actually parsed as subcommand by simple parser
        // because rm is the command and -f follows. But the simple
        // parser classifies flags before subcommand assignment, so
        // -f should be in the flags list.
        let has_short_flag = spans.iter().any(|s| s.role == HighlightRole::ShortFlag);
        assert!(has_short_flag, "spans = {spans:?}");
    }

    #[test]
    fn fn_fn_env_var_role_is_mapped() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "echo".into(),
                range: Range::new(0, 4),
            }),
            positionals: vec![PositionalNode {
                value: "$HOME".into(),
                kind: ArgKind::EnvVar,
                range: Range::new(5, 10),
            }],
            ..PipelineElement::empty()
        });
        let spans = highlight_ast(&ast);
        assert!(spans.iter().any(|s| s.role == HighlightRole::EnvVar));
    }

    #[test]
    fn pipeline_emits_separator_spans() {
        let ast = parse_simple("ls | grep foo");
        let spans = highlight_ast(&ast);
        let sep_count = spans
            .iter()
            .filter(|s| s.role == HighlightRole::Separator)
            .count();
        assert_eq!(sep_count, 1);
        // Each stage contributes a Command span.
        let cmd_count = spans
            .iter()
            .filter(|s| s.role == HighlightRole::Command)
            .count();
        assert_eq!(cmd_count, 2);
    }
}
