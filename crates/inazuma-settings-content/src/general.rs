use inazuma_collections::HashMap;
use inazuma_settings_macros::{MergeFrom, with_fallible_options};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::terminal::{AlternateScroll, Shell, TuiAwareness};

/// Content for the `[general]` section in settings.toml.
///
/// In Carrot, the terminal IS the app — these are the primary settings,
/// not secondary "terminal panel" settings like in an editor.
#[with_fallible_options]
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct GeneralSettingsContent {
    /// What shell to use when opening a terminal.
    ///
    /// Default: system
    pub shell: Option<Shell>,

    /// Where the terminal starts: "home", "previous", or a custom path.
    ///
    /// Default: home
    pub working_directory: Option<GeneralWorkingDirectory>,

    /// Input mode: "carrot" (context chips, custom input) or "shell_ps1" (native prompt).
    ///
    /// Default: carrot
    pub input_mode: Option<GeneralInputMode>,

    /// Maximum number of lines to keep in the scrollback history.
    /// Maximum allowed value is 100_000.
    /// 0 disables scrolling.
    ///
    /// Default: 10_000
    pub scrollback_history: Option<usize>,

    /// Whether Alternate Scroll mode (code: ?1007) is active by default.
    /// Converts mouse scroll events into up/down key presses when in the
    /// alternate screen (e.g. vim, less).
    ///
    /// Default: on
    pub alternate_scroll: Option<AlternateScroll>,

    /// Whether the Option key behaves as the Meta key (macOS).
    ///
    /// Default: false
    pub option_as_meta: Option<bool>,

    /// Whether selecting text automatically copies to the system clipboard.
    ///
    /// Default: false
    pub copy_on_select: Option<bool>,

    /// Whether to keep the text selection after copying to the clipboard.
    ///
    /// Default: false
    pub keep_selection_on_copy: Option<bool>,

    /// Multiplier for mouse wheel scrolling speed.
    ///
    /// Default: 1.0
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub scroll_multiplier: Option<f32>,

    /// TUI-awareness policy. Controls whether Carrot actively protects
    /// in-place TUI redraws from scrollback corruption.
    ///
    /// - `full` (default): DEC 2026 synchronized updates, shell-emitted
    ///   TUI hints, and the cursor-up heuristic all active.
    /// - `strict_protocol`: DEC 2026 and shell hints only; heuristic off.
    /// - `off`: all three signals disabled (legacy behaviour, diagnosis).
    ///
    /// Default: full
    pub tui_awareness: Option<TuiAwareness>,

    /// Environment variables to inject into the terminal shell.
    /// Use `:` to separate multiple values.
    ///
    /// Default: {}
    pub env: Option<HashMap<String, String>>,
}

/// Where the terminal shell starts.
#[derive(
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum GeneralWorkingDirectory {
    /// User home directory ($HOME).
    #[default]
    Home,
    /// Last used directory from previous session.
    Previous,
    /// A fixed custom path (shell-expanded).
    Custom(String),
}

/// Terminal input mode.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum GeneralInputMode {
    /// Carrot mode: shell prompt suppressed, context chips visible, custom input bar.
    #[default]
    Carrot,
    /// Shell PS1 mode: native prompt (Starship, P10k) visible, raw shell input.
    ShellPs1,
}
