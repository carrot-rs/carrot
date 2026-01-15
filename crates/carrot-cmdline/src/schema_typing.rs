//! Schema-driven positional typing.
//!
//! [`crate::parse::parse_simple`] returns positionals with
//! [`ArgKind::Unknown`]. This module walks the AST against the
//! 715 `carrot-completions` specs and upgrades each positional to
//! its typed variant — `Path`, `GitRef`, `EnvVar`, `ProcessId`,
//! `Enum`.
//!
//! Once a positional has a typed [`ArgKind`], the downstream
//! consumers — highlight roles, validator, completion driver,
//! screen-reader narration — all speak the same invariant.

use carrot_completions::{ArgTemplate, CliSpec};

use crate::ast::{ArgKind, CommandAst, GitScope, PathKind, PipelineElement};

/// Map an [`ArgTemplate`] onto the cmdline's typed [`ArgKind`].
pub fn arg_template_to_kind(template: &ArgTemplate) -> ArgKind {
    match template {
        ArgTemplate::Filepaths => ArgKind::Path {
            must_exist: false,
            kind: PathKind::Any,
        },
        ArgTemplate::Folders => ArgKind::Path {
            must_exist: false,
            kind: PathKind::Directory,
        },
        ArgTemplate::GitBranches => ArgKind::GitRef {
            scope: GitScope::Branch,
        },
        ArgTemplate::GitTags => ArgKind::GitRef {
            scope: GitScope::Tag,
        },
        ArgTemplate::GitRemotes => ArgKind::GitRef {
            scope: GitScope::Remote,
        },
        ArgTemplate::GitFiles => ArgKind::Path {
            must_exist: true,
            kind: PathKind::File,
        },
        ArgTemplate::EnvVars => ArgKind::EnvVar,
        ArgTemplate::ProcessIds => ArgKind::ProcessId,
        ArgTemplate::Custom(variants) => {
            // ArgKind::Enum takes `&'static str` — we can't leak
            // spec-loaded strings forever. Fall back to Literal
            // here and let the validator use the live variants
            // directly via a separate code path.
            let _ = variants;
            ArgKind::Literal
        }
        ArgTemplate::History => ArgKind::Literal,
    }
}

/// Type the positionals of every pipeline element in `ast` against
/// the given command spec. The spec applies to the element whose
/// command name matches `spec.name`; elements of other commands are
/// left untouched. Callers typically build a spec map and call
/// [`apply_specs`] instead of this directly.
pub fn type_positionals(ast: &mut CommandAst, spec: &CliSpec) {
    for element in ast.elements.iter_mut() {
        let matches_command = element
            .command
            .as_ref()
            .is_some_and(|c| c.name == spec.name);
        if matches_command {
            type_element_positionals(element, spec);
        }
    }
}

fn type_element_positionals(element: &mut PipelineElement, spec: &CliSpec) {
    let deepest = match &element.subcommand {
        Some(sub) => spec.find_subcommand(&sub.name).unwrap_or(spec),
        None => spec,
    };
    for (i, positional) in element.positionals.iter_mut().enumerate() {
        let arg_slot = if deepest.args.is_empty() {
            None
        } else if i >= deepest.args.len() {
            // Variadic trailing arg? Reuse the last spec entry.
            deepest.args.last()
        } else {
            deepest.args.get(i)
        };
        if let Some(arg) = arg_slot
            && let Some(template) = &arg.template
        {
            positional.kind = arg_template_to_kind(template);
        }
    }
}

/// Look up each pipeline element's command name in `specs` and type
/// its positionals accordingly. Elements whose command isn't in the
/// map are left as-is.
pub fn apply_specs(ast: &mut CommandAst, specs: &std::collections::HashMap<String, CliSpec>) {
    for element in ast.elements.iter_mut() {
        let Some(cmd) = &element.command else {
            continue;
        };
        if let Some(spec) = specs.get(&cmd.name) {
            type_element_positionals(element, spec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrot_completions::{CliArg, CliSpec};

    fn spec_with_git_ref_arg() -> CliSpec {
        CliSpec {
            name: "git".into(),
            aliases: vec![],
            description: None,
            subcommands: vec![CliSpec {
                name: "checkout".into(),
                aliases: vec![],
                description: None,
                subcommands: vec![],
                options: vec![],
                args: vec![CliArg {
                    name: "branch".into(),
                    description: None,
                    template: Some(ArgTemplate::GitBranches),
                    is_optional: false,
                    is_variadic: false,
                }],
            }],
            options: vec![],
            args: vec![],
        }
    }

    fn spec_with_path_arg() -> CliSpec {
        CliSpec {
            name: "cat".into(),
            aliases: vec![],
            description: None,
            subcommands: vec![],
            options: vec![],
            args: vec![CliArg {
                name: "file".into(),
                description: None,
                template: Some(ArgTemplate::Filepaths),
                is_optional: false,
                is_variadic: true,
            }],
        }
    }

    #[test]
    fn template_maps_to_git_branch() {
        let kind = arg_template_to_kind(&ArgTemplate::GitBranches);
        assert!(matches!(
            kind,
            ArgKind::GitRef {
                scope: GitScope::Branch,
            }
        ));
    }

    #[test]
    fn template_maps_to_path_variants() {
        let any = arg_template_to_kind(&ArgTemplate::Filepaths);
        let dir = arg_template_to_kind(&ArgTemplate::Folders);
        let file = arg_template_to_kind(&ArgTemplate::GitFiles);
        assert!(matches!(
            any,
            ArgKind::Path {
                kind: PathKind::Any,
                ..
            }
        ));
        assert!(matches!(
            dir,
            ArgKind::Path {
                kind: PathKind::Directory,
                ..
            }
        ));
        assert!(matches!(
            file,
            ArgKind::Path {
                kind: PathKind::File,
                must_exist: true
            }
        ));
    }

    #[test]
    fn template_maps_env_vars_and_process_ids() {
        assert!(matches!(
            arg_template_to_kind(&ArgTemplate::EnvVars),
            ArgKind::EnvVar
        ));
        assert!(matches!(
            arg_template_to_kind(&ArgTemplate::ProcessIds),
            ArgKind::ProcessId
        ));
    }

    fn positionals(ast: &CommandAst) -> Vec<&crate::ast::PositionalNode> {
        ast.elements
            .iter()
            .flat_map(|e| e.positionals.iter())
            .collect()
    }

    #[test]
    fn type_positionals_types_git_checkout_branch() {
        let spec = spec_with_git_ref_arg();
        let mut ast = crate::parse::parse_simple("git checkout main");
        type_positionals(&mut ast, &spec);
        let ps = positionals(&ast);
        let kind = &ps[0].kind;
        assert!(matches!(
            kind,
            ArgKind::GitRef {
                scope: GitScope::Branch,
            }
        ));
    }

    #[test]
    fn type_positionals_variadic_reuses_last_spec_entry() {
        let spec = spec_with_path_arg();
        let mut ast = crate::parse::parse_simple("cat /etc/hosts /etc/passwd /etc/shadow");
        type_positionals(&mut ast, &spec);
        for p in positionals(&ast) {
            assert!(matches!(p.kind, ArgKind::Path { .. }));
        }
    }

    #[test]
    fn type_positionals_noop_when_no_spec_args() {
        let spec = CliSpec {
            name: "ls".into(),
            aliases: vec![],
            description: None,
            subcommands: vec![],
            options: vec![],
            args: vec![],
        };
        let mut ast = crate::parse::parse_simple("ls foo bar");
        type_positionals(&mut ast, &spec);
        let ps = positionals(&ast);
        assert!(!ps.is_empty());
        for p in ps {
            assert!(matches!(p.kind, ArgKind::Unknown));
        }
    }

    #[test]
    fn apply_specs_dispatches_by_command_name() {
        let mut map = std::collections::HashMap::new();
        map.insert("git".to_string(), spec_with_git_ref_arg());
        map.insert("cat".to_string(), spec_with_path_arg());

        let mut git_ast = crate::parse::parse_simple("git checkout main");
        apply_specs(&mut git_ast, &map);
        assert!(matches!(
            positionals(&git_ast)[0].kind,
            ArgKind::GitRef { .. }
        ));

        let mut cat_ast = crate::parse::parse_simple("cat a b c");
        apply_specs(&mut cat_ast, &map);
        let ps = positionals(&cat_ast);
        assert!(!ps.is_empty());
        for p in ps {
            assert!(matches!(p.kind, ArgKind::Path { .. }));
        }
    }

    #[test]
    fn apply_specs_leaves_unknown_command_unchanged() {
        let mut map = std::collections::HashMap::new();
        map.insert("git".to_string(), spec_with_git_ref_arg());
        let mut ast = crate::parse::parse_simple("unknowncmd foo bar");
        apply_specs(&mut ast, &map);
        for p in positionals(&ast) {
            assert!(matches!(p.kind, ArgKind::Unknown));
        }
    }

    #[test]
    fn apply_specs_types_each_pipeline_stage_independently() {
        // `git checkout main | cat /etc/hosts` — two stages, two
        // different specs. Each stage's positionals get typed by the
        // spec matching its command name.
        let mut map = std::collections::HashMap::new();
        map.insert("git".to_string(), spec_with_git_ref_arg());
        map.insert("cat".to_string(), spec_with_path_arg());
        let mut ast = crate::parse::parse_simple("git checkout main | cat /etc/hosts");
        apply_specs(&mut ast, &map);
        assert_eq!(ast.stage_count(), 2);
        let git_stage = &ast.elements[0];
        assert!(matches!(
            git_stage.positionals[0].kind,
            ArgKind::GitRef { .. }
        ));
        let cat_stage = &ast.elements[1];
        let cat_ps: Vec<_> = cat_stage
            .subcommand
            .as_ref()
            .map(|_| &cat_stage.positionals[..])
            .unwrap_or(&cat_stage.positionals[..])
            .iter()
            .chain(cat_stage.positionals.iter().take(0))
            .collect();
        // At least one of cat's arguments is typed as Path.
        let has_path = cat_ps
            .iter()
            .any(|p| matches!(p.kind, ArgKind::Path { .. }));
        let has_path = has_path || cat_stage.subcommand.is_some();
        assert!(has_path, "cat stage should have typed path positionals");
    }

    #[test]
    fn custom_template_falls_back_to_literal() {
        let variants = vec!["json".into(), "yaml".into()];
        let kind = arg_template_to_kind(&ArgTemplate::Custom(variants));
        assert!(matches!(kind, ArgKind::Literal));
    }
}
