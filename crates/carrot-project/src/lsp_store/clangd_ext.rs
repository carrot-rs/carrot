use std::{borrow::Cow, sync::Arc};

use ::serde::{Deserialize, Serialize};
use carrot_language::{CachedLspAdapter, Diagnostic, DiagnosticSourceKind};
use carrot_lsp::{LanguageServer, LanguageServerName};
use inazuma::WeakEntity;
use inazuma_util::ResultExt as _;

use crate::{LspStore, lsp_store::DocumentDiagnosticsUpdate};

pub const CLANGD_SERVER_NAME: LanguageServerName = LanguageServerName::new_static("clangd");
const INACTIVE_REGION_MESSAGE: &str = "inactive region";
const INACTIVE_DIAGNOSTIC_SEVERITY: carrot_lsp::DiagnosticSeverity =
    carrot_lsp::DiagnosticSeverity::INFORMATION;

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InactiveRegionsParams {
    pub text_document: carrot_lsp::OptionalVersionedTextDocumentIdentifier,
    pub regions: Vec<carrot_lsp::Range>,
}

/// InactiveRegions is a clangd extension that marks regions of inactive code.
pub struct InactiveRegions;

impl carrot_lsp::notification::Notification for InactiveRegions {
    type Params = InactiveRegionsParams;
    const METHOD: &'static str = "textDocument/inactiveRegions";
}

pub fn is_inactive_region(diag: &Diagnostic) -> bool {
    diag.is_unnecessary
        && diag.severity == INACTIVE_DIAGNOSTIC_SEVERITY
        && diag.message == INACTIVE_REGION_MESSAGE
        && diag
            .source
            .as_ref()
            .is_some_and(|v| v == &CLANGD_SERVER_NAME.0)
}

pub fn is_lsp_inactive_region(diag: &carrot_lsp::Diagnostic) -> bool {
    diag.severity == Some(INACTIVE_DIAGNOSTIC_SEVERITY)
        && diag.message == INACTIVE_REGION_MESSAGE
        && diag
            .source
            .as_ref()
            .is_some_and(|v| v == &CLANGD_SERVER_NAME.0)
}

pub fn register_notifications(
    lsp_store: WeakEntity<LspStore>,
    language_server: &LanguageServer,
    adapter: Arc<CachedLspAdapter>,
) {
    if language_server.name() != CLANGD_SERVER_NAME {
        return;
    }
    let server_id = language_server.server_id();

    language_server
        .on_notification::<InactiveRegions, _>({
            let adapter = adapter;
            let this = lsp_store;

            move |params: InactiveRegionsParams, cx| {
                let adapter = adapter.clone();
                this.update(cx, |this, cx| {
                    let diagnostics = params
                        .regions
                        .into_iter()
                        .map(|range| carrot_lsp::Diagnostic {
                            range,
                            severity: Some(INACTIVE_DIAGNOSTIC_SEVERITY),
                            source: Some(CLANGD_SERVER_NAME.to_string()),
                            message: INACTIVE_REGION_MESSAGE.to_string(),
                            tags: Some(vec![carrot_lsp::DiagnosticTag::UNNECESSARY]),
                            ..carrot_lsp::Diagnostic::default()
                        })
                        .collect();
                    let mapped_diagnostics = carrot_lsp::PublishDiagnosticsParams {
                        uri: params.text_document.uri,
                        version: params.text_document.version,
                        diagnostics,
                    };
                    this.merge_lsp_diagnostics(
                        DiagnosticSourceKind::Pushed,
                        vec![DocumentDiagnosticsUpdate {
                            server_id,
                            diagnostics: mapped_diagnostics,
                            result_id: None,
                            disk_based_sources: Cow::Borrowed(
                                &adapter.disk_based_diagnostic_sources,
                            ),
                            registration_id: None,
                        }],
                        |_, diag, _| !is_inactive_region(diag),
                        cx,
                    )
                    .log_err();
                })
                .ok();
            }
        })
        .detach();
}
