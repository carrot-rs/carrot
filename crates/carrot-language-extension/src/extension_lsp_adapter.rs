use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use carrot_extension::{Extension, ExtensionLanguageServerProxy, WorktreeDelegate};
use carrot_language::{
    BinaryStatus, CodeLabel, DynLspInstaller, HighlightId, Language, LanguageName,
    LanguageServerBinaryLocations, LspAdapter, LspAdapterDelegate, Toolchain,
};
use carrot_lsp::{
    CodeActionKind, LanguageServerBinary, LanguageServerBinaryOptions, LanguageServerName,
    LanguageServerSelector, Uri,
};
use futures::{FutureExt, future::join_all, lock::OwnedMutexGuard};
use inazuma::{App, AppContext, AsyncApp, Task};
use inazuma_collections::{HashMap, HashSet};
use inazuma_util::{ResultExt, fs::make_file_executable, maybe, rel_path::RelPath};
use serde::Serialize;
use serde_json::Value;

use crate::{LanguageServerRegistryProxy, LspAccess};

/// An adapter that allows an [`LspAdapterDelegate`] to be used as a [`WorktreeDelegate`].
struct WorktreeDelegateAdapter(pub Arc<dyn LspAdapterDelegate>);

#[async_trait]
impl WorktreeDelegate for WorktreeDelegateAdapter {
    fn id(&self) -> u64 {
        self.0.worktree_id().to_proto()
    }

    fn root_path(&self) -> String {
        self.0.worktree_root_path().to_string_lossy().into_owned()
    }

    async fn read_text_file(&self, path: &RelPath) -> Result<String> {
        self.0.read_text_file(path).await
    }

    async fn which(&self, binary_name: String) -> Option<String> {
        self.0
            .which(binary_name.as_ref())
            .await
            .map(|path| path.to_string_lossy().into_owned())
    }

    async fn shell_env(&self) -> Vec<(String, String)> {
        self.0.shell_env().await.into_iter().collect()
    }
}

impl ExtensionLanguageServerProxy for LanguageServerRegistryProxy {
    fn register_language_server(
        &self,
        extension: Arc<dyn Extension>,
        language_server_id: LanguageServerName,
        language: LanguageName,
    ) {
        self.language_registry.register_lsp_adapter(
            language.clone(),
            Arc::new(ExtensionLspAdapter::new(
                extension,
                language_server_id,
                language,
            )),
        );
    }

    fn remove_language_server(
        &self,
        language: &LanguageName,
        language_server_name: &LanguageServerName,
        cx: &mut App,
    ) -> Task<Result<()>> {
        self.language_registry
            .remove_lsp_adapter(language, language_server_name);

        let mut tasks = Vec::new();
        match &self.lsp_access {
            LspAccess::ViaLspStore(lsp_store) => lsp_store.update(cx, |lsp_store, cx| {
                let stop_task = lsp_store.stop_language_servers_for_buffers(
                    Vec::new(),
                    HashSet::from_iter([LanguageServerSelector::Name(
                        language_server_name.clone(),
                    )]),
                    cx,
                );
                tasks.push(stop_task);
            }),
            LspAccess::ViaWorkspaces(lsp_store_provider) => {
                if let Ok(lsp_stores) = lsp_store_provider(cx) {
                    for lsp_store in lsp_stores {
                        lsp_store.update(cx, |lsp_store, cx| {
                            let stop_task = lsp_store.stop_language_servers_for_buffers(
                                Vec::new(),
                                HashSet::from_iter([LanguageServerSelector::Name(
                                    language_server_name.clone(),
                                )]),
                                cx,
                            );
                            tasks.push(stop_task);
                        });
                    }
                }
            }
            LspAccess::Noop => {}
        }

        cx.background_spawn(async move {
            let results = join_all(tasks).await;
            for result in results {
                result?;
            }
            Ok(())
        })
    }

    fn update_language_server_status(
        &self,
        language_server_id: LanguageServerName,
        status: BinaryStatus,
    ) {
        log::debug!(
            "updating binary status for {} to {:?}",
            language_server_id,
            status
        );
        self.language_registry
            .update_lsp_binary_status(language_server_id, status);
    }
}

struct ExtensionLspAdapter {
    extension: Arc<dyn Extension>,
    language_server_id: LanguageServerName,
    language_name: LanguageName,
}

impl ExtensionLspAdapter {
    fn new(
        extension: Arc<dyn Extension>,
        language_server_id: LanguageServerName,
        language_name: LanguageName,
    ) -> Self {
        Self {
            extension,
            language_server_id,
            language_name,
        }
    }
}

#[async_trait(?Send)]
impl DynLspInstaller for ExtensionLspAdapter {
    fn get_language_server_command(
        self: Arc<Self>,
        delegate: Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: LanguageServerBinaryOptions,
        _: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _: AsyncApp,
    ) -> LanguageServerBinaryLocations {
        async move {
            let ret = maybe!(async move {
                let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
                let command = self
                    .extension
                    .language_server_command(
                        self.language_server_id.clone(),
                        self.language_name.clone(),
                        delegate,
                    )
                    .await?;

                // on windows, extensions might produce weird paths
                // that start with a leading slash due to WASI
                // requiring that for PWD and friends so account for
                // that here and try to transform those paths back
                // to windows paths
                //
                // if we don't do this, std will interpret the path as relative,
                // which changes join behavior
                let command_path: &Path = if cfg!(windows)
                    && let Some(command) = command.command.to_str()
                {
                    let mut chars = command.chars();
                    if chars.next().is_some_and(|c| c == '/')
                        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                        && chars.next().is_some_and(|c| c == ':')
                        && chars.next().is_some_and(|c| c == '\\' || c == '/')
                    {
                        // looks like a windows path with a leading slash, so strip it
                        command.strip_prefix('/').unwrap().as_ref()
                    } else {
                        command.as_ref()
                    }
                } else {
                    command.command.as_ref()
                };
                let path = self.extension.path_from_extension(command_path);

                // TODO: This should now be done via the `make_file_executable` function in
                // the extension API, but we're leaving these existing usages in place temporarily
                // to avoid any compatibility issues with the extension versions.
                //
                // We can remove once the following extension versions no longer see any use:
                // - toml@0.0.2
                // - zig@0.0.1
                if ["toml", "zig"].contains(&self.extension.manifest().id.as_ref())
                    && path.starts_with(&self.extension.work_dir())
                {
                    make_file_executable(&path)
                        .await
                        .context("failed to set file permissions")?;
                }

                Ok(LanguageServerBinary {
                    path,
                    arguments: command
                        .args
                        .into_iter()
                        .map(|arg| {
                            // on windows, extensions might produce weird paths
                            // that start with a leading slash due to WASI
                            // requiring that for PWD and friends so account for
                            // that here and try to transform those paths back
                            // to windows paths
                            if cfg!(windows) {
                                let mut chars = arg.chars();
                                if chars.next().is_some_and(|c| c == '/')
                                    && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                                    && chars.next().is_some_and(|c| c == ':')
                                    && chars.next().is_some_and(|c| c == '\\' || c == '/')
                                {
                                    // looks like a windows path with a leading slash, so strip it
                                    arg.strip_prefix('/').unwrap().into()
                                } else {
                                    arg.into()
                                }
                            } else {
                                arg.into()
                            }
                        })
                        .collect(),
                    env: Some(command.env.into_iter().collect()),
                })
            })
            .await;
            (ret, None)
        }
        .boxed_local()
    }

    async fn try_fetch_server_binary(
        &self,
        _: &Arc<dyn LspAdapterDelegate>,
        _: PathBuf,
        _: bool,
        _: &mut AsyncApp,
    ) -> Result<LanguageServerBinary> {
        unreachable!("get_language_server_command is overridden")
    }
}

#[async_trait(?Send)]
impl LspAdapter for ExtensionLspAdapter {
    fn name(&self) -> LanguageServerName {
        self.language_server_id.clone()
    }

    fn code_action_kinds(&self) -> Option<Vec<CodeActionKind>> {
        let code_action_kinds = self
            .extension
            .manifest()
            .language_servers
            .get(&self.language_server_id)
            .and_then(|server| server.code_action_kinds.clone());

        code_action_kinds.or(Some(vec![
            CodeActionKind::EMPTY,
            CodeActionKind::QUICKFIX,
            CodeActionKind::REFACTOR,
            CodeActionKind::REFACTOR_EXTRACT,
            CodeActionKind::SOURCE,
        ]))
    }

    fn language_ids(&self) -> HashMap<LanguageName, String> {
        // TODO: The language IDs can be provided via the language server options
        // in `extension.toml now but we're leaving these existing usages in place temporarily
        // to avoid any compatibility issues between Carrot and the extension versions.
        //
        // We can remove once the following extension versions no longer see any use:
        // - php@0.0.1
        if self.extension.manifest().id.as_ref() == "php" {
            return HashMap::from_iter([(LanguageName::new_static("PHP"), "php".into())]);
        }

        self.extension
            .manifest()
            .language_servers
            .get(&self.language_server_id)
            .map(|server| server.language_ids.clone())
            .unwrap_or_default()
    }

    async fn initialization_options(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_options = self
            .extension
            .language_server_initialization_options(
                self.language_server_id.clone(),
                self.language_name.clone(),
                delegate,
            )
            .await?;
        Ok(if let Some(json_options) = json_options {
            serde_json::from_str(&json_options).with_context(|| {
                format!("failed to parse initialization_options from extension: {json_options}")
            })?
        } else {
            None
        })
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: Option<Uri>,
        _cx: &mut AsyncApp,
    ) -> Result<Value> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_options: Option<String> = self
            .extension
            .language_server_workspace_configuration(self.language_server_id.clone(), delegate)
            .await?;
        Ok(if let Some(json_options) = json_options {
            serde_json::from_str(&json_options).with_context(|| {
                format!("failed to parse workspace_configuration from extension: {json_options}")
            })?
        } else {
            serde_json::json!({})
        })
    }

    async fn initialization_options_schema(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _cached_binary: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _cx: &mut AsyncApp,
    ) -> Option<serde_json::Value> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_schema: Option<String> = self
            .extension
            .language_server_initialization_options_schema(
                self.language_server_id.clone(),
                delegate,
            )
            .await
            .ok()
            .flatten();
        json_schema.and_then(|s| serde_json::from_str(&s).ok())
    }

    async fn settings_schema(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _cached_binary: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _cx: &mut AsyncApp,
    ) -> Option<serde_json::Value> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_schema: Option<String> = self
            .extension
            .language_server_workspace_configuration_schema(
                self.language_server_id.clone(),
                delegate,
            )
            .await
            .ok()
            .flatten();
        json_schema.and_then(|s| serde_json::from_str(&s).ok())
    }

    async fn additional_initialization_options(
        self: Arc<Self>,
        target_language_server_id: LanguageServerName,
        delegate: &Arc<dyn LspAdapterDelegate>,
    ) -> Result<Option<serde_json::Value>> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_options: Option<String> = self
            .extension
            .language_server_additional_initialization_options(
                self.language_server_id.clone(),
                target_language_server_id.clone(),
                delegate,
            )
            .await?;
        Ok(if let Some(json_options) = json_options {
            serde_json::from_str(&json_options).with_context(|| {
                format!(
                    "failed to parse additional_initialization_options from extension: {json_options}"
                )
            })?
        } else {
            None
        })
    }

    async fn additional_workspace_configuration(
        self: Arc<Self>,
        target_language_server_id: LanguageServerName,

        delegate: &Arc<dyn LspAdapterDelegate>,

        _cx: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        let delegate = Arc::new(WorktreeDelegateAdapter(delegate.clone())) as _;
        let json_options: Option<String> = self
            .extension
            .language_server_additional_workspace_configuration(
                self.language_server_id.clone(),
                target_language_server_id.clone(),
                delegate,
            )
            .await?;
        Ok(if let Some(json_options) = json_options {
            serde_json::from_str(&json_options).with_context(|| {
                format!("failed to parse additional_workspace_configuration from extension: {json_options}")
            })?
        } else {
            None
        })
    }

    async fn labels_for_completions(
        self: Arc<Self>,
        completions: &[carrot_lsp::CompletionItem],
        language: &Arc<Language>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        let completions = completions
            .iter()
            .cloned()
            .map(lsp_completion_to_extension)
            .collect::<Vec<_>>();

        let labels = self
            .extension
            .labels_for_completions(self.language_server_id.clone(), completions)
            .await?;

        Ok(labels_from_extension(labels, language))
    }

    async fn labels_for_symbols(
        self: Arc<Self>,
        symbols: &[carrot_language::Symbol],
        language: &Arc<Language>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        let symbols = symbols
            .iter()
            .cloned()
            .map(
                |carrot_language::Symbol {
                     name,
                     kind,
                     container_name,
                 }| carrot_extension::Symbol {
                    name,
                    kind: lsp_symbol_kind_to_extension(kind),
                    container_name,
                },
            )
            .collect::<Vec<_>>();

        let labels = self
            .extension
            .labels_for_symbols(self.language_server_id.clone(), symbols)
            .await?;

        Ok(labels_from_extension(labels, language))
    }

    fn is_extension(&self) -> bool {
        true
    }
}

fn labels_from_extension(
    labels: Vec<Option<carrot_extension::CodeLabel>>,
    language: &Arc<Language>,
) -> Vec<Option<CodeLabel>> {
    labels
        .into_iter()
        .map(|label| {
            let label = label?;
            let runs = if label.code.is_empty() {
                Vec::new()
            } else {
                language.highlight_text(&label.code.as_str().into(), 0..label.code.len())
            };
            build_code_label(&label, &runs, language)
        })
        .collect()
}

fn build_code_label(
    label: &carrot_extension::CodeLabel,
    parsed_runs: &[(Range<usize>, HighlightId)],
    language: &Arc<Language>,
) -> Option<CodeLabel> {
    let mut text = String::new();
    let mut runs = vec![];

    for span in &label.spans {
        match span {
            carrot_extension::CodeLabelSpan::CodeRange(range) => {
                let code_span = &label.code.get(range.clone())?;
                let mut input_ix = range.start;
                let mut output_ix = text.len();
                for (run_range, id) in parsed_runs {
                    if run_range.start >= range.end {
                        break;
                    }
                    if run_range.end <= input_ix {
                        continue;
                    }

                    if run_range.start > input_ix {
                        let len = run_range.start - input_ix;
                        output_ix += len;
                        input_ix += len;
                    }

                    let len = range.end.min(run_range.end) - input_ix;
                    runs.push((output_ix..output_ix + len, *id));
                    output_ix += len;
                    input_ix += len;
                }

                text.push_str(code_span);
            }
            carrot_extension::CodeLabelSpan::Literal(span) => {
                if let Some(highlight_id) = language
                    .grammar()
                    .zip(span.highlight_name.as_ref())
                    .and_then(|(grammar, highlight_name)| {
                        grammar.highlight_id_for_name(highlight_name)
                    })
                {
                    let ix = text.len();
                    runs.push((ix..ix + span.text.len(), highlight_id));
                }
                text.push_str(&span.text);
            }
        }
    }

    let filter_range = label.filter_range.clone();
    text.get(filter_range.clone())?;
    Some(CodeLabel::new(text, filter_range, runs))
}

fn lsp_completion_to_extension(value: carrot_lsp::CompletionItem) -> carrot_extension::Completion {
    carrot_extension::Completion {
        label: value.label,
        label_details: value
            .label_details
            .map(lsp_completion_item_label_details_to_extension),
        detail: value.detail,
        kind: value.kind.map(lsp_completion_item_kind_to_extension),
        insert_text_format: value
            .insert_text_format
            .map(lsp_insert_text_format_to_extension),
    }
}

fn lsp_completion_item_label_details_to_extension(
    value: carrot_lsp::CompletionItemLabelDetails,
) -> carrot_extension::CompletionLabelDetails {
    carrot_extension::CompletionLabelDetails {
        detail: value.detail,
        description: value.description,
    }
}

fn lsp_completion_item_kind_to_extension(
    value: carrot_lsp::CompletionItemKind,
) -> carrot_extension::CompletionKind {
    match value {
        carrot_lsp::CompletionItemKind::TEXT => carrot_extension::CompletionKind::Text,
        carrot_lsp::CompletionItemKind::METHOD => carrot_extension::CompletionKind::Method,
        carrot_lsp::CompletionItemKind::FUNCTION => carrot_extension::CompletionKind::Function,
        carrot_lsp::CompletionItemKind::CONSTRUCTOR => {
            carrot_extension::CompletionKind::Constructor
        }
        carrot_lsp::CompletionItemKind::FIELD => carrot_extension::CompletionKind::Field,
        carrot_lsp::CompletionItemKind::VARIABLE => carrot_extension::CompletionKind::Variable,
        carrot_lsp::CompletionItemKind::CLASS => carrot_extension::CompletionKind::Class,
        carrot_lsp::CompletionItemKind::INTERFACE => carrot_extension::CompletionKind::Interface,
        carrot_lsp::CompletionItemKind::MODULE => carrot_extension::CompletionKind::Module,
        carrot_lsp::CompletionItemKind::PROPERTY => carrot_extension::CompletionKind::Property,
        carrot_lsp::CompletionItemKind::UNIT => carrot_extension::CompletionKind::Unit,
        carrot_lsp::CompletionItemKind::VALUE => carrot_extension::CompletionKind::Value,
        carrot_lsp::CompletionItemKind::ENUM => carrot_extension::CompletionKind::Enum,
        carrot_lsp::CompletionItemKind::KEYWORD => carrot_extension::CompletionKind::Keyword,
        carrot_lsp::CompletionItemKind::SNIPPET => carrot_extension::CompletionKind::Snippet,
        carrot_lsp::CompletionItemKind::COLOR => carrot_extension::CompletionKind::Color,
        carrot_lsp::CompletionItemKind::FILE => carrot_extension::CompletionKind::File,
        carrot_lsp::CompletionItemKind::REFERENCE => carrot_extension::CompletionKind::Reference,
        carrot_lsp::CompletionItemKind::FOLDER => carrot_extension::CompletionKind::Folder,
        carrot_lsp::CompletionItemKind::ENUM_MEMBER => carrot_extension::CompletionKind::EnumMember,
        carrot_lsp::CompletionItemKind::CONSTANT => carrot_extension::CompletionKind::Constant,
        carrot_lsp::CompletionItemKind::STRUCT => carrot_extension::CompletionKind::Struct,
        carrot_lsp::CompletionItemKind::EVENT => carrot_extension::CompletionKind::Event,
        carrot_lsp::CompletionItemKind::OPERATOR => carrot_extension::CompletionKind::Operator,
        carrot_lsp::CompletionItemKind::TYPE_PARAMETER => {
            carrot_extension::CompletionKind::TypeParameter
        }
        _ => carrot_extension::CompletionKind::Other(extract_int(value)),
    }
}

fn lsp_insert_text_format_to_extension(
    value: carrot_lsp::InsertTextFormat,
) -> carrot_extension::InsertTextFormat {
    match value {
        carrot_lsp::InsertTextFormat::PLAIN_TEXT => carrot_extension::InsertTextFormat::PlainText,
        carrot_lsp::InsertTextFormat::SNIPPET => carrot_extension::InsertTextFormat::Snippet,
        _ => carrot_extension::InsertTextFormat::Other(extract_int(value)),
    }
}

fn lsp_symbol_kind_to_extension(value: carrot_lsp::SymbolKind) -> carrot_extension::SymbolKind {
    match value {
        carrot_lsp::SymbolKind::FILE => carrot_extension::SymbolKind::File,
        carrot_lsp::SymbolKind::MODULE => carrot_extension::SymbolKind::Module,
        carrot_lsp::SymbolKind::NAMESPACE => carrot_extension::SymbolKind::Namespace,
        carrot_lsp::SymbolKind::PACKAGE => carrot_extension::SymbolKind::Package,
        carrot_lsp::SymbolKind::CLASS => carrot_extension::SymbolKind::Class,
        carrot_lsp::SymbolKind::METHOD => carrot_extension::SymbolKind::Method,
        carrot_lsp::SymbolKind::PROPERTY => carrot_extension::SymbolKind::Property,
        carrot_lsp::SymbolKind::FIELD => carrot_extension::SymbolKind::Field,
        carrot_lsp::SymbolKind::CONSTRUCTOR => carrot_extension::SymbolKind::Constructor,
        carrot_lsp::SymbolKind::ENUM => carrot_extension::SymbolKind::Enum,
        carrot_lsp::SymbolKind::INTERFACE => carrot_extension::SymbolKind::Interface,
        carrot_lsp::SymbolKind::FUNCTION => carrot_extension::SymbolKind::Function,
        carrot_lsp::SymbolKind::VARIABLE => carrot_extension::SymbolKind::Variable,
        carrot_lsp::SymbolKind::CONSTANT => carrot_extension::SymbolKind::Constant,
        carrot_lsp::SymbolKind::STRING => carrot_extension::SymbolKind::String,
        carrot_lsp::SymbolKind::NUMBER => carrot_extension::SymbolKind::Number,
        carrot_lsp::SymbolKind::BOOLEAN => carrot_extension::SymbolKind::Boolean,
        carrot_lsp::SymbolKind::ARRAY => carrot_extension::SymbolKind::Array,
        carrot_lsp::SymbolKind::OBJECT => carrot_extension::SymbolKind::Object,
        carrot_lsp::SymbolKind::KEY => carrot_extension::SymbolKind::Key,
        carrot_lsp::SymbolKind::NULL => carrot_extension::SymbolKind::Null,
        carrot_lsp::SymbolKind::ENUM_MEMBER => carrot_extension::SymbolKind::EnumMember,
        carrot_lsp::SymbolKind::STRUCT => carrot_extension::SymbolKind::Struct,
        carrot_lsp::SymbolKind::EVENT => carrot_extension::SymbolKind::Event,
        carrot_lsp::SymbolKind::OPERATOR => carrot_extension::SymbolKind::Operator,
        carrot_lsp::SymbolKind::TYPE_PARAMETER => carrot_extension::SymbolKind::TypeParameter,
        _ => carrot_extension::SymbolKind::Other(extract_int(value)),
    }
}

fn extract_int<T: Serialize>(value: T) -> i32 {
    maybe!({
        let kind = serde_json::to_value(&value)?;
        serde_json::from_value(kind)
    })
    .log_err()
    .unwrap_or(-1)
}

#[test]
fn test_build_code_label() {
    use inazuma_util::test::marked_text_ranges;

    let (code, code_ranges) = marked_text_ranges(
        "«const» «a»: «fn»(«Bcd»(«Efgh»)) -> «Ijklm» = pqrs.tuv",
        false,
    );
    let code_runs = code_ranges
        .into_iter()
        .map(|range| (range, HighlightId(0)))
        .collect::<Vec<_>>();

    let label = build_code_label(
        &carrot_extension::CodeLabel {
            spans: vec![
                carrot_extension::CodeLabelSpan::CodeRange(code.find("pqrs").unwrap()..code.len()),
                carrot_extension::CodeLabelSpan::CodeRange(
                    code.find(": fn").unwrap()..code.find(" = ").unwrap(),
                ),
            ],
            filter_range: 0.."pqrs.tuv".len(),
            code,
        },
        &code_runs,
        &carrot_language::PLAIN_TEXT,
    )
    .unwrap();

    let (label_text, label_ranges) =
        marked_text_ranges("pqrs.tuv: «fn»(«Bcd»(«Efgh»)) -> «Ijklm»", false);
    let label_runs = label_ranges
        .into_iter()
        .map(|range| (range, HighlightId(0)))
        .collect::<Vec<_>>();

    assert_eq!(
        label,
        CodeLabel::new(label_text, label.filter_range.clone(), label_runs)
    )
}

#[test]
fn test_build_code_label_with_invalid_ranges() {
    use inazuma_util::test::marked_text_ranges;

    let (code, code_ranges) = marked_text_ranges("const «a»: «B» = '🏀'", false);
    let code_runs = code_ranges
        .into_iter()
        .map(|range| (range, HighlightId(0)))
        .collect::<Vec<_>>();

    // A span uses a code range that is invalid because it starts inside of
    // a multi-byte character.
    let label = build_code_label(
        &carrot_extension::CodeLabel {
            spans: vec![
                carrot_extension::CodeLabelSpan::CodeRange(
                    code.find('B').unwrap()..code.find(" = ").unwrap(),
                ),
                carrot_extension::CodeLabelSpan::CodeRange(
                    (code.find('🏀').unwrap() + 1)..code.len(),
                ),
            ],
            filter_range: 0.."B".len(),
            code,
        },
        &code_runs,
        &carrot_language::PLAIN_TEXT,
    );
    assert!(label.is_none());

    // Filter range extends beyond actual text
    let label = build_code_label(
        &carrot_extension::CodeLabel {
            spans: vec![carrot_extension::CodeLabelSpan::Literal(
                carrot_extension::CodeLabelSpanLiteral {
                    text: "abc".into(),
                    highlight_name: Some("type".into()),
                },
            )],
            filter_range: 0..5,
            code: String::new(),
        },
        &code_runs,
        &carrot_language::PLAIN_TEXT,
    );
    assert!(label.is_none());
}
