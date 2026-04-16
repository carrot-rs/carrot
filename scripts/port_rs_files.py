#!/usr/bin/env python3
"""Port .rs files: rename legacy crate references to Carrot equivalents."""
import os
import re

CRATE_DIRS = [
    'crates/carrot-dap-adapters',
    'crates/carrot-debug-adapter-extension',
    'crates/carrot-debugger-tools',
    'crates/carrot-debugger-ui',
    'crates/carrot-livekit-api',
    'crates/carrot-livekit-client',
    'crates/carrot-call',
    'crates/carrot-channel',
    'crates/carrot-collab',
    'crates/carrot-collab-ui',
    'crates/carrot-extension-cli',
    'crates/carrot-extension-host',
    'crates/carrot-extensions-ui',
    'crates/carrot-extension-api',
]

USE_REPLACEMENTS = [
    ('use gpui_tokio::', 'use inazuma_tokio::'),
    ('use gpui_platform::', 'use inazuma_platform::'),
    ('use gpui::', 'use inazuma::'),
    ('use ui_input::', 'use carrot_ui_input::'),
    ('use ui::', 'use carrot_ui::'),
    ('use theme_settings::', 'use carrot_theme_settings::'),
    ('use theme_extension::', 'use carrot_theme_extension::'),
    ('use theme::', 'use carrot_theme::'),
    ('use settings::', 'use inazuma_settings_framework::'),
    ('use workspace::', 'use carrot_workspace::'),
    ('use editor::', 'use carrot_editor::'),
    ('use collections::', 'use inazuma_collections::'),
    ('use util::', 'use inazuma_util::'),
    ('use util;', 'use inazuma_util;'),
    ('use project::', 'use carrot_project::'),
    ('use language_model::', 'use carrot_language_model::'),
    ('use language_extension::', 'use carrot_language_extension::'),
    ('use language::', 'use carrot_language::'),
    ('use client::', 'use carrot_client::'),
    ('use fs::', 'use carrot_fs::'),
    ('use picker::', 'use inazuma_picker::'),
    ('use menu::', 'use inazuma_menu::'),
    ('use zed_actions::', 'use carrot_actions::'),
    ('use fuzzy::', 'use inazuma_fuzzy::'),
    ('use dap_adapters::', 'use carrot_dap_adapters::'),
    ('use debugger_tools::', 'use carrot_debugger_tools::'),
    ('use debugger_ui::', 'use carrot_debugger_ui::'),
    ('use dap::', 'use carrot_dap::'),
    ('use task::', 'use carrot_task::'),
    ('use paths::', 'use carrot_paths::'),
    ('use text::', 'use carrot_text::'),
    ('use clock::', 'use inazuma_clock::'),
    ('use rpc::', 'use carrot_rpc::'),
    ('use db::', 'use carrot_db::'),
    ('use http_client_tls::', 'use carrot_http_client_tls::'),
    ('use http_client::', 'use carrot_http_client::'),
    ('use node_runtime::', 'use carrot_node_runtime::'),
    ('use livekit_api::', 'use carrot_livekit_api::'),
    ('use livekit_client::', 'use carrot_livekit_client::'),
    ('use feature_flags::', 'use carrot_feature_flags::'),
    ('use telemetry_events::', 'use carrot_telemetry_events::'),
    ('use telemetry::', 'use carrot_telemetry::'),
    ('use audio::', 'use carrot_audio::'),
    ('use release_channel::', 'use carrot_release_channel::'),
    ('use cloud_api_types::', 'use carrot_cloud_api_types::'),
    ('use extension_host::', 'use carrot_extension_host::'),
    ('use extension::', 'use carrot_extension::'),
    ('use file_icons::', 'use carrot_file_icons::'),
    ('use notifications::', 'use carrot_notifications::'),
    ('use command_palette_hooks::', 'use carrot_command_palette_hooks::'),
    ('use tasks_ui::', 'use carrot_tasks_ui::'),
    ('use terminal_view::', 'use carrot_terminal_view::'),
    ('use time_format::', 'use carrot_time_format::'),
    ('use title_bar::', 'use carrot_title_bar::'),
    ('use vim_mode_setting::', 'use carrot_vim_mode_setting::'),
    ('use call::', 'use carrot_call::'),
    ('use channel::', 'use carrot_channel::'),
    ('use collab_ui::', 'use carrot_collab_ui::'),
    ('use collab::', 'use carrot_collab::'),
    ('use multi_buffer::', 'use carrot_multi_buffer::'),
    ('use lsp::', 'use carrot_lsp::'),
    ('use remote_server::', 'use carrot_remote_server::'),
    ('use remote::', 'use carrot_remote::'),
    ('use settings_content::', 'use carrot_settings_content::'),
    ('use snippet_provider::', 'use carrot_snippet_provider::'),
    ('use reqwest_client::', 'use carrot_reqwest_client::'),
    ('use session::', 'use carrot_session::'),
    ('use worktree::', 'use carrot_worktree::'),
    ('use agent::', 'use carrot_agent::'),
    ('use buffer_diff::', 'use carrot_buffer_diff::'),
    ('use git_hosting_providers::', 'use carrot_git_hosting_providers::'),
    ('use git_ui::', 'use carrot_git_ui::'),
    ('use git::', 'use carrot_git::'),
    ('use file_finder::', 'use carrot_file_finder::'),
    ('use prompt_store::', 'use carrot_prompt_store::'),
    ('use recent_projects::', 'use carrot_recent_projects::'),
    ('use zlog::', 'use carrot_log::'),
    ('use ztracing::', 'use carrot_tracing::'),
    ('use assistant_text_thread::', 'use carrot_assistant_text_thread::'),
    ('use assistant_slash_command::', 'use carrot_assistant_slash_command::'),
]

INLINE_PATTERNS = [
    (r'(?<![a-zA-Z0-9_])gpui_tokio::', 'inazuma_tokio::'),
    (r'(?<![a-zA-Z0-9_])gpui_platform::', 'inazuma_platform::'),
    (r'(?<![a-zA-Z0-9_])gpui::', 'inazuma::'),
    (r'(?<![a-zA-Z0-9_])ui_input::', 'carrot_ui_input::'),
    (r'(?<![a-zA-Z0-9_])ui::', 'carrot_ui::'),
    (r'(?<![a-zA-Z0-9_])theme_settings::', 'carrot_theme_settings::'),
    (r'(?<![a-zA-Z0-9_])theme_extension::', 'carrot_theme_extension::'),
    (r'(?<![a-zA-Z0-9_])theme::', 'carrot_theme::'),
    (r'(?<![a-zA-Z0-9_])settings::', 'inazuma_settings_framework::'),
    (r'(?<![a-zA-Z0-9_])workspace::', 'carrot_workspace::'),
    (r'(?<![a-zA-Z0-9_])editor::', 'carrot_editor::'),
    (r'(?<![a-zA-Z0-9_])collections::', 'inazuma_collections::'),
    (r'(?<![a-zA-Z0-9_])util::', 'inazuma_util::'),
    (r'(?<![a-zA-Z0-9_])project::', 'carrot_project::'),
    (r'(?<![a-zA-Z0-9_])language_model::', 'carrot_language_model::'),
    (r'(?<![a-zA-Z0-9_])language_extension::', 'carrot_language_extension::'),
    (r'(?<![a-zA-Z0-9_])language::', 'carrot_language::'),
    (r'(?<![a-zA-Z0-9_])client::', 'carrot_client::'),
    (r'(?<![a-zA-Z0-9_])fs::', 'carrot_fs::'),
    (r'(?<![a-zA-Z0-9_])picker::', 'inazuma_picker::'),
    (r'(?<![a-zA-Z0-9_])menu::', 'inazuma_menu::'),
    (r'(?<![a-zA-Z0-9_])zed_actions::', 'carrot_actions::'),
    (r'(?<![a-zA-Z0-9_])fuzzy::', 'inazuma_fuzzy::'),
    (r'(?<![a-zA-Z0-9_])dap_adapters::', 'carrot_dap_adapters::'),
    (r'(?<![a-zA-Z0-9_])debugger_tools::', 'carrot_debugger_tools::'),
    (r'(?<![a-zA-Z0-9_])debugger_ui::', 'carrot_debugger_ui::'),
    (r'(?<![a-zA-Z0-9_])dap::', 'carrot_dap::'),
    (r'(?<![a-zA-Z0-9_])task::', 'carrot_task::'),
    (r'(?<![a-zA-Z0-9_])paths::', 'carrot_paths::'),
    (r'(?<![a-zA-Z0-9_])text::', 'carrot_text::'),
    (r'(?<![a-zA-Z0-9_])clock::', 'inazuma_clock::'),
    (r'(?<![a-zA-Z0-9_])rpc::', 'carrot_rpc::'),
    (r'(?<![a-zA-Z0-9_])db::', 'carrot_db::'),
    (r'(?<![a-zA-Z0-9_])http_client_tls::', 'carrot_http_client_tls::'),
    (r'(?<![a-zA-Z0-9_])http_client::', 'carrot_http_client::'),
    (r'(?<![a-zA-Z0-9_])node_runtime::', 'carrot_node_runtime::'),
    (r'(?<![a-zA-Z0-9_])livekit_api::', 'carrot_livekit_api::'),
    (r'(?<![a-zA-Z0-9_])livekit_client::', 'carrot_livekit_client::'),
    (r'(?<![a-zA-Z0-9_])feature_flags::', 'carrot_feature_flags::'),
    (r'(?<![a-zA-Z0-9_])telemetry_events::', 'carrot_telemetry_events::'),
    (r'(?<![a-zA-Z0-9_])telemetry::', 'carrot_telemetry::'),
    (r'(?<![a-zA-Z0-9_])audio::', 'carrot_audio::'),
    (r'(?<![a-zA-Z0-9_])release_channel::', 'carrot_release_channel::'),
    (r'(?<![a-zA-Z0-9_])cloud_api_types::', 'carrot_cloud_api_types::'),
    (r'(?<![a-zA-Z0-9_])extension_host::', 'carrot_extension_host::'),
    (r'(?<![a-zA-Z0-9_])extension::', 'carrot_extension::'),
    (r'(?<![a-zA-Z0-9_])file_icons::', 'carrot_file_icons::'),
    (r'(?<![a-zA-Z0-9_])notifications::', 'carrot_notifications::'),
    (r'(?<![a-zA-Z0-9_])command_palette_hooks::', 'carrot_command_palette_hooks::'),
    (r'(?<![a-zA-Z0-9_])tasks_ui::', 'carrot_tasks_ui::'),
    (r'(?<![a-zA-Z0-9_])terminal_view::', 'carrot_terminal_view::'),
    (r'(?<![a-zA-Z0-9_])time_format::', 'carrot_time_format::'),
    (r'(?<![a-zA-Z0-9_])title_bar::', 'carrot_title_bar::'),
    (r'(?<![a-zA-Z0-9_])vim_mode_setting::', 'carrot_vim_mode_setting::'),
    (r'(?<![a-zA-Z0-9_])call::', 'carrot_call::'),
    (r'(?<![a-zA-Z0-9_])channel::', 'carrot_channel::'),
    (r'(?<![a-zA-Z0-9_])collab_ui::', 'carrot_collab_ui::'),
    (r'(?<![a-zA-Z0-9_])collab::', 'carrot_collab::'),
    (r'(?<![a-zA-Z0-9_])multi_buffer::', 'carrot_multi_buffer::'),
    (r'(?<![a-zA-Z0-9_])lsp::', 'carrot_lsp::'),
    (r'(?<![a-zA-Z0-9_])remote_server::', 'carrot_remote_server::'),
    (r'(?<![a-zA-Z0-9_])remote::', 'carrot_remote::'),
    (r'(?<![a-zA-Z0-9_])session::', 'carrot_session::'),
    (r'(?<![a-zA-Z0-9_])worktree::', 'carrot_worktree::'),
    (r'(?<![a-zA-Z0-9_])agent::', 'carrot_agent::'),
    (r'(?<![a-zA-Z0-9_])buffer_diff::', 'carrot_buffer_diff::'),
    (r'(?<![a-zA-Z0-9_])git_hosting_providers::', 'carrot_git_hosting_providers::'),
    (r'(?<![a-zA-Z0-9_])git_ui::', 'carrot_git_ui::'),
    (r'(?<![a-zA-Z0-9_])git::', 'carrot_git::'),
    (r'(?<![a-zA-Z0-9_])zlog::', 'carrot_log::'),
    (r'(?<![a-zA-Z0-9_])ztracing::', 'carrot_tracing::'),
]

COMPILED_INLINE = [(re.compile(p), r) for p, r in INLINE_PATTERNS]

def process_content(content):
    for old, new in USE_REPLACEMENTS:
        content = content.replace(old, new)

    content = content.replace('actions!(zed,', 'actions!(carrot,')  # legacy-ns → carrot

    for pattern, replacement in COMPILED_INLINE:
        content = pattern.sub(replacement, content)

    # Fix over-replacements
    content = content.replace('std::carrot_fs::', 'std::fs::')
    content = content.replace('smol::carrot_fs::', 'smol::fs::')
    content = content.replace('tokio::carrot_fs::', 'tokio::fs::')
    content = content.replace('futures::carrot_task::', 'futures::task::')
    content = content.replace('std::carrot_task::', 'std::task::')
    content = content.replace('tokio::carrot_task::', 'tokio::task::')
    content = content.replace('core::carrot_task::', 'core::task::')
    content = content.replace('std::carrot_channel::', 'std::channel::')
    content = content.replace('std::carrot_text::', 'std::text::')
    content = content.replace('core::carrot_text::', 'core::text::')
    content = re.sub(r'\bself::carrot_', 'self::', content)
    content = re.sub(r'\bsuper::carrot_', 'super::', content)
    content = re.sub(r'\bcrate::carrot_', 'crate::', content)
    content = re.sub(r'\bself::inazuma_', 'self::', content)
    content = re.sub(r'\bsuper::inazuma_', 'super::', content)
    content = re.sub(r'\bcrate::inazuma_', 'crate::', content)
    content = content.replace('carrot_carrot_', 'carrot_')
    content = content.replace('inazuma_inazuma_', 'inazuma_')

    return content

files = []
for d in CRATE_DIRS:
    for root, dirs, fnames in os.walk(d):
        for f in fnames:
            if f.endswith('.rs'):
                files.append(os.path.join(root, f))

count = 0
for fpath in files:
    with open(fpath, 'r') as f:
        content = f.read()

    new_content = process_content(content)

    if new_content != content:
        with open(fpath, 'w') as f:
            f.write(new_content)
        count += 1

print(f'Processed {len(files)} files, modified {count}')
