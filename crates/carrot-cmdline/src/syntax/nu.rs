//! Nushell-grammar-driven parser.
//!
//! Uses the `tree-sitter-nu` grammar (nushell/tree-sitter-nu). The
//! grammar wraps one or more stages in a `pipeline` node whose
//! children are `pipe_element`s separated by `|` tokens. Each
//! `pipe_element` contains the actual `command` (or other
//! expression) plus optional redirects.
//!
//! tree-sitter-nu is the upstream syntax grammar; `nu-parser` is
//! Nushell's in-process parser but it pulls the full runtime
//! (`nu-engine`, `nu-protocol`, …). carrot-cmdline only needs the
//! syntactic shape (command, flags, positionals, pipeline stages)
//! for highlighting, completion, and agent handoff. Semantic Nu
//! features (typed table columns, pipeline value shapes) flow in
//! through `carrot-completions` specs and the block-header renderer.

use tree_sitter::{Language, Node, Parser, Tree};

use crate::ast::{
    ArgKind, CommandAst, CommandNode, FlagNode, PipelineElement, PositionalNode, Range, Separator,
    SeparatorKind, SubcommandNode,
};

/// Parse a nushell command line with the tree-sitter-nu grammar.
pub fn parse_nu(input: &str) -> CommandAst {
    if input.trim().is_empty() {
        return CommandAst::empty();
    }
    let mut parser = Parser::new();
    let lang: Language = tree_sitter_nu::LANGUAGE.into();
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
        let ast = ast_from_pipeline(pipeline_node, source);
        if !ast.elements.is_empty() {
            return Some(ast);
        }
    }
    // Nu wraps even single-command inputs in a pipeline node, so the
    // path above is the common case. If we somehow got here the
    // input has no pipeline — try a bare command.
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
            "|" => {
                pending_separator = Some(Separator {
                    kind: SeparatorKind::Pipe,
                    range: Range::new(child.start_byte(), child.end_byte()),
                });
            }
            "pipe_element" => {
                if let Some(command_node) = first_named_child(child, "command") {
                    let mut element = extract_command_element(command_node, source);
                    element.separator = pending_separator.take();
                    elements.push(element);
                }
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

    // `head` field carries the command identifier. Nu allows `^cmd`
    // to force external execution; skip the `^` marker and pick the
    // identifier sibling.
    if let Some(head_node) = command_node.child_by_field_name("head") {
        let name_node = if head_node.is_named() {
            head_node
        } else {
            first_named_child_any(head_node).unwrap_or(head_node)
        };
        let range = Range::new(name_node.start_byte(), name_node.end_byte());
        let text = source[range.start..range.end].to_string();
        if text != "^" {
            element.command = Some(CommandNode { name: text, range });
        }
    }
    if element.command.is_none() {
        let mut cursor = command_node.walk();
        for child in command_node.children(&mut cursor) {
            if child.kind() == "cmd_identifier" {
                let range = Range::new(child.start_byte(), child.end_byte());
                element.command = Some(CommandNode {
                    name: source[range.start..range.end].to_string(),
                    range,
                });
                break;
            }
        }
    }

    let mut saw_subcommand = false;
    let mut cursor = command_node.walk();
    for child in command_node.children(&mut cursor) {
        match child.kind() {
            "long_flag" => {
                element.flags.push(flag_from_nu_node(child, source, true));
            }
            "short_flag" => {
                element.flags.push(flag_from_nu_node(child, source, false));
            }
            "val_string" | "val_number" | "val_variable" | "val_interpolated" | "val_filesize"
            | "val_duration" | "val_bool" | "val_date" | "val_list" | "val_record"
            | "val_table" | "val_range" => {
                let range = Range::new(child.start_byte(), child.end_byte());
                let text = source[range.start..range.end].to_string();
                let unquoted = strip_nu_quotes(&text);
                if element.command.is_some() && !saw_subcommand && !unquoted.starts_with('-') {
                    element.subcommand = Some(SubcommandNode {
                        name: unquoted.to_string(),
                        depth: 0,
                        range,
                    });
                    saw_subcommand = true;
                } else {
                    element.positionals.push(PositionalNode {
                        value: unquoted.to_string(),
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

fn flag_from_nu_node(node: Node<'_>, source: &str, is_long: bool) -> FlagNode {
    let range = Range::new(node.start_byte(), node.end_byte());
    let name = node
        .child_by_field_name("name")
        .map(|n| source[n.start_byte()..n.end_byte()].to_string())
        .unwrap_or_default();
    let value = node
        .child_by_field_name("value")
        .map(|n| strip_nu_quotes(&source[n.start_byte()..n.end_byte()]).to_string());
    FlagNode {
        name,
        value,
        is_long,
        range,
    }
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

fn first_named_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn first_named_child_any(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn strip_nu_quotes(raw: &str) -> &str {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn first(ast: &CommandAst) -> &PipelineElement {
        ast.first().expect("first pipeline element")
    }

    #[test]
    fn empty_input_yields_empty_ast() {
        let ast = parse_nu("");
        assert!(!ast.has_command());
    }

    #[test]
    fn simple_command_parses() {
        let ast = parse_nu("ls");
        assert_eq!(first(&ast).command.as_ref().unwrap().name, "ls");
    }

    #[test]
    fn long_flag_with_value() {
        let ast = parse_nu("ls --long");
        let el = first(&ast);
        assert!(el.flags.iter().any(|f| f.name == "long" && f.is_long));
    }

    #[test]
    fn short_flag_parses() {
        let ast = parse_nu("ls -a");
        let el = first(&ast);
        assert!(el.flags.iter().any(|f| f.name == "a" && !f.is_long));
    }

    #[test]
    fn positional_string_parses() {
        let ast = parse_nu("open \"notes.md\"");
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
        let ast = parse_nu("echo \"unterminated");
        assert!(ast.has_command());
    }

    #[test]
    fn pipeline_splits_stages() {
        // Three plain stages — avoids the `where size > 1mb` literal
        // whose comparison operator shape differs from our test focus.
        let ast = parse_nu("ls | sort-by name | first 10");
        assert_eq!(ast.stage_count(), 3);
        let names: Vec<&str> = ast
            .elements
            .iter()
            .map(|e| e.command.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, vec!["ls", "sort-by", "first"]);
        assert!(ast.elements[0].separator.is_none());
        assert_eq!(
            ast.elements[1].separator.map(|s| s.kind),
            Some(SeparatorKind::Pipe),
        );
        assert_eq!(
            ast.elements[2].separator.map(|s| s.kind),
            Some(SeparatorKind::Pipe),
        );
    }
}
