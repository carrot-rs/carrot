//! Fish-grammar-driven parser.
//!
//! Uses the `tree-sitter-fish` grammar. Fish 4.x ships its own Rust
//! parser inside the main fish-shell repository, but it isn't
//! published as a standalone crate — the whole runtime (executor,
//! builtins, autoload cache) ships together. tree-sitter-fish gives
//! us exactly the syntactic layer carrot-cmdline needs (command,
//! arguments, flags, strings, redirects, pipes).
//!
//! tree-sitter-fish names the pipeline wrapper `pipe` (not
//! `pipeline` like bash/zsh); the walker handles that spelling
//! difference and otherwise mirrors the other shells.

use tree_sitter::{Language, Node, Parser, Tree};

use crate::ast::{
    ArgKind, CommandAst, CommandNode, FlagNode, PipelineElement, PositionalNode, Range, Separator,
    SeparatorKind, SubcommandNode,
};

/// Parse a fish command line with the tree-sitter-fish grammar.
pub fn parse_fish(input: &str) -> CommandAst {
    if input.trim().is_empty() {
        return CommandAst::empty();
    }
    let mut parser = Parser::new();
    let lang: Language = tree_sitter_fish::language();
    if parser.set_language(&lang).is_err() {
        return crate::parse::parse_simple(input);
    }
    let Some(tree) = parser.parse(input, None) else {
        return crate::parse::parse_simple(input);
    };
    // tree-sitter-fish reports MISSING ";" at end-of-input when the
    // line has no explicit terminator — that's expected for a live
    // command line, so we only fall back on real ERROR nodes.
    if has_real_error(tree.root_node()) {
        return crate::parse::parse_simple(input);
    }
    extract_ast(&tree, input).unwrap_or_else(|| crate::parse::parse_simple(input))
}

fn extract_ast(tree: &Tree, source: &str) -> Option<CommandAst> {
    let root = tree.root_node();
    if let Some(pipe_node) = find_first_child(root, "pipe") {
        return Some(ast_from_pipe(pipe_node, source));
    }
    let command_node = find_first_child(root, "command")?;
    let element = extract_command_element(command_node, source);
    Some(CommandAst::from_element(element))
}

fn ast_from_pipe(pipe_node: Node<'_>, source: &str) -> CommandAst {
    let mut elements: Vec<PipelineElement> = Vec::new();
    let mut pending_separator: Option<Separator> = None;
    collect_pipe_children(pipe_node, source, &mut elements, &mut pending_separator);
    CommandAst {
        elements,
        errors: Vec::new(),
    }
}

/// tree-sitter-fish nests pipes left-associatively: `a | b | c` →
/// `pipe(pipe(a, b), c)`. This flattens into a linear element list.
fn collect_pipe_children(
    pipe_node: Node<'_>,
    source: &str,
    elements: &mut Vec<PipelineElement>,
    pending_separator: &mut Option<Separator>,
) {
    let mut cursor = pipe_node.walk();
    for child in pipe_node.children(&mut cursor) {
        match child.kind() {
            "|" | "|&" => {
                *pending_separator = Some(Separator {
                    kind: SeparatorKind::Pipe,
                    range: Range::new(child.start_byte(), child.end_byte()),
                });
            }
            "pipe" => {
                collect_pipe_children(child, source, elements, pending_separator);
            }
            "command" => {
                let mut element = extract_command_element(child, source);
                element.separator = pending_separator.take();
                elements.push(element);
            }
            _ => {}
        }
    }
}

fn extract_command_element(command_node: Node<'_>, source: &str) -> PipelineElement {
    let mut element = PipelineElement::empty();

    if let Some(name_node) = command_node.child_by_field_name("name") {
        let range = Range::new(name_node.start_byte(), name_node.end_byte());
        element.command = Some(CommandNode {
            name: strip_fish_quotes(&source[range.start..range.end]).to_string(),
            range,
        });
    }

    let mut saw_subcommand = false;
    let mut cursor = command_node.walk();
    for child in command_node.children_by_field_name("argument", &mut cursor) {
        match child.kind() {
            "word"
            | "double_quote_string"
            | "single_quote_string"
            | "integer"
            | "float"
            | "glob"
            | "home_dir_expansion"
            | "variable_expansion"
            | "concatenation"
            | "command_substitution"
            | "brace_expansion" => {
                let range = Range::new(child.start_byte(), child.end_byte());
                let raw = &source[range.start..range.end];
                let text = strip_fish_quotes(raw).to_string();
                if text.starts_with('-') && text.len() > 1 {
                    element.flags.push(flag_from_word(&text, range));
                } else if element.command.is_some() && !saw_subcommand {
                    element.subcommand = Some(SubcommandNode {
                        name: text,
                        depth: 0,
                        range,
                    });
                    saw_subcommand = true;
                } else {
                    element.positionals.push(PositionalNode {
                        value: text,
                        kind: ArgKind::Unknown,
                        range,
                    });
                }
            }
            _ => {}
        }
    }

    element
}

fn has_real_error(node: Node<'_>) -> bool {
    if node.is_error() {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_real_error(child) {
            return true;
        }
    }
    false
}

fn find_first_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_first_child(child, kind) {
            return Some(found);
        }
    }
    None
}

fn strip_fish_quotes(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn flag_from_word(text: &str, range: Range) -> FlagNode {
    // Fish convention: long flags are `--name`, short flags are
    // `-n`. Fish also accepts multi-letter short flags (`-abc`) —
    // we keep the raw body so completions can decide how to split.
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
        let ast = parse_fish("");
        assert!(!ast.has_command());
    }

    #[test]
    fn simple_command_parses() {
        let ast = parse_fish("ls");
        assert_eq!(first(&ast).command.as_ref().unwrap().name, "ls");
    }

    #[test]
    fn command_with_subcommand_parses() {
        let ast = parse_fish("git checkout main");
        let el = first(&ast);
        assert_eq!(el.command.as_ref().unwrap().name, "git");
        let sub = el.subcommand.as_ref().map(|s| s.name.as_str());
        assert_eq!(sub, Some("checkout"));
        assert!(el.positionals.iter().any(|p| p.value == "main"));
    }

    #[test]
    fn short_and_long_flags_classified() {
        let ast = parse_fish("ls -la --color=auto");
        let el = first(&ast);
        assert!(el.flags.iter().any(|f| f.name == "la" && !f.is_long));
        assert!(
            el.flags
                .iter()
                .any(|f| f.name == "color" && f.is_long && f.value.as_deref() == Some("auto"))
        );
    }

    #[test]
    fn string_argument_unquoted() {
        let ast = parse_fish("open 'notes.md'");
        let el = first(&ast);
        assert_eq!(el.command.as_ref().unwrap().name, "open");
        let values: Vec<&str> = el
            .subcommand
            .iter()
            .map(|s| s.name.as_str())
            .chain(el.positionals.iter().map(|p| p.value.as_str()))
            .collect();
        assert!(values.contains(&"notes.md"));
    }

    #[test]
    fn malformed_input_falls_back_to_simple_parser() {
        let ast = parse_fish("echo \"unterminated");
        assert!(ast.has_command());
    }

    #[test]
    fn pipeline_splits_stages() {
        let ast = parse_fish("ls | grep foo");
        assert_eq!(ast.stage_count(), 2);
        let names: Vec<&str> = ast
            .elements
            .iter()
            .map(|e| e.command.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, vec!["ls", "grep"]);
        assert_eq!(
            ast.elements[1].separator.map(|s| s.kind),
            Some(SeparatorKind::Pipe),
        );
    }

    #[test]
    fn three_stage_pipeline_preserves_order() {
        let input = "cat log | grep error | wc -l";
        let ast = parse_fish(input);
        assert_eq!(ast.stage_count(), 3);
        let names: Vec<&str> = ast
            .elements
            .iter()
            .map(|e| e.command.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, vec!["cat", "grep", "wc"]);
    }
}
