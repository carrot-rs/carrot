//! AST → Editor highlight bridge.
//!
//! Rendering-side glue that takes the role-tagged [`HighlightSpan`]
//! vector produced by [`crate::highlight::highlight_ast`] and pushes
//! it into the backing [`carrot_editor::Editor`] via
//! `editor.highlight_text(HighlightKey::CommandAst(tier), ...)`.
//!
//! The cmdline **does not own its own rendering engine**. We rely on
//! the editor's existing text-highlight pipeline (anchors,
//! invalidation, painting) — see the `HighlightKey::CommandAst(tier)`
//! variant we added in `carrot-editor::display_map`.

use std::collections::HashMap;
use std::ops::Range;

use carrot_editor::{Anchor, Editor, display_map::HighlightKey};
use carrot_multi_buffer::MultiBufferOffset;
use inazuma::{Context, HighlightStyle};

use crate::highlight::{HighlightRole, HighlightSpan};

/// Total number of highlight tiers. Every [`HighlightRole`] maps to
/// a stable tier index inside [0, TIER_COUNT). Incrementing this
/// constant (and the role-mapping) is the extension point when new
/// roles appear.
pub const TIER_COUNT: usize = 14;

/// Deterministic mapping from [`HighlightRole`] to tier index.
/// Stable ordering — consumers persist per-tier theme overrides by
/// index.
pub const fn role_to_tier(role: HighlightRole) -> usize {
    match role {
        HighlightRole::Command => 0,
        HighlightRole::Subcommand => 1,
        HighlightRole::LongFlag => 2,
        HighlightRole::ShortFlag => 3,
        HighlightRole::FlagValue => 4,
        HighlightRole::Path => 5,
        HighlightRole::GitRef => 6,
        HighlightRole::Url => 7,
        HighlightRole::EnvVar => 8,
        HighlightRole::ProcessId => 9,
        HighlightRole::EnumLiteral => 10,
        HighlightRole::Positional => 11,
        HighlightRole::Separator => 12,
        HighlightRole::Error => 13,
    }
}

/// Palette describing how each [`HighlightRole`] should render.
/// Built by the theme layer (`carrot-theme`) and handed to
/// [`apply_ast_highlights`].
#[derive(Debug, Clone, Default)]
pub struct CmdlineHighlightPalette {
    pub command: HighlightStyle,
    pub subcommand: HighlightStyle,
    pub long_flag: HighlightStyle,
    pub short_flag: HighlightStyle,
    pub flag_value: HighlightStyle,
    pub path: HighlightStyle,
    pub git_ref: HighlightStyle,
    pub url: HighlightStyle,
    pub env_var: HighlightStyle,
    pub process_id: HighlightStyle,
    pub enum_literal: HighlightStyle,
    pub positional: HighlightStyle,
    pub separator: HighlightStyle,
    pub error: HighlightStyle,
}

impl CmdlineHighlightPalette {
    /// Look up the [`HighlightStyle`] for a role via its tier index.
    pub fn style_for(&self, role: HighlightRole) -> HighlightStyle {
        match role {
            HighlightRole::Command => self.command,
            HighlightRole::Subcommand => self.subcommand,
            HighlightRole::LongFlag => self.long_flag,
            HighlightRole::ShortFlag => self.short_flag,
            HighlightRole::FlagValue => self.flag_value,
            HighlightRole::Path => self.path,
            HighlightRole::GitRef => self.git_ref,
            HighlightRole::Url => self.url,
            HighlightRole::EnvVar => self.env_var,
            HighlightRole::ProcessId => self.process_id,
            HighlightRole::EnumLiteral => self.enum_literal,
            HighlightRole::Positional => self.positional,
            HighlightRole::Separator => self.separator,
            HighlightRole::Error => self.error,
        }
    }
}

/// Push the span list into the editor's highlight store. Previous
/// `HighlightKey::CommandAst(_)` entries are cleared before the new
/// set is installed, so each call is a full refresh — safe to call
/// on every keystroke.
///
/// Spans whose byte range doesn't slice cleanly inside the current
/// buffer are silently dropped (buffer mutated after the AST was
/// computed). The editor's anchor mapping handles the live case.
pub fn apply_ast_highlights(
    editor: &mut Editor,
    spans: &[HighlightSpan],
    palette: &CmdlineHighlightPalette,
    cx: &mut Context<Editor>,
) {
    // Drop stale tiers first. `clear_highlights` is cheap — it's a
    // `HashMap::remove`.
    for tier in 0..TIER_COUNT {
        editor.clear_highlights(HighlightKey::CommandAst(tier), cx);
    }
    if spans.is_empty() {
        return;
    }
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let buffer_len = snapshot.len().0;

    let mut by_role: HashMap<HighlightRole, Vec<Range<Anchor>>> = HashMap::new();
    for span in spans {
        if span.range.end > buffer_len {
            continue;
        }
        let anchor_start = snapshot.anchor_before(MultiBufferOffset(span.range.start));
        let anchor_end = snapshot.anchor_after(MultiBufferOffset(span.range.end));
        by_role
            .entry(span.role)
            .or_default()
            .push(anchor_start..anchor_end);
    }
    for (role, ranges) in by_role {
        let style = palette.style_for(role);
        editor.highlight_text(
            HighlightKey::CommandAst(role_to_tier(role)),
            ranges,
            style,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_to_tier_is_distinct_for_every_role() {
        let roles = [
            HighlightRole::Command,
            HighlightRole::Subcommand,
            HighlightRole::LongFlag,
            HighlightRole::ShortFlag,
            HighlightRole::FlagValue,
            HighlightRole::Path,
            HighlightRole::GitRef,
            HighlightRole::Url,
            HighlightRole::EnvVar,
            HighlightRole::ProcessId,
            HighlightRole::EnumLiteral,
            HighlightRole::Positional,
            HighlightRole::Separator,
            HighlightRole::Error,
        ];
        let mut seen = std::collections::HashSet::new();
        for r in roles {
            let tier = role_to_tier(r);
            assert!(tier < TIER_COUNT, "tier out of bounds for {r:?}");
            assert!(seen.insert(tier), "duplicate tier for {r:?}");
        }
        assert_eq!(seen.len(), TIER_COUNT);
    }

    #[test]
    fn palette_style_for_matches_role() {
        let mut palette = CmdlineHighlightPalette::default();
        palette.command = HighlightStyle {
            font_weight: Some(inazuma::FontWeight::BOLD),
            ..Default::default()
        };
        let style = palette.style_for(HighlightRole::Command);
        assert_eq!(style.font_weight, palette.command.font_weight);
    }
}
