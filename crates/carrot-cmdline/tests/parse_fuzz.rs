//! Property-based fuzz tests for the fallback parser.
//!
//! The hand-written tests cover expected shapes; these cover the
//! invariants that must hold for *every* input the cmdline might
//! see — ASCII pranks, unicode, empty tokens, ragged whitespace.

use carrot_cmdline::{
    ast::{CommandAst, Range},
    parse::parse_simple,
};
use proptest::prelude::*;

/// Sum of all reported byte ranges must cover exactly the bytes the
/// parser classified — ranges never overlap, never point past the
/// input, and every range actually slices valid UTF-8.
fn validate_ranges(input: &str, ast: &CommandAst) {
    let len = input.len();
    let mut ranges: Vec<Range> = Vec::new();

    for element in &ast.elements {
        if let Some(sep) = &element.separator {
            ranges.push(sep.range);
        }
        if let Some(c) = &element.command {
            ranges.push(c.range);
        }
        if let Some(s) = &element.subcommand {
            ranges.push(s.range);
        }
        for f in &element.flags {
            ranges.push(f.range);
        }
        for p in &element.positionals {
            ranges.push(p.range);
        }
    }

    for r in &ranges {
        assert!(r.start <= r.end, "range out of order in {input:?}");
        assert!(r.end <= len, "range past input length in {input:?}");
        assert!(
            input.is_char_boundary(r.start),
            "range start not at char boundary: {input:?} range={r:?}"
        );
        assert!(
            input.is_char_boundary(r.end),
            "range end not at char boundary: {input:?} range={r:?}"
        );
    }

    ranges.sort_by_key(|r| (r.start, r.end));
    for win in ranges.windows(2) {
        let a = win[0];
        let b = win[1];
        assert!(
            a.end <= b.start,
            "overlapping ranges in {input:?}: {a:?} vs {b:?}"
        );
    }
}

/// Every node's stored text must equal the slice of the input at
/// its reported range.
fn validate_text_matches_range(input: &str, ast: &CommandAst) {
    for element in &ast.elements {
        if let Some(c) = &element.command {
            assert_eq!(&input[c.range.start..c.range.end], c.name);
        }
        if let Some(s) = &element.subcommand {
            assert_eq!(&input[s.range.start..s.range.end], s.name);
        }
        for p in &element.positionals {
            assert_eq!(&input[p.range.start..p.range.end], p.value);
        }
        for f in &element.flags {
            let slice = &input[f.range.start..f.range.end];
            assert!(slice.starts_with('-'));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    // Arbitrary ASCII-ish strings with newlines, tabs, quotes, dashes.
    #[test]
    fn parse_does_not_panic(input in "[ \\t\\n\"'\\-a-zA-Z0-9/.=]{0,128}") {
        let _ast = parse_simple(&input);
    }

    #[test]
    fn ranges_stay_in_bounds(input in "[ \\t\\n\"'\\-a-zA-Z0-9/.=]{0,128}") {
        let ast = parse_simple(&input);
        validate_ranges(&input, &ast);
    }

    #[test]
    fn ranges_point_at_stored_text(input in "[ \\t\\n\"'\\-a-zA-Z0-9/.=]{0,128}") {
        let ast = parse_simple(&input);
        validate_text_matches_range(&input, &ast);
    }

    #[test]
    fn unicode_inputs_do_not_panic(input in "\\PC{0,64}") {
        let ast = parse_simple(&input);
        validate_ranges(&input, &ast);
    }

    #[test]
    fn empty_string_always_empty_ast(_dummy in "x") {
        let ast = parse_simple("");
        prop_assert!(!ast.has_command());
        prop_assert!(ast.elements.is_empty());
    }

    #[test]
    fn whitespace_only_never_produces_command(spaces in "[ \\t\\n]{1,32}") {
        let ast = parse_simple(&spaces);
        prop_assert!(!ast.has_command());
    }

    #[test]
    fn token_count_roughly_matches_split_whitespace(input in "[ a-z]{1,64}") {
        let ast = parse_simple(&input);
        let tokens = input.split_ascii_whitespace().count();
        // Sum of (command + subcommand + positionals) across all
        // pipeline stages should equal the non-flag token count when
        // the input has no flags or pipes.
        let counted: usize = ast.elements.iter().map(|e| {
            e.command.is_some() as usize
                + e.subcommand.is_some() as usize
                + e.positionals.len()
        }).sum();
        prop_assert_eq!(counted, tokens);
    }
}
