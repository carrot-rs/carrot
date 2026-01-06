//! Semantic-AST-driven screen-reader narration.
//!
//! Plain CLIs are unstructured text — hostile to screen readers.
//! Because the cmdline owns the semantic AST, we can narrate the
//! structure of the input:
//!
//! > "git command, checkout subcommand, branch argument, partial
//! > input m a, three suggestions: main, master, macos-fixes."
//!
//! instead of:
//!
//! > "git space checkout space m a."
//!
//! This module produces those narration strings from a [`CommandAst`]
//! plus an optional [`CompletionSet`], and exposes IME composition
//! text + candidates as distinct AccessKit nodes so CJK screen
//! readers get the composition string + alternatives, not only the
//! final committed text.
//!
//! The resulting `NarrationLine` is consumed by the AccessKit
//! adapter in `inazuma::accessibility`, which wraps each line in
//! an `accesskit::Node`.

use crate::ast::{ArgKind, CommandAst, FlagNode, GitScope, PipelineElement, PositionalNode};
use crate::completion::CompletionSet;

/// A line of screen-reader narration. Produced from an AST +
/// optional completion set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrationLine {
    pub text: String,
    pub role: NarrationRole,
}

/// What semantic slot the narration line describes. Consumers
/// (AccessKit adapter, UIA, AT-SPI) map this onto their platform
/// role vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NarrationRole {
    /// The command name (first token).
    Command,
    /// The subcommand (second token), if any.
    Subcommand,
    /// A flag (`-v`, `--color=auto`).
    Flag,
    /// A typed positional argument.
    Positional,
    /// A semantic error on a positional.
    Error,
    /// The completion dropdown summary ("three suggestions: ...").
    CompletionSummary,
    /// The IME composition string (pre-commit input from CJK etc.).
    Composition,
    /// An IME composition candidate.
    CompositionCandidate,
    /// A pipeline separator ("pipe stage 2").
    PipelineSeparator,
}

/// Produce narration lines for an AST.
///
/// Single-stage output is deterministic and ordered: command →
/// subcommand → flags → positionals. Multi-stage pipelines prefix
/// each stage's lines with a `PipelineSeparator` announcement so
/// screen readers understand the stage boundary, e.g. "pipe stage
/// 2". Errors are narrated last, independent of stage.
///
/// Empty AST produces an empty vector.
pub fn narrate_ast(ast: &CommandAst) -> Vec<NarrationLine> {
    let mut out = Vec::new();
    let multi = ast.elements.len() > 1;
    for (idx, element) in ast.elements.iter().enumerate() {
        if multi && idx > 0 {
            out.push(NarrationLine {
                text: format!("pipe stage {}", idx + 1),
                role: NarrationRole::PipelineSeparator,
            });
        }
        narrate_element(element, &mut out);
    }
    for err in &ast.errors {
        if matches!(err.severity, crate::ast::ErrorSeverity::Error) {
            out.push(NarrationLine {
                text: format!("error: {}", err.message),
                role: NarrationRole::Error,
            });
        }
    }
    out
}

fn narrate_element(element: &PipelineElement, out: &mut Vec<NarrationLine>) {
    if let Some(cmd) = &element.command {
        out.push(NarrationLine {
            text: format!("{} command", cmd.name),
            role: NarrationRole::Command,
        });
    }
    if let Some(sub) = &element.subcommand {
        out.push(NarrationLine {
            text: format!("{} subcommand", sub.name),
            role: NarrationRole::Subcommand,
        });
    }
    for flag in &element.flags {
        out.push(NarrationLine {
            text: flag_description(flag),
            role: NarrationRole::Flag,
        });
    }
    for positional in &element.positionals {
        out.push(NarrationLine {
            text: positional_description(positional),
            role: NarrationRole::Positional,
        });
    }
}

fn flag_description(flag: &FlagNode) -> String {
    let style = if flag.is_long {
        "long flag"
    } else {
        "short flag"
    };
    match &flag.value {
        Some(v) => format!("{style} {} with value {v}", flag.name),
        None => format!("{style} {}", flag.name),
    }
}

fn positional_description(p: &PositionalNode) -> String {
    let kind = match &p.kind {
        ArgKind::Path { .. } => "path argument",
        ArgKind::GitRef { scope } => match scope {
            GitScope::Branch => "branch argument",
            GitScope::Tag => "tag argument",
            GitScope::Remote => "remote argument",
            GitScope::Commit => "commit argument",
            GitScope::Any => "git reference argument",
        },
        ArgKind::Url => "URL argument",
        ArgKind::EnvVar => "environment variable argument",
        ArgKind::ProcessId => "process ID argument",
        ArgKind::Enum(_) => "option argument",
        ArgKind::Literal | ArgKind::Unknown => "argument",
    };
    format!("{kind} {}", p.value)
}

/// Produce a one-line narration summary for a completion set.
/// Returns `None` when the set is empty. Caps the listed labels at
/// `max_inline` — beyond that the count only is read aloud.
pub fn narrate_completion_summary(set: &CompletionSet, max_inline: usize) -> Option<NarrationLine> {
    if set.candidates.is_empty() {
        return None;
    }
    let count = set.candidates.len();
    let max_inline = max_inline.max(1);
    let text = if count <= max_inline {
        let labels: Vec<&str> = set.candidates.iter().map(|c| c.label.as_str()).collect();
        match count {
            1 => format!("one suggestion: {}", labels[0]),
            _ => format!("{} suggestions: {}", number_word(count), labels.join(", ")),
        }
    } else {
        format!("{count} suggestions available, press arrow keys to browse")
    };
    Some(NarrationLine {
        text,
        role: NarrationRole::CompletionSummary,
    })
}

fn number_word(n: usize) -> String {
    match n {
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        6 => "six".into(),
        7 => "seven".into(),
        8 => "eight".into(),
        9 => "nine".into(),
        10 => "ten".into(),
        _ => n.to_string(),
    }
}

/// IME composition snapshot. Separate fields so each becomes a
/// dedicated AccessKit node — composing text and candidates are
/// exposed distinctly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImeComposition {
    /// Pre-commit composing text as the user types (e.g. Pinyin
    /// romaji before commit, Japanese hiragana before kanji pick).
    pub composing: String,
    /// Candidate conversions the IME is offering. Empty when the
    /// IME hasn't produced candidates yet.
    pub candidates: Vec<String>,
    /// Currently highlighted candidate index, if the IME reports one.
    pub selected_candidate: Option<usize>,
}

impl ImeComposition {
    pub fn is_active(&self) -> bool {
        !self.composing.is_empty() || !self.candidates.is_empty()
    }

    pub fn narrate(&self) -> Vec<NarrationLine> {
        let mut out = Vec::new();
        if !self.composing.is_empty() {
            out.push(NarrationLine {
                text: format!("composing {}", self.composing),
                role: NarrationRole::Composition,
            });
        }
        for (i, cand) in self.candidates.iter().enumerate() {
            let selected = self.selected_candidate == Some(i);
            out.push(NarrationLine {
                text: if selected {
                    format!("candidate {cand}, selected")
                } else {
                    format!("candidate {cand}")
                },
                role: NarrationRole::CompositionCandidate,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstError, CommandNode, ErrorSeverity, PathKind, Range, SubcommandNode};
    use crate::completion::{CompletionCandidate, CompletionSet, CompletionSource};

    fn git_checkout_main_ast() -> CommandAst {
        CommandAst::from_element(PipelineElement {
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
                value: "ma".into(),
                kind: ArgKind::GitRef {
                    scope: GitScope::Branch,
                },
                range: Range::new(13, 15),
            }],
            ..PipelineElement::empty()
        })
    }

    #[test]
    fn narrate_ast_emits_command_then_subcommand_then_positional() {
        let ast = git_checkout_main_ast();
        let lines = narrate_ast(&ast);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].role, NarrationRole::Command);
        assert_eq!(lines[0].text, "git command");
        assert_eq!(lines[1].role, NarrationRole::Subcommand);
        assert_eq!(lines[1].text, "checkout subcommand");
        assert_eq!(lines[2].role, NarrationRole::Positional);
        assert_eq!(lines[2].text, "branch argument ma");
    }

    #[test]
    fn narrate_ast_covers_flags_with_value() {
        let ast = crate::parse::parse_simple("ls -la --color=auto");
        let lines = narrate_ast(&ast);
        assert!(
            lines
                .iter()
                .any(|l| l.text == "short flag la" && l.role == NarrationRole::Flag)
        );
        assert!(
            lines
                .iter()
                .any(|l| l.text == "long flag color with value auto")
        );
    }

    #[test]
    fn narrate_ast_covers_path_positional() {
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
        let lines = narrate_ast(&ast);
        assert!(lines.iter().any(|l| l.text == "path argument /etc/hosts"));
    }

    #[test]
    fn pipeline_narration_includes_stage_boundaries() {
        let ast = crate::parse::parse_simple("ls | grep foo");
        let lines = narrate_ast(&ast);
        assert!(
            lines
                .iter()
                .any(|l| l.role == NarrationRole::PipelineSeparator && l.text == "pipe stage 2")
        );
        let command_count = lines
            .iter()
            .filter(|l| l.role == NarrationRole::Command)
            .count();
        assert_eq!(command_count, 2);
    }

    #[test]
    fn narrate_ast_emits_error_for_severity_error_only() {
        let mut ast = git_checkout_main_ast();
        ast.errors.push(AstError {
            range: Range::new(13, 15),
            message: "no branch or tag named `ma`".into(),
            severity: ErrorSeverity::Error,
        });
        ast.errors.push(AstError {
            range: Range::new(0, 3),
            message: "deprecated command".into(),
            severity: ErrorSeverity::Warning,
        });
        let lines = narrate_ast(&ast);
        let errors: Vec<_> = lines
            .iter()
            .filter(|l| l.role == NarrationRole::Error)
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].text.contains("no branch or tag"));
    }

    #[test]
    fn completion_summary_uses_word_for_small_counts() {
        let mut set = CompletionSet::new();
        for label in ["main", "master", "macos-fixes"] {
            set.candidates.push(CompletionCandidate::replace(
                CompletionSource::Git,
                label,
                Range::new(0, 0),
            ));
        }
        let line = narrate_completion_summary(&set, 5).unwrap();
        assert_eq!(line.role, NarrationRole::CompletionSummary);
        assert_eq!(line.text, "three suggestions: main, master, macos-fixes");
    }

    #[test]
    fn completion_summary_summarises_large_set_by_count() {
        let mut set = CompletionSet::new();
        for i in 0..42 {
            set.candidates.push(CompletionCandidate::replace(
                CompletionSource::History,
                format!("cmd{i}"),
                Range::new(0, 0),
            ));
        }
        let line = narrate_completion_summary(&set, 5).unwrap();
        assert!(line.text.starts_with("42 suggestions available"));
    }

    #[test]
    fn completion_summary_one_suggestion_wording() {
        let mut set = CompletionSet::new();
        set.candidates.push(CompletionCandidate::replace(
            CompletionSource::Git,
            "main",
            Range::new(0, 0),
        ));
        let line = narrate_completion_summary(&set, 5).unwrap();
        assert_eq!(line.text, "one suggestion: main");
    }

    #[test]
    fn completion_summary_empty_returns_none() {
        let set = CompletionSet::new();
        assert!(narrate_completion_summary(&set, 5).is_none());
    }

    #[test]
    fn ime_composition_is_active_when_composing_present() {
        let comp = ImeComposition {
            composing: "ni".into(),
            candidates: Vec::new(),
            selected_candidate: None,
        };
        assert!(comp.is_active());
    }

    #[test]
    fn ime_composition_narration_announces_composing() {
        let comp = ImeComposition {
            composing: "ni".into(),
            candidates: vec!["你".into(), "尼".into(), "妮".into()],
            selected_candidate: Some(0),
        };
        let lines = comp.narrate();
        assert_eq!(lines[0].text, "composing ni");
        assert_eq!(lines[1].text, "candidate 你, selected");
        assert_eq!(lines[2].text, "candidate 尼");
        assert_eq!(lines[3].text, "candidate 妮");
    }

    #[test]
    fn ime_composition_is_inactive_when_empty() {
        let comp = ImeComposition::default();
        assert!(!comp.is_active());
        assert!(comp.narrate().is_empty());
    }

    #[test]
    fn narrate_ast_on_empty_returns_empty() {
        let ast = CommandAst::empty();
        assert!(narrate_ast(&ast).is_empty());
    }
}
