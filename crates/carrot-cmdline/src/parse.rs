//! Shell-agnostic fallback parser.
//!
//! When per-shell tree-sitter walkers can't make sense of partial
//! mid-typing input, the cmdline needs a baseline that still returns
//! a structured [`CommandAst`]. This module is that fallback: a
//! whitespace-tokenising parser that recognises pipe (`|`) tokens
//! and splits the input into [`PipelineElement`]s, filling each
//! element with a best-effort command / subcommand / flag /
//! positional classification.
//!
//! What it handles correctly:
//!
//! - Single / double quoted strings (quotes preserved as boundary
//!   markers; content kept literal).
//! - Short flags (`-v`, `-xzvf`) and long flags (`--verbose`,
//!   `--format=json`).
//! - A leading command per pipeline element, optional subcommand at
//!   depth 0, positional arguments afterwards.
//! - Pipe tokens (`|`) as element separators with accurate ranges.
//! - Byte ranges for every node so renderers / validators can map
//!   back to the input string.
//!
//! What it does NOT handle:
//!
//! - Schema-driven type classification — every positional comes
//!   back as [`ArgKind::Unknown`]. Real typing lives in
//!   `schema_typing.rs`.
//! - Non-pipe separators (`&&`, `||`, `;`), redirects, subshells,
//!   here-docs. The tree-sitter walkers per shell cover those.
//! - Brace / tilde / param expansion — preserved verbatim.

use crate::ast::{
    ArgKind, CommandAst, CommandNode, FlagNode, PipelineElement, PositionalNode, Range, Separator,
    SeparatorKind, SubcommandNode,
};

/// Parse a single shell command line with whitespace tokenisation.
///
/// Whitespace-only / empty inputs produce an empty AST.
pub fn parse_simple(input: &str) -> CommandAst {
    let tokens = tokenise(input);
    if tokens.is_empty() {
        return CommandAst::empty();
    }

    // Split tokens into pipeline elements on `|` separator tokens.
    let mut elements: Vec<PipelineElement> = Vec::new();
    let mut pending: Vec<Token<'_>> = Vec::new();
    let mut pending_separator: Option<Separator> = None;

    for tok in tokens {
        if tok.text == "|" {
            if !pending.is_empty() || pending_separator.is_some() {
                let drained = std::mem::take(&mut pending);
                elements.push(build_element(drained, pending_separator.take()));
            }
            pending_separator = Some(Separator {
                kind: SeparatorKind::Pipe,
                range: tok.range,
            });
        } else {
            pending.push(tok);
        }
    }
    if !pending.is_empty() || pending_separator.is_some() {
        elements.push(build_element(pending, pending_separator));
    }

    CommandAst {
        elements,
        errors: Vec::new(),
    }
}

fn build_element(tokens: Vec<Token<'_>>, separator: Option<Separator>) -> PipelineElement {
    let mut element = PipelineElement {
        separator,
        ..PipelineElement::empty()
    };
    let mut cursor = tokens.into_iter();
    let Some(first) = cursor.next() else {
        return element;
    };
    if is_flag(first.text) {
        element.flags.push(flag_from(first.text, first.range));
    } else {
        element.command = Some(CommandNode {
            name: first.text.to_string(),
            range: first.range,
        });
    }

    let mut saw_subcommand = false;
    for tok in cursor {
        if is_flag(tok.text) {
            element.flags.push(flag_from(tok.text, tok.range));
            continue;
        }
        if !saw_subcommand && element.command.is_some() {
            element.subcommand = Some(SubcommandNode {
                name: tok.text.to_string(),
                depth: 0,
                range: tok.range,
            });
            saw_subcommand = true;
            continue;
        }
        element.positionals.push(PositionalNode {
            value: tok.text.to_string(),
            kind: ArgKind::Unknown,
            range: tok.range,
        });
    }
    element
}

/// A literal token with its byte range in the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token<'a> {
    text: &'a str,
    range: Range,
}

/// Whitespace-tokenise, preserving quoted runs and emitting pipe
/// (`|`) as a standalone token. `||` is kept as a single two-char
/// token so the "or" separator isn't mis-read as two pipes (today
/// we ignore `||` in the fallback — only `|` produces a Separator).
fn tokenise(input: &str) -> Vec<Token<'_>> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                if i < bytes.len() {
                    // Include the closing quote in the range.
                    i += 1;
                }
            }
            b'|' => {
                i += 1;
                if i < bytes.len() && bytes[i] == b'|' {
                    // `||` — keep as one token, not a pipe separator.
                    i += 1;
                }
            }
            _ => {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'|' {
                    i += 1;
                }
            }
        }
        out.push(Token {
            text: &input[start..i],
            range: Range::new(start, i),
        });
    }
    out
}

fn is_flag(text: &str) -> bool {
    text.starts_with('-') && text.len() > 1
}

fn flag_from(text: &str, range: Range) -> FlagNode {
    let is_long = text.starts_with("--");
    let body = if is_long { &text[2..] } else { &text[1..] };
    match body.split_once('=') {
        Some((name, value)) => FlagNode {
            name: name.to_string(),
            value: Some(value.to_string()),
            is_long,
            range,
        },
        None => FlagNode {
            name: body.to_string(),
            value: None,
            is_long,
            range,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(ast: &CommandAst) -> &PipelineElement {
        ast.first().expect("first pipeline element")
    }

    #[test]
    fn empty_input_yields_empty_ast() {
        let ast = parse_simple("");
        assert!(!ast.has_command());
        assert!(ast.elements.is_empty());
    }

    #[test]
    fn whitespace_only_yields_empty_ast() {
        let ast = parse_simple("   \t  ");
        assert!(!ast.has_command());
    }

    #[test]
    fn simple_command_extracts_name_and_range() {
        let ast = parse_simple("ls");
        let cmd = first(&ast).command.as_ref().unwrap();
        assert_eq!(cmd.name, "ls");
        assert_eq!(cmd.range, Range::new(0, 2));
    }

    #[test]
    fn command_plus_subcommand() {
        let ast = parse_simple("git checkout");
        let el = first(&ast);
        assert_eq!(el.command.as_ref().unwrap().name, "git");
        let sub = el.subcommand.as_ref().unwrap();
        assert_eq!(sub.name, "checkout");
        assert_eq!(sub.depth, 0);
        assert_eq!(sub.range, Range::new(4, 12));
    }

    #[test]
    fn long_and_short_flags_classified() {
        let ast = parse_simple("ls -la --color=auto");
        let el = first(&ast);
        assert_eq!(el.flags.len(), 2);
        let la = &el.flags[0];
        assert_eq!(la.name, "la");
        assert!(!la.is_long);
        assert_eq!(la.value, None);

        let color = &el.flags[1];
        assert_eq!(color.name, "color");
        assert!(color.is_long);
        assert_eq!(color.value.as_deref(), Some("auto"));
    }

    #[test]
    fn positional_follows_subcommand() {
        let ast = parse_simple("git checkout main");
        let el = first(&ast);
        assert_eq!(el.positionals.len(), 1);
        let p = &el.positionals[0];
        assert_eq!(p.value, "main");
        assert!(matches!(p.kind, ArgKind::Unknown));
        assert_eq!(p.range, Range::new(13, 17));
    }

    #[test]
    fn quoted_string_kept_as_single_token() {
        let ast = parse_simple(r#"echo "hello world""#);
        let el = first(&ast);
        let quoted = el
            .subcommand
            .as_ref()
            .map(|s| s.name.as_str())
            .or_else(|| el.positionals.first().map(|p| p.value.as_str()))
            .unwrap();
        assert_eq!(quoted, "\"hello world\"");
    }

    #[test]
    fn single_quoted_strings_are_preserved() {
        let ast = parse_simple("echo 'it works'");
        let el = first(&ast);
        let text = el
            .subcommand
            .as_ref()
            .map(|s| s.name.as_str())
            .or_else(|| el.positionals.first().map(|p| p.value.as_str()))
            .unwrap();
        assert_eq!(text, "'it works'");
    }

    #[test]
    fn ranges_cover_each_token_exactly() {
        let input = "git  push    origin";
        let ast = parse_simple(input);
        let el = first(&ast);
        let cmd = el.command.as_ref().unwrap();
        assert_eq!(&input[cmd.range.start..cmd.range.end], "git");
        let sub = el.subcommand.as_ref().unwrap();
        assert_eq!(&input[sub.range.start..sub.range.end], "push");
        let positional = &el.positionals[0];
        assert_eq!(
            &input[positional.range.start..positional.range.end],
            "origin"
        );
    }

    #[test]
    fn lone_dash_is_not_a_flag() {
        let ast = parse_simple("cat -");
        let el = first(&ast);
        assert!(el.flags.is_empty());
        let sub = el.subcommand.as_ref().unwrap();
        assert_eq!(sub.name, "-");
    }

    #[test]
    fn depth_reports_subcommand_chain_length() {
        assert_eq!(parse_simple("").depth(), 0);
        assert_eq!(parse_simple("ls").depth(), 0);
        assert_eq!(parse_simple("git push").depth(), 1);
    }

    #[test]
    fn positional_at_finds_the_right_node() {
        let ast = parse_simple("git checkout main");
        let at = ast.positional_at(14).unwrap();
        assert_eq!(at.value, "main");
    }

    #[test]
    fn pipe_splits_into_two_elements() {
        let input = "ls | grep foo";
        let ast = parse_simple(input);
        assert_eq!(ast.stage_count(), 2);
        assert!(ast.has_pipeline());
        let first = &ast.elements[0];
        assert_eq!(first.command.as_ref().unwrap().name, "ls");
        assert!(first.separator.is_none());
        let second = &ast.elements[1];
        let sep = second.separator.expect("pipe separator");
        assert_eq!(sep.kind, SeparatorKind::Pipe);
        assert_eq!(&input[sep.range.start..sep.range.end], "|");
        assert_eq!(second.command.as_ref().unwrap().name, "grep");
    }

    #[test]
    fn three_stage_pipeline_preserves_order() {
        let input = "ls | where size > 1mb | select name";
        let ast = parse_simple(input);
        assert_eq!(ast.stage_count(), 3);
        let names: Vec<&str> = ast
            .elements
            .iter()
            .map(|e| e.command.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, vec!["ls", "where", "select"]);
    }

    #[test]
    fn double_pipe_is_not_two_pipes() {
        // `||` is a logical OR — the fallback keeps it as one token
        // and doesn't split the element.
        let ast = parse_simple("cmd1 || cmd2");
        assert_eq!(ast.stage_count(), 1);
    }

    #[test]
    fn positional_at_searches_across_stages() {
        // Stage 2's `grep foo bar`: `foo` becomes subcommand, `bar`
        // lands as a positional. positional_at reaches across stages.
        let input = "ls | grep foo bar";
        let ast = parse_simple(input);
        let bar = ast.positional_at(14).expect("positional over stage 2");
        assert_eq!(bar.value, "bar");
    }
}
