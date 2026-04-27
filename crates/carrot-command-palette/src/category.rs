//! Search categories — the buckets the command palette can filter by.
//!
//! Each category maps 1:1 to a chip in the modal header and to a `prefix:`
//! the user can type directly into the search field (e.g. `sessions: foo`,
//! `env: HOME`).
//!
//! `SearchCategory` is `Serialize`/`Deserialize` because it travels as the
//! `category_filter` field of the `ToggleWithFilter` keymap action.

use carrot_ui::{ColorName, IconName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Categories of items the panel can search over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SearchCategory {
    Workflows,
    Prompts,
    Notebooks,
    EnvironmentVariables,
    Files,
    Drive,
    Actions,
    Sessions,
    LaunchConfigurations,
    Conversations,
    History,
}

impl SearchCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workflows => "workflows",
            Self::Prompts => "prompts",
            Self::Notebooks => "notebooks",
            Self::EnvironmentVariables => "environment variables",
            Self::Files => "files",
            Self::Drive => "drive",
            Self::Actions => "actions",
            Self::Sessions => "sessions",
            Self::LaunchConfigurations => "launch configurations",
            Self::Conversations => "conversations",
            Self::History => "history",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Self::Workflows => IconName::Terminal,
            Self::Prompts => IconName::Chat,
            Self::Notebooks => IconName::FileTextOutlined,
            Self::EnvironmentVariables => IconName::Code,
            Self::Files => IconName::FileGeneric,
            Self::Drive => IconName::FileTree,
            Self::Actions => IconName::BoltFilled,
            Self::Sessions => IconName::Terminal,
            Self::LaunchConfigurations => IconName::Settings,
            Self::Conversations => IconName::Chat,
            Self::History => IconName::HistoryRerun,
        }
    }

    /// Distinct accent color for the icon. Categories without a strong
    /// semantic color fall back to the muted text color via `None`.
    pub fn icon_color(self) -> Option<ColorName> {
        match self {
            Self::Workflows => Some(ColorName::Red),
            Self::Notebooks => Some(ColorName::Blue),
            Self::EnvironmentVariables => Some(ColorName::Purple),
            Self::Actions => Some(ColorName::Amber),
            Self::Sessions => Some(ColorName::Cyan),
            Self::Conversations => Some(ColorName::Sky),
            Self::History => Some(ColorName::Green),
            Self::Prompts | Self::Files | Self::Drive | Self::LaunchConfigurations => None,
        }
    }

    /// Placeholder shown inside the search input when this category is the
    /// active filter. Empty queries display this so the user always sees
    /// what they're searching for.
    pub fn search_placeholder(self) -> &'static str {
        match self {
            Self::Workflows => "Search workflows",
            Self::Prompts => "Search prompts",
            Self::Notebooks => "Search notebooks",
            Self::EnvironmentVariables => "Search environment variables",
            Self::Files => "Search files",
            Self::Drive => "Search drive",
            Self::Actions => "Search actions",
            Self::Sessions => "Search sessions",
            Self::LaunchConfigurations => "Search launch configurations",
            Self::Conversations => "Search conversations",
            Self::History => "Search history",
        }
    }

    /// Prefix a user can type to restrict the search to this category, e.g.
    /// `sessions:` or `env:`. Matched case-sensitively on ASCII letters with
    /// a trailing colon.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Workflows => "workflows:",
            Self::Prompts => "prompts:",
            Self::Notebooks => "notebooks:",
            Self::EnvironmentVariables => "env:",
            Self::Files => "files:",
            Self::Drive => "drive:",
            Self::Actions => "actions:",
            Self::Sessions => "sessions:",
            Self::LaunchConfigurations => "launch:",
            Self::Conversations => "conversations:",
            Self::History => "history:",
        }
    }

    pub fn all() -> &'static [SearchCategory] {
        &[
            Self::Workflows,
            Self::Prompts,
            Self::Notebooks,
            Self::EnvironmentVariables,
            Self::Files,
            Self::Drive,
            Self::Actions,
            Self::Sessions,
            Self::LaunchConfigurations,
            Self::Conversations,
            Self::History,
        ]
    }
}

/// Short-form aliases — single-character prefixes that expand to their
/// full category when the user types them followed by `:`. Mirrors the
/// single-letter convention power-users already know from shell aliases.
const SHORT_FORMS: &[(&str, SearchCategory)] = &[
    ("h:", SearchCategory::History),
    ("f:", SearchCategory::Files),
    ("a:", SearchCategory::Actions),
    ("s:", SearchCategory::Sessions),
    ("p:", SearchCategory::Prompts),
    ("n:", SearchCategory::Notebooks),
    ("w:", SearchCategory::Workflows),
    ("e:", SearchCategory::EnvironmentVariables),
    ("d:", SearchCategory::Drive),
    ("l:", SearchCategory::LaunchConfigurations),
    ("c:", SearchCategory::Conversations),
];

/// Splits a typed filter prefix off the front of `raw` and returns the
/// remaining query text. `(Some(cat), rest)` means the user explicitly
/// filtered via e.g. `sessions: ` or the short form `s: `. `(None, rest)`
/// means the query is free text and the currently selected chip (if any)
/// decides the scope.
pub fn parse_filter_prefix(raw: &str) -> (Option<SearchCategory>, &str) {
    let trimmed = raw.trim_start();
    for &cat in SearchCategory::all() {
        if let Some(rest) = trimmed.strip_prefix(cat.prefix()) {
            return (Some(cat), rest.trim_start());
        }
    }
    for &(short, cat) in SHORT_FORMS {
        if let Some(rest) = trimmed.strip_prefix(short) {
            return (Some(cat), rest.trim_start());
        }
    }
    (None, trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_prefix() {
        let (cat, rest) = parse_filter_prefix("env: HOME");
        assert_eq!(cat, Some(SearchCategory::EnvironmentVariables));
        assert_eq!(rest, "HOME");
    }

    #[test]
    fn parses_sessions_prefix_no_space() {
        let (cat, rest) = parse_filter_prefix("sessions:foo");
        assert_eq!(cat, Some(SearchCategory::Sessions));
        assert_eq!(rest, "foo");
    }

    #[test]
    fn parses_history_prefix() {
        let (cat, rest) = parse_filter_prefix("history: git");
        assert_eq!(cat, Some(SearchCategory::History));
        assert_eq!(rest, "git");
    }

    #[test]
    fn parses_short_form_history() {
        let (cat, rest) = parse_filter_prefix("h: git");
        assert_eq!(cat, Some(SearchCategory::History));
        assert_eq!(rest, "git");
    }

    #[test]
    fn parses_short_form_files() {
        let (cat, rest) = parse_filter_prefix("f:readme");
        assert_eq!(cat, Some(SearchCategory::Files));
        assert_eq!(rest, "readme");
    }

    #[test]
    fn leaves_plain_query_untouched() {
        let (cat, rest) = parse_filter_prefix("some query");
        assert_eq!(cat, None);
        assert_eq!(rest, "some query");
    }

    #[test]
    fn trims_leading_whitespace() {
        let (cat, rest) = parse_filter_prefix("   hello");
        assert_eq!(cat, None);
        assert_eq!(rest, "hello");
    }
}
