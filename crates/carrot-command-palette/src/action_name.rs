//! Pure helpers for normalizing and humanizing action names.
//!
//! These were originally attached to the legacy `CommandPalette` modal but
//! live here now so external consumers (keymap editor, which-key) keep a
//! stable import path independent of the modal's UI.

/// Removes subsequent whitespace characters and double colons from the query.
///
/// This improves the likelihood of a match by either humanized name or
/// keymap-style name.
pub fn normalize_action_query(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_char = None;

    for char in input.trim().chars() {
        match (last_char, char) {
            (Some(':'), ':') => continue,
            (Some(last_char), char) if last_char.is_whitespace() && char.is_whitespace() => {
                continue;
            }
            _ => {
                last_char = Some(char);
            }
        }
        result.push(char);
    }

    result
}

/// Converts a snake/camel/namespace-style action name into a space-separated
/// lowercase label suitable for display.
pub fn humanize_action_name(name: &str) -> String {
    let capacity = name.len() + name.chars().filter(|c| c.is_uppercase()).count();
    let mut result = String::with_capacity(capacity);
    for char in name.chars() {
        if char == ':' {
            if result.ends_with(':') {
                result.push(' ');
            } else {
                result.push(':');
            }
        } else if char == '_' {
            result.push(' ');
        } else if char.is_uppercase() {
            if !result.ends_with(' ') {
                result.push(' ');
            }
            result.extend(char.to_lowercase());
        } else {
            result.push(char);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_humanize_action_name() {
        assert_eq!(
            humanize_action_name("editor::GoToDefinition"),
            "editor: go to definition"
        );
        assert_eq!(
            humanize_action_name("workspace::NewFile"),
            "workspace: new file"
        );
        assert_eq!(humanize_action_name("snake_case"), "snake case");
    }

    #[test]
    fn test_normalize_action_query() {
        assert_eq!(normalize_action_query("  editor::go  "), "editor:go");
        assert_eq!(normalize_action_query("go to  line"), "go to line");
    }
}
