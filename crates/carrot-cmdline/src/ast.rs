//! Semantic Command AST — the crate's own contribution.
//!
//! Every keystroke in the cmdline feeds a tree-sitter (or native,
//! for fish 4.0+) reparse; the resulting syntax tree is typed
//! against the 715+ `carrot-completions` specs to produce a
//! **semantic** interpretation: not "tokens", but "this token is a
//! git ref", "this is a path that must exist", "this is a typo that
//! should be a `--flag`".
//!
//! What this unlocks:
//!
//! - **Role-based syntax highlight.** Paths one colour, git-refs
//!   another, URLs a third — schema-driven, not regex.
//! - **Semantic validation.** `git checkout ma` → the token `ma` is
//!   a `GitRef`, underlined red if no branch matches.
//! - **Typed completions.** Completion candidates come from the
//!   appropriate source (git branches, filesystem, env vars) rather
//!   than a generic word list.
//! - **Structured AI context.** Predictions see `{command: git,
//!   subcommand: checkout, partial: GitRef("ma")}`, not a string.
//! - **Structured agent handoff.** On `#`, the agent receives the
//!   AST, not raw text.
//! - **Structured accessibility.** Screen readers narrate roles.
//!
//! # Separation of concerns
//!
//! This module owns the **data types** only. The parser that
//! populates a `CommandAst` lives in [`crate::parse`] (fallback) and
//! the per-shell modules under `syntax/` (tree-sitter / native AST).
//! Schema lookup that drives typed positionals lives in
//! [`crate::validation`]. Keeping shape separate from producers
//! means every consumer (highlight, validate, agent handoff) reads
//! the same invariant through one import.

/// Type of a positional argument, as declared by the command spec.
///
/// Unknown values fall back to [`ArgKind::Unknown`] — the AST stays
/// complete even when the schema doesn't classify a token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArgKind {
    /// Filesystem path.
    Path {
        /// If `true`, the AST validator reports an error when the
        /// path doesn't exist on disk.
        must_exist: bool,
        /// Expected shape: file, directory, or either.
        kind: PathKind,
    },
    /// A git ref — branch / tag / remote / commit hash.
    GitRef { scope: GitScope },
    /// An RFC-3986 URL.
    Url,
    /// Environment variable name (e.g. `$EDITOR`, `$HOME`).
    EnvVar,
    /// Unix process id.
    ProcessId,
    /// One of a known set of literals (e.g. `--format json|yaml`).
    Enum(Vec<&'static str>),
    /// Free-form literal — the command takes it verbatim.
    Literal,
    /// Schema classified the token but we don't understand the
    /// category. Treated as free-form at render time.
    Unknown,
}

/// Expected filesystem shape for a `Path` arg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    File,
    Directory,
    Any,
}

/// Scope of a git-ref positional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitScope {
    Branch,
    Tag,
    Remote,
    Commit,
    /// Any of the above — the command is ref-agnostic.
    Any,
}

/// The top-level command — `git` in `git checkout main`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandNode {
    /// Name as the user typed it.
    pub name: String,
    /// Byte range inside the input string. Usable for highlight-
    /// span attachment.
    pub range: Range,
}

/// Subcommand — `checkout` in `git checkout main`. Nested
/// subcommands (e.g. `aws s3 ls`) produce multiple siblings at
/// increasing depth — `depth: 0` for `s3`, `depth: 1` for `ls`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubcommandNode {
    pub name: String,
    pub depth: u8,
    pub range: Range,
}

/// A flag — short (`-v`) or long (`--verbose`), optionally with a
/// value (`--format json`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlagNode {
    pub name: String,
    pub value: Option<String>,
    /// `true` for `--long`, `false` for `-s`.
    pub is_long: bool,
    pub range: Range,
}

/// A positional argument plus its classification from the schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PositionalNode {
    pub value: String,
    pub kind: ArgKind,
    pub range: Range,
}

/// A semantic error — the input is syntactically valid (tree-sitter
/// accepted it) but the type or the live state of the system says
/// something is off.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AstError {
    pub range: Range,
    pub message: String,
    pub severity: ErrorSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorSeverity {
    /// Underlined red, blocks run-on-submit.
    Error,
    /// Underlined yellow, run-on-submit still allowed.
    Warning,
    /// Informational — typo suggestion, e.g. "did you mean --verbose?".
    Hint,
}

/// A byte range inside the input string. Half-open `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl Range {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }
}

/// Kind of separator token between two pipeline elements.
///
/// Mirrors the shell-level connector taxonomy: a `|` pipes stdout,
/// `&&`/`||` gate the next stage on exit status, `;` (or newline)
/// sequences unconditionally. Today's parsers only emit `Pipe`; the
/// other variants are reserved so we can extend syntactic support
/// without breaking the AST surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeparatorKind {
    /// `|` — stdout of the previous element feeds stdin of this one.
    Pipe,
    /// `&&` — run this element only if the previous succeeded.
    And,
    /// `||` — run this element only if the previous failed.
    Or,
    /// `;` or newline — run this element regardless of exit status.
    Sequence,
}

/// A separator token preceding a pipeline element. Carries both the
/// kind and the byte range of the token itself, so the highlighter
/// role-colours the connector and validation can attach errors to
/// the exact separator (e.g. "piped into a command that doesn't
/// read stdin").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Separator {
    pub kind: SeparatorKind,
    pub range: Range,
}

/// One element of a pipeline — the parsed shape of a single command
/// between separators.
///
/// A non-piped input `git checkout main` produces exactly one
/// element with `separator == None`. `ls | where size > 1mb | select
/// name` produces three elements; the first has `separator == None`,
/// the next two carry `Separator { kind: Pipe, range }` pointing at
/// the `|` token that precedes them.
///
/// Matches the reference shape from Nushell's `PipelineElement`
/// (`nu-protocol/src/ast/pipeline.rs`) with the separator unified
/// into an enum so the same container handles `|` / `&&` / `||` /
/// `;` without a second layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct PipelineElement {
    /// Separator token preceding this element. `None` for the very
    /// first element in the pipeline.
    pub separator: Option<Separator>,
    /// Top-level command of this element.
    pub command: Option<CommandNode>,
    /// First subcommand if the spec declares one.
    pub subcommand: Option<SubcommandNode>,
    /// Flags attached to this element — `--flag`, `-f`, `--key=val`.
    pub flags: Vec<FlagNode>,
    /// Positional arguments of this element.
    pub positionals: Vec<PositionalNode>,
}

impl PipelineElement {
    /// Shorthand for an empty element — no command yet, used when a
    /// walker can't resolve the stage but wants a placeholder.
    pub fn empty() -> Self {
        Self::default()
    }

    /// `true` if this element has a command node — distinguishes
    /// "parsed but empty" from "parsed with a command".
    pub fn has_command(&self) -> bool {
        self.command.is_some()
    }

    /// Return the positional (if any) that covers `byte_offset`
    /// within this element's range span.
    pub fn positional_at(&self, byte_offset: usize) -> Option<&PositionalNode> {
        self.positionals
            .iter()
            .find(|p| p.range.contains(byte_offset))
    }

    /// Depth of the command chain in this element.
    pub fn depth(&self) -> u8 {
        self.subcommand.as_ref().map(|s| s.depth + 1).unwrap_or(0)
    }
}

/// The full parsed command as understood by the cmdline.
///
/// # Pipeline representation
///
/// A parsed input is always a sequence of [`PipelineElement`]s. A
/// single command (`ls`) produces `elements.len() == 1` with no
/// separator; a pipeline produces one element per stage with a
/// [`Separator`] attached to every element after the first.
///
/// Consumers that only care about the leading command call
/// [`CommandAst::first`]; consumers that understand pipelines
/// (screen-reader narration, agent handoff, AI prediction,
/// schema validation) iterate `ast.elements`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CommandAst {
    /// Pipeline stages in source order. Empty only when the input
    /// has produced no parseable command at all.
    pub elements: Vec<PipelineElement>,
    /// Top-level errors spanning the input or crossing stages. Per-
    /// element errors point at ranges inside a specific element.
    pub errors: Vec<AstError>,
}

impl CommandAst {
    /// Empty AST — no command parsed yet (the user hasn't typed
    /// anything, or the input is whitespace-only).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Convenience: AST from a single pipeline element (no pipes).
    pub fn from_element(element: PipelineElement) -> Self {
        Self {
            elements: if element == PipelineElement::empty() {
                Vec::new()
            } else {
                vec![element]
            },
            errors: Vec::new(),
        }
    }

    /// First element of the pipeline — the leading command. `None`
    /// when the input has produced no parseable element.
    pub fn first(&self) -> Option<&PipelineElement> {
        self.elements.first()
    }

    /// Whether the AST describes at least one parseable command.
    pub fn has_command(&self) -> bool {
        self.elements.iter().any(|e| e.command.is_some())
    }

    /// Whether any error has severity Error (run should be blocked).
    pub fn has_blocking_error(&self) -> bool {
        self.errors
            .iter()
            .any(|e| e.severity == ErrorSeverity::Error)
    }

    /// Return the positional (if any) that covers `byte_offset`,
    /// searching across every pipeline element. Used to decide which
    /// completion source to consult for the token under the cursor.
    pub fn positional_at(&self, byte_offset: usize) -> Option<&PositionalNode> {
        self.elements
            .iter()
            .flat_map(|e| e.positionals.iter())
            .find(|p| p.range.contains(byte_offset))
    }

    /// Return the element whose overall span contains `byte_offset`,
    /// useful for driving completion on the active stage of a
    /// pipeline.
    pub fn element_at(&self, byte_offset: usize) -> Option<&PipelineElement> {
        self.elements
            .iter()
            .find(|e| element_span_contains(e, byte_offset))
    }

    /// Depth of the leading element's command chain. Mirrors the
    /// previous flat-field `depth()` semantics.
    pub fn depth(&self) -> u8 {
        self.first().map(PipelineElement::depth).unwrap_or(0)
    }

    /// `true` if the input has more than one pipeline element.
    pub fn has_pipeline(&self) -> bool {
        self.elements.len() > 1
    }

    /// Total element count (0 for empty input).
    pub fn stage_count(&self) -> usize {
        self.elements.len()
    }

    /// Whether the AST is syntactically complete enough to submit.
    ///
    /// Used by the Cmdline view to decide whether `Enter` commits
    /// the command (complete) or inserts a newline (incomplete — the
    /// user is mid-heredoc, unclosed string, dangling pipe, etc.).
    ///
    /// Current heuristic (conservative — errs toward committing):
    /// - Input must contain at least one command
    /// - No blocking errors
    /// - The final element must have a command (not a dangling pipe)
    ///
    /// Shell-specific rules (unterminated strings, open heredocs,
    /// trailing backslash line-continuation) are layered on top by
    /// the shell-aware parsers that populate `errors`.
    pub fn is_complete(&self) -> bool {
        if !self.has_command() {
            return false;
        }
        if self.has_blocking_error() {
            return false;
        }
        // Dangling pipe / separator without a following command —
        // e.g. `ls |` while the user is still typing.
        matches!(self.elements.last(), Some(last) if last.command.is_some())
    }
}

fn element_span_contains(element: &PipelineElement, offset: usize) -> bool {
    let spans = std::iter::once(element.separator.as_ref().map(|s| s.range))
        .chain(std::iter::once(element.command.as_ref().map(|c| c.range)))
        .chain(std::iter::once(
            element.subcommand.as_ref().map(|s| s.range),
        ))
        .chain(element.flags.iter().map(|f| Some(f.range)))
        .chain(element.positionals.iter().map(|p| Some(p.range)))
        .flatten();
    let mut start = usize::MAX;
    let mut end = 0;
    let mut any = false;
    for span in spans {
        any = true;
        if span.start < start {
            start = span.start;
        }
        if span.end > end {
            end = span.end;
        }
    }
    any && offset >= start && offset < end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ast_has_no_command() {
        let ast = CommandAst::empty();
        assert!(!ast.has_command());
        assert!(!ast.has_blocking_error());
        assert_eq!(ast.depth(), 0);
    }

    #[test]
    fn range_helpers() {
        let r = Range::new(3, 7);
        assert_eq!(r.len(), 4);
        assert!(!r.is_empty());
        assert!(r.contains(3));
        assert!(r.contains(6));
        assert!(!r.contains(7)); // half-open
        assert!(!r.contains(2));
    }

    #[test]
    fn empty_range_is_empty() {
        let r = Range::new(5, 5);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn blocking_error_detected() {
        let mut ast = CommandAst::empty();
        ast.errors.push(AstError {
            range: Range::new(0, 3),
            message: "oh no".into(),
            severity: ErrorSeverity::Warning,
        });
        assert!(!ast.has_blocking_error());
        ast.errors.push(AstError {
            range: Range::new(5, 8),
            message: "stop".into(),
            severity: ErrorSeverity::Error,
        });
        assert!(ast.has_blocking_error());
    }

    #[test]
    fn positional_at_finds_covering_range() {
        let ast = CommandAst::from_element(PipelineElement {
            positionals: vec![
                PositionalNode {
                    value: "main".into(),
                    kind: ArgKind::GitRef {
                        scope: GitScope::Branch,
                    },
                    range: Range::new(13, 17),
                },
                PositionalNode {
                    value: "--force".into(),
                    kind: ArgKind::Literal,
                    range: Range::new(18, 25),
                },
            ],
            ..PipelineElement::empty()
        });
        let at_14 = ast.positional_at(14).expect("cover 14");
        assert_eq!(at_14.value, "main");
        assert!(ast.positional_at(100).is_none());
    }

    #[test]
    fn depth_counts_subcommand_chain() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "aws".into(),
                range: Range::new(0, 3),
            }),
            subcommand: Some(SubcommandNode {
                name: "ls".into(),
                depth: 1,
                range: Range::new(7, 9),
            }),
            ..PipelineElement::empty()
        });
        assert_eq!(ast.depth(), 2);
        assert!(ast.has_command());
    }

    #[test]
    fn pipeline_elements_round_trip() {
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
                    separator: Some(Separator {
                        kind: SeparatorKind::Pipe,
                        range: Range::new(3, 4),
                    }),
                    command: Some(CommandNode {
                        name: "grep".into(),
                        range: Range::new(5, 9),
                    }),
                    positionals: vec![PositionalNode {
                        value: "foo".into(),
                        kind: ArgKind::Unknown,
                        range: Range::new(10, 13),
                    }],
                    ..PipelineElement::empty()
                },
            ],
            errors: Vec::new(),
        };
        assert_eq!(ast.stage_count(), 2);
        assert!(ast.has_pipeline());
        assert_eq!(
            ast.first()
                .map(|e| e.command.as_ref().unwrap().name.as_str()),
            Some("ls")
        );
        let second = &ast.elements[1];
        assert_eq!(second.separator.map(|s| s.kind), Some(SeparatorKind::Pipe));
        let at_11 = ast.positional_at(11).expect("cover 11");
        assert_eq!(at_11.value, "foo");
        let active_stage = ast.element_at(11).expect("stage 2 covers 11");
        assert_eq!(active_stage.command.as_ref().unwrap().name, "grep");
    }

    #[test]
    fn arg_kind_variants_are_hash_compatible() {
        use std::collections::HashSet;
        let mut set: HashSet<ArgKind> = HashSet::new();
        set.insert(ArgKind::Url);
        set.insert(ArgKind::Url);
        set.insert(ArgKind::GitRef {
            scope: GitScope::Branch,
        });
        set.insert(ArgKind::Path {
            must_exist: true,
            kind: PathKind::File,
        });
        // Url deduplicates, GitRef + Path are distinct.
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn is_complete_rejects_empty_input() {
        assert!(!CommandAst::empty().is_complete());
    }

    #[test]
    fn is_complete_accepts_single_command() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "ls".into(),
                range: Range::new(0, 2),
            }),
            ..PipelineElement::empty()
        });
        assert!(ast.is_complete());
    }

    #[test]
    fn is_complete_rejects_dangling_pipe() {
        // `ls |` — first stage has a command, second is a bare pipe
        // element with no command yet.
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
                    separator: Some(Separator {
                        kind: SeparatorKind::Pipe,
                        range: Range::new(3, 4),
                    }),
                    ..PipelineElement::empty()
                },
            ],
            errors: Vec::new(),
        };
        assert!(!ast.is_complete());
    }

    #[test]
    fn is_complete_rejects_blocking_errors() {
        let mut ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "ls".into(),
                range: Range::new(0, 2),
            }),
            ..PipelineElement::empty()
        });
        ast.errors.push(AstError {
            range: Range::new(0, 2),
            message: "unterminated string".into(),
            severity: ErrorSeverity::Error,
        });
        assert!(!ast.is_complete());
    }

    #[test]
    fn ast_is_cloneable() {
        let ast = CommandAst::from_element(PipelineElement {
            command: Some(CommandNode {
                name: "ls".into(),
                range: Range::new(0, 2),
            }),
            ..PipelineElement::empty()
        });
        let cloned = ast.clone();
        assert_eq!(ast, cloned);
    }
}
