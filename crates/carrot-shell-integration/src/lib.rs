mod context;
pub mod gh_cli;
pub mod known_tui;
mod metadata;
mod osc_marker;
pub mod shell_install;

pub use context::{GitStats, ShellContext, shorten_path};
pub use known_tui::{KNOWN_TUI_COMMANDS, is_known_tui, known_tuis_env_value};
pub use metadata::{ShellMetadataPayload, TuiHintPayload};
pub use osc_marker::{PositionedMarker, PromptKindType, ShellMarker};
