//! Bash-grammar-driven parser.
//!
//! Uses the `tree-sitter-bash` grammar to walk the input into a
//! semantic [`CommandAst`]. Detects `pipeline` nodes and emits one
//! [`PipelineElement`] per stage, attaching a [`Separator`] with
//! [`SeparatorKind::Pipe`] to every element after the first.
//!
//! Falls back to [`crate::parse::parse_simple`] when the grammar
//! returns a real `ERROR` node; MISSING tokens (common mid-typing)
//! are tolerated and the partial tree is consumed.

use tree_sitter::{Language, Node, Parser, Tree};

use crate::ast::{
    ArgKind, CommandAst, CommandNode, FlagNode, PipelineElement, PositionalNode, Range, Separator,
    SeparatorKind, SubcommandNode,
};

/// Parse a bash command line with the tree-sitter-bash grammar.
pub fn parse_bash(input: &str) -> CommandAst {
    if input.trim().is_empty() {
        return CommandAst::empty();
    }
    let mut parser = Parser::new();
    let lang: Language = tree_sitter_bash::LANGUAGE.into();
    if parser.set_language(&lang).is_err() {
        return crate::parse::parse_simple(input);
    }
    let Some(tree) = parser.parse(input, None) else {
        return crate::parse::parse_simple(input);
    };
    if has_real_error(tree.root_node()) {
        return crate::parse::parse_simple(input);
    }
    extract_ast(&tree, input).unwrap_or_else(|| crate::parse::parse_simple(input))
}

fn extract_ast(tree: &Tree, source: &str) -> Option<CommandAst> {
    let root = tree.root_node();
    if let Some(pipeline_node) = find_first_child(root, "pipeline") {
        return Some(ast_from_pipeline(pipeline_node, source));
    }
    let command_node = find_first_child(root, "command")?;
    let element = extract_command_element(command_node, source);
    Some(CommandAst::from_element(element))
}

fn ast_from_pipeline(pipeline: Node<'_>, source: &str) -> CommandAst {
    let mut elements: Vec<PipelineElement> = Vec::new();
    let mut pending_separator: Option<Separator> = None;
    let mut cursor = pipeline.walk();
    for child in pipeline.children(&mut cursor) {
        match child.kind() {
            "|" | "|&" => {
                pending_separator = Some(Separator {
                    kind: SeparatorKind::Pipe,
                    range: Range::new(child.start_byte(), child.end_byte()),
                });
            }
            "command" => {
                let mut element = extract_command_element(child, source);
                element.separator = pending_separator.take();
                elements.push(element);
            }
            _ => {}
        }
    }
    CommandAst {
        elements,
        errors: Vec::new(),
    }
}

fn extract_command_element(command_node: Node<'_>, source: &str) -> PipelineElement {
    let mut element = PipelineElement::empty();
    let mut cursor = command_node.walk();

    if let Some(name_node) = command_node.child_by_field_name("name") {
        let range = Range::new(name_node.start_byte(), name_node.end_byte());
        element.command = Some(CommandNode {
            name: source[range.start..range.end].to_string(),
            range,
        });
    }

    let mut saw_subcommand = false;
    for child in command_node.children(&mut cursor) {
        match child.kind() {
            "command_name" => {}
            "word" | "string" | "raw_string" => {
                let range = Range::new(child.start_byte(), child.end_byte());
                let text = source[range.start..range.end].to_string();
                if element.command.is_some() && !saw_subcommand && !text.starts_with('-') {
                    element.subcommand = Some(SubcommandNode {
                        name: text,
                        depth: 0,
                        range,
                    });
                    saw_subcommand = true;
                } else if text.starts_with('-') {
                    element.flags.push(flag_from_word(&text, range));
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

fn flag_from_word(text: &str, range: Range) -> FlagNode {
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
        let ast = parse_bash("");
        assert!(!ast.has_command());
    }

    #[test]
    fn simple_command_parses() {
        let ast = parse_bash("ls");
        assert_eq!(first(&ast).command.as_ref().unwrap().name, "ls");
    }

    #[test]
    fn command_with_subcommand_parses() {
        let ast = parse_bash("git checkout main");
        let el = first(&ast);
        assert_eq!(el.command.as_ref().unwrap().name, "git");
        assert_eq!(
            el.subcommand.as_ref().map(|s| s.name.as_str()),
            Some("checkout"),
        );
        assert_eq!(el.positionals.len(), 1);
        assert_eq!(el.positionals[0].value, "main");
    }

    #[test]
    fn short_and_long_flags_classified() {
        let ast = parse_bash("ls -la --color=auto");
        let el = first(&ast);
        assert!(el.flags.iter().any(|f| f.name == "la" && !f.is_long));
        assert!(
            el.flags
                .iter()
                .any(|f| f.name == "color" && f.is_long && f.value.as_deref() == Some("auto"))
        );
    }

    #[test]
    fn malformed_input_falls_back_to_simple_parser() {
        let ast = parse_bash("echo \"unterminated");
        assert!(ast.has_command());
    }

    #[test]
    fn byte_ranges_slice_back_to_tokens() {
        let input = "git push origin";
        let ast = parse_bash(input);
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
    fn pipeline_splits_stages() {
        let input = "ls -la | grep foo";
        let ast = parse_bash(input);
        assert_eq!(ast.stage_count(), 2);
        let first = &ast.elements[0];
        assert_eq!(first.command.as_ref().unwrap().name, "ls");
        assert!(first.separator.is_none());
        let second = &ast.elements[1];
        assert_eq!(second.command.as_ref().unwrap().name, "grep");
        let sep = second.separator.expect("pipe");
        assert_eq!(sep.kind, SeparatorKind::Pipe);
        assert_eq!(&input[sep.range.start..sep.range.end], "|");
    }

    #[test]
    fn three_stage_pipeline_preserves_names() {
        let input = "cat file.log | grep error | wc -l";
        let ast = parse_bash(input);
        assert_eq!(ast.stage_count(), 3);
        let names: Vec<&str> = ast
            .elements
            .iter()
            .map(|e| e.command.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, vec!["cat", "grep", "wc"]);
    }
}
