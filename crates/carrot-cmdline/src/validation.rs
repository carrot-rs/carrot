//! Semantic validation of a parsed [`CommandAst`].
//!
//! Once the syntax layer (parser → `ArgKind`-typed positionals)
//! has tagged each token with its intended role, the validator
//! checks the **live state** of that role against the environment:
//!
//! | Role | Validator checks |
//! |------|------------------|
//! | `Path { must_exist: true }` | path exists on disk |
//! | `GitRef { scope }` | ref is present in the provided ref set |
//! | `Enum(v)` | positional is in `v` |
//! | `Command` / flag name | is in the schema's allowed set |
//!
//! This module keeps the validation **pure**: callers pass in the
//! resolved data (existing paths, known refs, schema) and receive
//! a list of [`AstError`]s. File-system / git probing lives
//! elsewhere. That makes validation trivially testable and makes
//! the cmdline responsive — probe results are cached at a higher
//! layer, the validator just reads them.

use std::collections::HashSet;
use std::path::Path;

use crate::ast::{ArgKind, AstError, CommandAst, ErrorSeverity, Range};

/// Resolved live-state inputs that the validator reads instead of
/// probing itself. Every field is optional — omit what isn't
/// available and the validator simply skips the associated checks.
#[derive(Debug, Default, Clone)]
pub struct ValidationContext<'a> {
    /// Existing filesystem paths to consider "present". When
    /// `None`, path existence is not checked — omit the error
    /// rather than lie.
    pub existing_paths: Option<HashSet<&'a Path>>,
    /// Known git refs (branches / tags / remotes / short shas).
    /// Checked against positionals with `ArgKind::GitRef`.
    pub known_refs: Option<HashSet<&'a str>>,
    /// Names of commands the schema knows about. When the parsed
    /// command name isn't present, the validator emits a warning
    /// (not an error — the shell may still find it via `$PATH`).
    pub known_commands: Option<HashSet<&'a str>>,
}

/// Validate `ast` against `ctx`. Returns fresh errors — the caller
/// is responsible for merging / replacing the errors already on the
/// AST (we do not mutate to keep the borrow shape simple).
///
/// Every pipeline element is validated independently: each stage
/// has its own command name and its own positionals, and a pipeline
/// like `ls | git checkout xyz` should flag `xyz` as an unknown ref
/// on the second stage, not silently miss it.
pub fn validate(ast: &CommandAst, ctx: &ValidationContext<'_>) -> Vec<AstError> {
    let mut errors = Vec::new();

    for element in &ast.elements {
        // Unknown command → Hint (not Error — $PATH may still resolve).
        if let (Some(cmd), Some(commands)) = (&element.command, ctx.known_commands.as_ref())
            && !commands.contains(cmd.name.as_str())
        {
            errors.push(AstError {
                range: cmd.range,
                message: format!("unknown command `{}`", cmd.name),
                severity: ErrorSeverity::Hint,
            });
        }

        for positional in &element.positionals {
            match &positional.kind {
                ArgKind::Path {
                    must_exist: true, ..
                } => {
                    if let Some(paths) = ctx.existing_paths.as_ref()
                        && !paths.contains(Path::new(positional.value.as_str()))
                    {
                        errors.push(AstError {
                            range: positional.range,
                            message: format!("path `{}` does not exist", positional.value),
                            severity: ErrorSeverity::Error,
                        });
                    }
                }
                ArgKind::GitRef { .. } => {
                    if let Some(refs) = ctx.known_refs.as_ref()
                        && !refs.contains(positional.value.as_str())
                    {
                        errors.push(AstError {
                            range: positional.range,
                            message: format!("no ref named `{}`", positional.value),
                            severity: ErrorSeverity::Error,
                        });
                    }
                }
                ArgKind::Enum(variants) => {
                    if !variants.iter().any(|v| *v == positional.value) {
                        errors.push(AstError {
                            range: positional.range,
                            message: format!(
                                "`{}` is not one of [{}]",
                                positional.value,
                                variants.join(", ")
                            ),
                            severity: ErrorSeverity::Error,
                        });
                    }
                }
                ArgKind::Path {
                    must_exist: false, ..
                }
                | ArgKind::Url
                | ArgKind::EnvVar
                | ArgKind::ProcessId
                | ArgKind::Literal
                | ArgKind::Unknown => {}
            }
        }
    }

    errors
}

/// Convenience helper: attach the computed errors to `ast` in
/// place, replacing any prior validation errors. Returns the number
/// of new errors attached.
pub fn apply_validation(ast: &mut CommandAst, new_errors: Vec<AstError>) -> usize {
    let count = new_errors.len();
    ast.errors = new_errors;
    count
}

// Small helper to keep `!matches!` patterns readable in tests.
#[allow(dead_code)]
fn is_error(e: &AstError) -> bool {
    matches!(e.severity, ErrorSeverity::Error)
}

// Workaround for the private `Range` constructor only being needed
// in tests — we don't want to widen the public API.
#[allow(dead_code)]
fn make_range(start: usize, end: usize) -> Range {
    Range::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CommandNode, GitScope, PathKind, PipelineElement, PositionalNode};

    fn ast_with_gitref() -> CommandAst {
        CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "git".into(),
                range: Range::new(0, 3),
            }),
            positionals: vec![PositionalNode {
                value: "main".into(),
                kind: ArgKind::GitRef {
                    scope: GitScope::Branch,
                },
                range: Range::new(13, 17),
            }],
            ..PipelineElement::empty()
        })
    }

    fn ast_with_path(must_exist: bool) -> CommandAst {
        CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "cat".into(),
                range: Range::new(0, 3),
            }),
            positionals: vec![PositionalNode {
                value: "/etc/does-not-exist".into(),
                kind: ArgKind::Path {
                    must_exist,
                    kind: PathKind::File,
                },
                range: Range::new(4, 23),
            }],
            ..PipelineElement::empty()
        })
    }

    #[test]
    fn empty_context_yields_no_errors() {
        let ast = ast_with_gitref();
        let errors = validate(&ast, &ValidationContext::default());
        assert!(errors.is_empty());
    }

    #[test]
    fn unknown_command_emits_hint_not_error() {
        let ast = ast_with_gitref();
        let ctx = ValidationContext {
            known_commands: Some(["cargo", "ls"].into_iter().collect()),
            ..Default::default()
        };
        let errors = validate(&ast, &ctx);
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0].severity, ErrorSeverity::Hint));
        assert!(errors[0].message.contains("unknown command"));
    }

    #[test]
    fn known_command_passes_silently() {
        let ast = ast_with_gitref();
        let ctx = ValidationContext {
            known_commands: Some(["git"].into_iter().collect()),
            ..Default::default()
        };
        assert!(validate(&ast, &ctx).is_empty());
    }

    #[test]
    fn missing_git_ref_errors() {
        let ast = ast_with_gitref();
        let ctx = ValidationContext {
            known_refs: Some(["develop", "master"].into_iter().collect()),
            ..Default::default()
        };
        let errors = validate(&ast, &ctx);
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0].severity, ErrorSeverity::Error));
        assert!(errors[0].message.contains("no ref"));
    }

    #[test]
    fn present_git_ref_passes() {
        let ast = ast_with_gitref();
        let ctx = ValidationContext {
            known_refs: Some(["main", "develop"].into_iter().collect()),
            ..Default::default()
        };
        assert!(validate(&ast, &ctx).is_empty());
    }

    #[test]
    fn nonexistent_required_path_errors() {
        let ast = ast_with_path(true);
        let ctx = ValidationContext {
            existing_paths: Some(HashSet::new()),
            ..Default::default()
        };
        let errors = validate(&ast, &ctx);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("does not exist"));
    }

    #[test]
    fn path_without_must_exist_never_errors() {
        let ast = ast_with_path(false);
        let ctx = ValidationContext {
            existing_paths: Some(HashSet::new()),
            ..Default::default()
        };
        assert!(validate(&ast, &ctx).is_empty());
    }

    #[test]
    fn enum_variant_outside_set_errors() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "git".into(),
                range: Range::new(0, 3),
            }),
            positionals: vec![PositionalNode {
                value: "xaml".into(),
                kind: ArgKind::Enum(vec!["json", "yaml", "toml"]),
                range: Range::new(4, 8),
            }],
            ..PipelineElement::empty()
        });
        let errors = validate(&ast, &ValidationContext::default());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("not one of"));
    }

    #[test]
    fn enum_variant_inside_set_passes() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "git".into(),
                range: Range::new(0, 3),
            }),
            positionals: vec![PositionalNode {
                value: "yaml".into(),
                kind: ArgKind::Enum(vec!["json", "yaml", "toml"]),
                range: Range::new(4, 8),
            }],
            ..PipelineElement::empty()
        });
        assert!(validate(&ast, &ValidationContext::default()).is_empty());
    }

    #[test]
    fn pipeline_validates_each_stage_independently() {
        // Stage 1: `ls` (known). Stage 2: `bogus` (unknown). Only the
        // second should produce an error.
        let ast = CommandAst {
            elements: vec![
                PipelineElement {
                    command: Some(CommandNode {
                        name: "ls".into(),
                        range: Range::new(0, 2),
                    }),
                    ..PipelineElement::empty()
                },
                PipelineElement {
                    separator: Some(crate::ast::Separator {
                        kind: crate::ast::SeparatorKind::Pipe,
                        range: Range::new(3, 4),
                    }),
                    command: Some(CommandNode {
                        name: "bogus".into(),
                        range: Range::new(5, 10),
                    }),
                    ..PipelineElement::empty()
                },
            ],
            errors: Vec::new(),
        };
        let ctx = ValidationContext {
            known_commands: Some(["ls", "grep", "wc"].into_iter().collect()),
            ..Default::default()
        };
        let errors = validate(&ast, &ctx);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("bogus"));
    }

    #[test]
    fn apply_validation_replaces_prior_errors() {
        let mut ast = ast_with_gitref();
        ast.errors.push(AstError {
            range: Range::new(0, 1),
            message: "stale".into(),
            severity: ErrorSeverity::Warning,
        });
        let new = vec![AstError {
            range: Range::new(13, 17),
            message: "no ref named `main`".into(),
            severity: ErrorSeverity::Error,
        }];
        let count = apply_validation(&mut ast, new);
        assert_eq!(count, 1);
        assert_eq!(ast.errors.len(), 1);
        assert!(matches!(ast.errors[0].severity, ErrorSeverity::Error));
    }

    #[test]
    fn missing_context_skips_checks_gracefully() {
        // With no context at all, even a clearly wrong ref doesn't
        // produce an error — we refuse to lie.
        let ast = ast_with_gitref();
        let errors = validate(&ast, &ValidationContext::default());
        assert!(errors.is_empty());
    }

    #[test]
    fn error_range_matches_offending_token() {
        let ast = ast_with_gitref();
        let ctx = ValidationContext {
            known_refs: Some(HashSet::new()),
            ..Default::default()
        };
        let errors = validate(&ast, &ctx);
        assert_eq!(errors[0].range, Range::new(13, 17));
    }

    #[test]
    fn is_error_helper_matches_only_error_severity() {
        let err = AstError {
            range: Range::new(0, 0),
            message: String::new(),
            severity: ErrorSeverity::Error,
        };
        let warn = AstError {
            range: Range::new(0, 0),
            message: String::new(),
            severity: ErrorSeverity::Warning,
        };
        assert!(is_error(&err));
        assert!(!is_error(&warn));
    }
}
