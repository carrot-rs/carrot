use carrot_project::{ProjectEntryId, WorktreeId};
use inazuma::{Action, Entity, actions};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{fmt, sync::Arc};

use crate::SplitDirection;
use crate::item::{ItemHandle, WeakItemHandle};
use crate::pane::Pane;

/// A selected entry in e.g. project panel.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SelectedEntry {
    pub worktree_id: WorktreeId,
    pub entry_id: ProjectEntryId,
}

/// A group of selected entries from project panel.
#[derive(Debug)]
pub struct DraggedSelection {
    pub active_selection: SelectedEntry,
    pub marked_selections: Arc<[SelectedEntry]>,
}

impl DraggedSelection {
    pub fn items<'a>(&'a self) -> Box<dyn Iterator<Item = &'a SelectedEntry> + 'a> {
        if self.marked_selections.contains(&self.active_selection) {
            Box::new(self.marked_selections.iter())
        } else {
            Box::new(std::iter::once(&self.active_selection))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SaveIntent {
    /// write all files (even if unchanged)
    /// prompt before overwriting on-disk changes
    Save,
    /// same as Save, but without auto formatting
    SaveWithoutFormat,
    /// write any files that have local changes
    /// prompt before overwriting on-disk changes
    SaveAll,
    /// always prompt for a new path
    SaveAs,
    /// prompt "you have unsaved changes" before writing
    Close,
    /// write all dirty files, don't prompt on conflict
    Overwrite,
    /// skip all save-related behavior
    Skip,
}

/// Activates a specific item in the pane by its index.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
pub struct ActivateItem(pub usize);

/// Closes the currently active item in the pane.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct CloseActiveItem {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes all inactive items in the pane.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct CloseOtherItems {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes all multibuffers in the pane.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct CloseMultibufferItems {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes all items in the pane.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct CloseAllItems {
    #[serde(default)]
    pub save_intent: Option<SaveIntent>,
}

/// Closes all items that have no unsaved changes.
#[derive(Clone, PartialEq, Debug, Default, Action)]
#[action(namespace = pane)]
pub struct CloseCleanItems;

/// Closes all items to the right of the current item.
#[derive(Clone, PartialEq, Debug, Default, Action)]
#[action(namespace = pane)]
pub struct CloseItemsToTheRight;

/// Closes all items to the left of the current item.
#[derive(Clone, PartialEq, Debug, Default, Action)]
#[action(namespace = pane)]
pub struct CloseItemsToTheLeft;

/// Reveals the current item in the project panel.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct RevealInProjectPanel {
    #[serde(skip)]
    pub entry_id: Option<u64>,
}

/// Opens the search interface with the specified configuration.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = pane)]
#[serde(deny_unknown_fields)]
pub struct DeploySearch {
    #[serde(default)]
    pub replace_enabled: bool,
    #[serde(default)]
    pub included_files: Option<String>,
    #[serde(default)]
    pub excluded_files: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub enum SplitMode {
    /// Clone the current pane.
    #[default]
    ClonePane,
    /// Create an empty new pane.
    EmptyPane,
    /// Move the item into a new pane. This will map to nop if only one pane exists.
    MovePane,
}

macro_rules! split_structs {
    ($($name:ident => $doc:literal),* $(,)?) => {
        $(
            #[doc = $doc]
            #[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Default, Action)]
            #[action(namespace = pane)]
            #[serde(deny_unknown_fields, default)]
            pub struct $name {
                pub mode: SplitMode,
            }
        )*
    };
}

split_structs!(
    SplitLeft => "Splits the pane to the left.",
    SplitRight => "Splits the pane to the right.",
    SplitUp => "Splits the pane upward.",
    SplitDown => "Splits the pane downward.",
    SplitHorizontal => "Splits the pane horizontally.",
    SplitVertical => "Splits the pane vertically."
);

actions!(
    pane,
    [
        /// Activates the previous item in the pane.
        ActivatePreviousItem,
        /// Activates the next item in the pane.
        ActivateNextItem,
        /// Activates the last item in the pane.
        ActivateLastItem,
        /// Switches to the alternate file.
        AlternateFile,
        /// Navigates back in history.
        GoBack,
        /// Navigates forward in history.
        GoForward,
        /// Navigates back in the tag stack.
        GoToOlderTag,
        /// Navigates forward in the tag stack.
        GoToNewerTag,
        /// Joins this pane into the next pane.
        JoinIntoNext,
        /// Joins all panes into one.
        JoinAll,
        /// Reopens the most recently closed item.
        ReopenClosedItem,
        /// Splits the pane to the left, moving the current item.
        SplitAndMoveLeft,
        /// Splits the pane upward, moving the current item.
        SplitAndMoveUp,
        /// Splits the pane to the right, moving the current item.
        SplitAndMoveRight,
        /// Splits the pane downward, moving the current item.
        SplitAndMoveDown,
        /// Swaps the current item with the one to the left.
        SwapItemLeft,
        /// Swaps the current item with the one to the right.
        SwapItemRight,
        /// Toggles preview mode for the current tab.
        TogglePreviewTab,
    ]
);

impl DeploySearch {
    pub fn find() -> Self {
        Self {
            replace_enabled: false,
            included_files: None,
            excluded_files: None,
        }
    }
}

pub const MAX_NAVIGATION_HISTORY_LEN: usize = 1024;

pub enum Event {
    AddItem {
        item: Box<dyn ItemHandle>,
    },
    ActivateItem {
        local: bool,
        focus_changed: bool,
    },
    Remove {
        focus_on_pane: Option<Entity<Pane>>,
    },
    RemovedItem {
        item: Box<dyn ItemHandle>,
    },
    Split {
        direction: SplitDirection,
        mode: SplitMode,
    },
    ItemPinned,
    ItemUnpinned,
    JoinAll,
    JoinIntoNext,
    ChangeItemTitle,
    Focus,
    ZoomIn,
    ZoomOut,
    UserSavedItem {
        item: Box<dyn WeakItemHandle>,
        save_intent: SaveIntent,
    },
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::AddItem { item } => f
                .debug_struct("AddItem")
                .field("item", &item.item_id())
                .finish(),
            Event::ActivateItem { local, .. } => f
                .debug_struct("ActivateItem")
                .field("local", local)
                .finish(),
            Event::Remove { .. } => f.write_str("Remove"),
            Event::RemovedItem { item } => f
                .debug_struct("RemovedItem")
                .field("item", &item.item_id())
                .finish(),
            Event::Split { direction, mode } => f
                .debug_struct("Split")
                .field("direction", direction)
                .field("mode", mode)
                .finish(),
            Event::JoinAll => f.write_str("JoinAll"),
            Event::JoinIntoNext => f.write_str("JoinIntoNext"),
            Event::ChangeItemTitle => f.write_str("ChangeItemTitle"),
            Event::Focus => f.write_str("Focus"),
            Event::ZoomIn => f.write_str("ZoomIn"),
            Event::ZoomOut => f.write_str("ZoomOut"),
            Event::UserSavedItem { item, save_intent } => f
                .debug_struct("UserSavedItem")
                .field("item", &item.id())
                .field("save_intent", save_intent)
                .finish(),
            Event::ItemPinned => f.write_str("ItemPinned"),
            Event::ItemUnpinned => f.write_str("ItemUnpinned"),
        }
    }
}
