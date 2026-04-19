use carrot_actions::{debug_panel, dev};
use carrot_collab_ui::collab_panel;
use carrot_release_channel::ReleaseChannel;
use carrot_terminal_view::terminal_panel;
use inazuma::{App, Menu, MenuItem, OsAction};

pub fn app_menus(cx: &mut App) -> Vec<Menu> {
    use carrot_actions::Quit;

    let mut view_items = vec![
        MenuItem::action(
            "Zoom In",
            carrot_actions::IncreaseBufferFontSize { persist: false },
        ),
        MenuItem::action(
            "Zoom Out",
            carrot_actions::DecreaseBufferFontSize { persist: false },
        ),
        MenuItem::action(
            "Reset Zoom",
            carrot_actions::ResetBufferFontSize { persist: false },
        ),
        MenuItem::action(
            "Reset All Zoom",
            carrot_actions::ResetAllZoom { persist: false },
        ),
        MenuItem::separator(),
        MenuItem::action("Toggle Left Dock", carrot_workspace::ToggleLeftDock),
        MenuItem::action("Toggle Right Dock", carrot_workspace::ToggleRightDock),
        MenuItem::action("Toggle Bottom Dock", carrot_workspace::ToggleBottomDock),
        MenuItem::action("Toggle All Docks", carrot_workspace::ToggleAllDocks),
        MenuItem::submenu(Menu {
            name: "Editor Layout".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Split Up", carrot_workspace::SplitUp::default()),
                MenuItem::action("Split Down", carrot_workspace::SplitDown::default()),
                MenuItem::action("Split Left", carrot_workspace::SplitLeft::default()),
                MenuItem::action("Split Right", carrot_workspace::SplitRight::default()),
            ],
        }),
        MenuItem::separator(),
        MenuItem::action("Project Panel", carrot_actions::project_panel::ToggleFocus),
        MenuItem::action("Outline Panel", carrot_outline_panel::ToggleFocus),
        MenuItem::action("Collab Panel", collab_panel::ToggleFocus),
        MenuItem::action("Terminal Panel", terminal_panel::ToggleFocus),
        MenuItem::action("Debugger Panel", debug_panel::ToggleFocus),
        MenuItem::separator(),
        MenuItem::action("Diagnostics", carrot_diagnostics::Deploy),
        MenuItem::separator(),
    ];

    if ReleaseChannel::try_global(cx) == Some(ReleaseChannel::Dev) {
        view_items.push(MenuItem::action(
            "Toggle Inazuma Inspector",
            dev::ToggleInspector,
        ));
        view_items.push(MenuItem::separator());
    }

    vec![
        Menu {
            name: "Carrot".into(),
            disabled: false,
            items: vec![
                MenuItem::action("About Carrot", carrot_actions::About),
                MenuItem::action("Check for Updates", carrot_auto_update::Check),
                MenuItem::separator(),
                MenuItem::submenu(Menu::new("Settings").items([
                    MenuItem::action("Open Settings", carrot_actions::OpenSettings),
                    MenuItem::action("Open Settings File", super::OpenSettingsFile),
                    MenuItem::action("Open Project Settings", carrot_actions::OpenProjectSettings),
                    MenuItem::action("Open Project Settings File", super::OpenProjectSettingsFile),
                    MenuItem::action("Open Default Settings", super::OpenDefaultSettings),
                    MenuItem::separator(),
                    MenuItem::action("Open Keymap", carrot_actions::OpenKeymap),
                    MenuItem::action("Open Keymap File", carrot_actions::OpenKeymapFile),
                    MenuItem::action(
                        "Open Default Key Bindings",
                        carrot_actions::OpenDefaultKeymap,
                    ),
                    MenuItem::separator(),
                    MenuItem::action(
                        "Select Theme...",
                        carrot_actions::theme_selector::Toggle::default(),
                    ),
                    MenuItem::action(
                        "Select Icon Theme...",
                        carrot_actions::icon_theme_selector::Toggle::default(),
                    ),
                ])),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("Services", inazuma::SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Extensions", carrot_actions::Extensions::default()),
                #[cfg(not(target_os = "windows"))]
                MenuItem::action("Install CLI", carrot_install_cli::InstallCliBinary),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action("Hide Carrot", super::Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action("Hide Others", super::HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action("Show All", super::ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Carrot", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New", carrot_workspace::NewFile),
                MenuItem::action("New Window", carrot_workspace::NewWindow),
                MenuItem::separator(),
                #[cfg(not(target_os = "macos"))]
                MenuItem::action("Open File...", carrot_workspace::OpenFiles),
                MenuItem::action(
                    if cfg!(not(target_os = "macos")) {
                        "Open Folder..."
                    } else {
                        "Open…"
                    },
                    carrot_workspace::Open::default(),
                ),
                MenuItem::action(
                    "Open Recent...",
                    carrot_actions::OpenRecent {
                        create_new_window: false,
                    },
                ),
                MenuItem::action(
                    "Open Remote...",
                    carrot_actions::OpenRemote {
                        create_new_window: false,
                        from_existing_connection: false,
                    },
                ),
                MenuItem::separator(),
                MenuItem::action(
                    "Add Folder to Project…",
                    carrot_workspace::AddFolderToProject,
                ),
                MenuItem::separator(),
                MenuItem::action("Save", carrot_workspace::Save { save_intent: None }),
                MenuItem::action("Save As…", carrot_workspace::SaveAs),
                MenuItem::action("Save All", carrot_workspace::SaveAll { save_intent: None }),
                MenuItem::separator(),
                MenuItem::action(
                    "Close Editor",
                    carrot_workspace::CloseActiveItem { save_intent: None },
                ),
                MenuItem::action("Close Project", carrot_workspace::CloseProject),
                MenuItem::action("Close Window", carrot_workspace::CloseWindow),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("Undo", carrot_editor::actions::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", carrot_editor::actions::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", carrot_editor::actions::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", carrot_editor::actions::Copy, OsAction::Copy),
                MenuItem::action("Copy and Trim", carrot_editor::actions::CopyAndTrim),
                MenuItem::os_action("Paste", carrot_editor::actions::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::action("Find", carrot_search::buffer_search::Deploy::find()),
                MenuItem::action("Find in Project", carrot_workspace::DeploySearch::find()),
                MenuItem::separator(),
                MenuItem::action(
                    "Toggle Line Comment",
                    carrot_editor::actions::ToggleComments::default(),
                ),
            ],
        },
        Menu {
            name: "Selection".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action(
                    "Select All",
                    carrot_editor::actions::SelectAll,
                    OsAction::SelectAll,
                ),
                MenuItem::action(
                    "Expand Selection",
                    carrot_editor::actions::SelectLargerSyntaxNode,
                ),
                MenuItem::action(
                    "Shrink Selection",
                    carrot_editor::actions::SelectSmallerSyntaxNode,
                ),
                MenuItem::action(
                    "Select Next Sibling",
                    carrot_editor::actions::SelectNextSyntaxNode,
                ),
                MenuItem::action(
                    "Select Previous Sibling",
                    carrot_editor::actions::SelectPreviousSyntaxNode,
                ),
                MenuItem::separator(),
                MenuItem::action(
                    "Add Cursor Above",
                    carrot_editor::actions::AddSelectionAbove {
                        skip_soft_wrap: true,
                    },
                ),
                MenuItem::action(
                    "Add Cursor Below",
                    carrot_editor::actions::AddSelectionBelow {
                        skip_soft_wrap: true,
                    },
                ),
                MenuItem::action(
                    "Select Next Occurrence",
                    carrot_editor::actions::SelectNext {
                        replace_newest: false,
                    },
                ),
                MenuItem::action(
                    "Select Previous Occurrence",
                    carrot_editor::actions::SelectPrevious {
                        replace_newest: false,
                    },
                ),
                MenuItem::action(
                    "Select All Occurrences",
                    carrot_editor::actions::SelectAllMatches,
                ),
                MenuItem::separator(),
                MenuItem::action("Move Line Up", carrot_editor::actions::MoveLineUp),
                MenuItem::action("Move Line Down", carrot_editor::actions::MoveLineDown),
                MenuItem::action(
                    "Duplicate Selection",
                    carrot_editor::actions::DuplicateLineDown,
                ),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: view_items,
        },
        Menu {
            name: "Go".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Back", carrot_workspace::GoBack),
                MenuItem::action("Forward", carrot_workspace::GoForward),
                MenuItem::separator(),
                MenuItem::action(
                    "Command Palette...",
                    carrot_actions::command_palette::Toggle,
                ),
                MenuItem::separator(),
                MenuItem::action(
                    "Go to File...",
                    carrot_workspace::ToggleFileFinder::default(),
                ),
                // MenuItem::action("Go to Symbol in Project", project_symbols::Toggle),
                MenuItem::action(
                    "Go to Symbol in Editor...",
                    carrot_actions::outline::ToggleOutline,
                ),
                MenuItem::action(
                    "Go to Line/Column...",
                    carrot_editor::actions::ToggleGoToLine,
                ),
                MenuItem::separator(),
                MenuItem::action("Go to Definition", carrot_editor::actions::GoToDefinition),
                MenuItem::action("Go to Declaration", carrot_editor::actions::GoToDeclaration),
                MenuItem::action(
                    "Go to Type Definition",
                    carrot_editor::actions::GoToTypeDefinition,
                ),
                MenuItem::action(
                    "Find All References",
                    carrot_editor::actions::FindAllReferences::default(),
                ),
                MenuItem::separator(),
                MenuItem::action(
                    "Next Problem",
                    carrot_editor::actions::GoToDiagnostic::default(),
                ),
                MenuItem::action(
                    "Previous Problem",
                    carrot_editor::actions::GoToPreviousDiagnostic::default(),
                ),
            ],
        },
        Menu {
            name: "Run".into(),
            disabled: false,
            items: vec![
                MenuItem::action(
                    "Spawn Task",
                    carrot_actions::Spawn::ViaModal {
                        reveal_target: None,
                    },
                ),
                MenuItem::action("Start Debugger", carrot_debugger_ui::Start),
                MenuItem::separator(),
                MenuItem::action("Edit tasks.json...", crate::carrot::OpenProjectTasks),
                MenuItem::action("Edit debug.json...", carrot_actions::OpenProjectDebugTasks),
                MenuItem::separator(),
                MenuItem::action("Continue", carrot_debugger_ui::Continue),
                MenuItem::action("Step Over", carrot_debugger_ui::StepOver),
                MenuItem::action("Step Into", carrot_debugger_ui::StepInto),
                MenuItem::action("Step Out", carrot_debugger_ui::StepOut),
                MenuItem::separator(),
                MenuItem::action(
                    "Toggle Breakpoint",
                    carrot_editor::actions::ToggleBreakpoint,
                ),
                MenuItem::action("Edit Breakpoint", carrot_editor::actions::EditLogBreakpoint),
                MenuItem::action(
                    "Clear All Breakpoints",
                    carrot_debugger_ui::ClearAllBreakpoints,
                ),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Minimize", super::Minimize),
                MenuItem::action("Zoom", super::Zoom),
                MenuItem::separator(),
            ],
        },
        Menu {
            name: "Help".into(),
            disabled: false,
            items: vec![
                MenuItem::action(
                    "View Release Notes Locally",
                    carrot_auto_update_ui::ViewReleaseNotesLocally,
                ),
                MenuItem::action("View Telemetry", carrot_actions::OpenTelemetryLog),
                MenuItem::action("View Dependency Licenses", carrot_actions::OpenLicenses),
                MenuItem::action("Show Welcome", carrot_onboarding::ShowWelcome),
                MenuItem::separator(),
                MenuItem::action(
                    "File Bug Report...",
                    carrot_actions::feedback::FileBugReport,
                ),
                MenuItem::action(
                    "Request Feature...",
                    carrot_actions::feedback::RequestFeature,
                ),
                MenuItem::action("Email Us...", carrot_actions::feedback::EmailCarrot),
                MenuItem::separator(),
                MenuItem::action(
                    "Documentation",
                    super::OpenBrowser {
                        url: "https://carrot.dev/docs".into(),
                    },
                ),
                MenuItem::action("Carrot Repository", carrot_feedback::OpenCarrotRepo),
                MenuItem::action(
                    "Carrot Twitter",
                    super::OpenBrowser {
                        url: "https://twitter.com/carrotdev".into(),
                    },
                ),
                MenuItem::action(
                    "Join the Team",
                    super::OpenBrowser {
                        url: "https://carrot.dev/jobs".into(),
                    },
                ),
            ],
        },
    ]
}
