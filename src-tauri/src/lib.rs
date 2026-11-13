mod commands;
mod error;
mod state;

use std::sync::Arc;

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::messages;
use mcp_chat_core::events::CoreEvent;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::{KeyringStore, SecretStore};

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let output_dir = data_dir.join("tool-output");
            std::fs::create_dir_all(&output_dir)?;
            let attachments_dir = data_dir.join("attachments");
            std::fs::create_dir_all(&attachments_dir)?;
            init_logging(&data_dir);

            if let Some(report) = mcp_chat_core::builtin_skills::install_global_skills() {
                if report.changed() {
                    tracing::info!(
                        installed = report.installed,
                        updated = report.updated,
                        "installed bundled skills"
                    );
                }
                if report.failed > 0 {
                    tracing::warn!(
                        failed = report.failed,
                        "some bundled skills could not be installed"
                    );
                }
            }

            let db_path = data_dir.join("mcp-chat.db");
            let db = tauri::async_runtime::block_on(Db::open(&db_path))?;
            tauri::async_runtime::block_on(messages::sweep_interrupted(&db.pool))?;
            tauri::async_runtime::block_on(mcp_chat_core::attachments::sweep_pending(
                &attachments_dir,
            ));

            let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore);
            let manager = Arc::new(McpManager::new(db.clone(), secrets.clone()));
            let engine = ChatEngine::new(
                db.clone(),
                secrets.clone(),
                manager.clone(),
                output_dir.clone(),
                attachments_dir,
            );

            let state = AppState {
                engine: engine.clone(),
                db,
                secrets,
                mcp: manager.clone(),
                data_dir: data_dir.clone(),
                output_dir,
            };
            app.manage(state);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                forward_events(handle, engine).await;
            });
            tauri::async_runtime::spawn(async move {
                manager.refresh_all().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::list_conversations,
            commands::attachments::stage_attachments,
            commands::attachments::stage_attachment_bytes,
            commands::attachments::read_clipboard_image,
            commands::attachments::discard_staged_attachments,
            commands::chat::search_conversations,
            commands::chat::get_conversation,
            commands::chat::create_conversation,
            commands::chat::rename_conversation,
            commands::chat::set_conversation_pinned,
            commands::chat::delete_conversation,
            commands::chat::set_conversation_model,
            commands::chat::set_conversation_reasoning_effort,
            commands::chat::set_conversation_workspace,
            commands::chat::set_system_prompt,
            commands::chat::get_messages,
            commands::chat::get_messages_page,
            commands::chat::send_message,
            commands::chat::steer_message,
            commands::chat::regenerate,
            commands::chat::continue_turn,
            commands::chat::revert_message,
            commands::chat::set_auto_approve,
            commands::chat::get_auto_approve,
            commands::chat::stop_generation,
            commands::chat::is_generating,
            commands::chat::respond_approval,
            commands::chat::respond_input,
            commands::chat::read_tool_output,
            commands::chat::export_conversation,
            commands::chat::get_conversation_tools,
            commands::chat::set_conversation_tools,
            commands::chat::context_status,
            commands::chat::compact_conversation,
            commands::providers::list_providers,
            commands::providers::create_provider,
            commands::providers::update_provider,
            commands::providers::set_provider_key,
            commands::providers::clear_provider_key,
            commands::providers::delete_provider,
            commands::providers::fetch_models,
            commands::providers::fetch_models_for_config,
            commands::providers::test_provider,
            commands::servers::list_servers,
            commands::servers::create_server,
            commands::servers::update_server,
            commands::servers::set_server_enabled,
            commands::servers::set_server_bearer,
            commands::servers::delete_server,
            commands::servers::test_server,
            commands::servers::refresh_server,
            commands::servers::refresh_all_servers,
            commands::servers::list_server_tools,
            commands::servers::run_server_tool,
            commands::servers::list_permission_rules,
            commands::servers::delete_permission_rule,
            commands::settings::app_info,
            commands::settings::list_settings,
            commands::settings::set_setting,
            commands::settings::delete_setting,
            commands::settings::delete_all_data,
            commands::settings::open_external,
            commands::settings::open_data_dir,
            commands::skills::list_skills,
            commands::skills::set_skill_enabled,
            commands::updates::check_for_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mcp-chat");
}

fn init_logging(data_dir: &std::path::Path) {
    let log_dir = data_dir.join("logs");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if std::fs::create_dir_all(&log_dir).is_ok() {
        let appender = tracing_appender::rolling::daily(&log_dir, "mcp-chat.log");
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter.clone())
            .with_writer(appender)
            .with_ansi(false)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .try_init();
    }
}

async fn forward_events(app: tauri::AppHandle, engine: Arc<ChatEngine>) {
    let mut receiver = engine.subscribe();
    loop {
        match receiver.recv().await {
            Ok(event) => {
                let channel = match &event {
                    CoreEvent::Delta { .. } => "chat://delta",
                    CoreEvent::Tool { .. }
                    | CoreEvent::Approval { .. }
                    | CoreEvent::InputRequest { .. }
                    | CoreEvent::Part { .. } => "chat://tool",
                    _ => "chat://status",
                };
                let _ = app.emit(channel, &event);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                let _ = app.emit(
                    "chat://status",
                    &CoreEvent::Resync {
                        reason: format!(
                            "The UI could not keep up with {count} events; reloading the active conversation."
                        ),
                    },
                );
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
