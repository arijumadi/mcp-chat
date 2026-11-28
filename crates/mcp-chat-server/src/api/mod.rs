//! Command dispatch.
//!
//! The web UI talks to the desktop shell through Tauri's `invoke(command, args)`.
//! This module serves the same contract over HTTP: one endpoint, one command
//! name, a JSON object of camelCase arguments, and a JSON result. Keeping the
//! names and shapes identical is what lets the same frontend bundle run in both
//! places with only the transport swapped.

pub mod acp;
pub mod chat;
pub mod misc;
pub mod providers;
pub mod servers;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// The request body for `POST /api/invoke`.
///
/// `call(command, args)` in the frontend becomes one of these, which is why the
/// same component code works against the desktop shell and this server.
#[derive(Debug, Deserialize)]
pub struct InvokeRequest {
    pub command: String,
    #[serde(default)]
    pub args: Value,
}

pub async fn invoke(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InvokeRequest>,
) -> Response {
    match dispatch(&state, &request.command, request.args).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            // Tauri rejects `invoke` with the error string itself, and the UI
            // renders `String(error)`. Returning the bare text keeps the two
            // transports indistinguishable.
            tracing::debug!(command = %request.command, %error, "command failed");
            (StatusCode::BAD_REQUEST, error).into_response()
        }
    }
}

/// Every command the web build supports.
///
/// Anything missing returns `501` from the endpoint, which is deliberate: the
/// browser UI degrades to a toast instead of silently doing nothing.
pub async fn dispatch(state: &AppState, command: &str, args: Value) -> Result<Value, String> {
    match command {
        // --- conversations and turns ---------------------------------------
        "list_conversations" => chat::list_conversations(state, args).await,
        "search_conversations" => chat::search_conversations(state, args).await,
        "get_conversation" => chat::get_conversation(state, args).await,
        "create_conversation" => chat::create_conversation(state, args).await,
        "rename_conversation" => chat::rename_conversation(state, args).await,
        "set_conversation_pinned" => chat::set_conversation_pinned(state, args).await,
        "delete_conversation" => chat::delete_conversation(state, args).await,
        "set_conversation_model" => chat::set_conversation_model(state, args).await,
        "set_conversation_reasoning_effort" => {
            chat::set_conversation_reasoning_effort(state, args).await
        }
        "set_conversation_workspace" => chat::set_conversation_workspace(state, args).await,
        "set_system_prompt" => chat::set_system_prompt(state, args).await,
        "get_messages" => chat::get_messages(state, args).await,
        "get_messages_page" => chat::get_messages_page(state, args).await,
        "send_message" => chat::send_message(state, args).await,
        "steer_message" => chat::steer_message(state, args).await,
        "regenerate" => chat::regenerate(state, args).await,
        "continue_turn" => chat::continue_turn(state, args).await,
        "revert_message" => chat::revert_message(state, args).await,
        "stop_generation" => chat::stop_generation(state, args).await,
        "is_generating" => chat::is_generating(state, args).await,
        "respond_approval" => chat::respond_approval(state, args).await,
        "respond_input" => chat::respond_input(state, args).await,
        "set_auto_approve" => chat::set_auto_approve(state, args).await,
        "get_auto_approve" => chat::get_auto_approve(state, args).await,
        "read_tool_output" => chat::read_tool_output(state, args).await,
        "compact_conversation" => chat::compact_conversation(state, args).await,
        "context_status" => chat::context_status(state, args).await,
        "export_conversation" => chat::export_conversation(state, args).await,
        "get_conversation_tools" => chat::get_conversation_tools(state, args).await,
        "set_conversation_tools" => chat::set_conversation_tools(state, args).await,

        // --- providers ------------------------------------------------------
        "list_providers" => providers::list_providers(state, args).await,
        "create_provider" => providers::create_provider(state, args).await,
        "update_provider" => providers::update_provider(state, args).await,
        "delete_provider" => providers::delete_provider(state, args).await,
        "set_provider_key" => providers::set_provider_key(state, args).await,
        "clear_provider_key" => providers::clear_provider_key(state, args).await,
        "fetch_models" => providers::fetch_models(state, args).await,
        "fetch_models_for_config" => providers::fetch_models_for_config(state, args).await,
        "test_provider" => providers::test_provider(state, args).await,

        // --- MCP servers ----------------------------------------------------
        "list_servers" => servers::list_servers(state, args).await,
        "create_server" => servers::create_server(state, args).await,
        "update_server" => servers::update_server(state, args).await,
        "delete_server" => servers::delete_server(state, args).await,
        "set_server_enabled" => servers::set_server_enabled(state, args).await,
        "set_server_bearer" => servers::set_server_bearer(state, args).await,
        "test_server" => servers::test_server(state, args).await,
        "refresh_server" => servers::refresh_server(state, args).await,
        "refresh_all_servers" => servers::refresh_all_servers(state, args).await,
        "list_server_tools" => servers::list_server_tools(state, args).await,
        "run_server_tool" => servers::run_server_tool(state, args).await,
        "list_permission_rules" => servers::list_permission_rules(state, args).await,
        "delete_permission_rule" => servers::delete_permission_rule(state, args).await,
        "start_server_oauth" => servers::start_server_oauth(state, args).await,
        "oauth_status" => servers::oauth_status(state, args).await,
        "cancel_server_oauth" => servers::cancel_server_oauth(state, args).await,
        "disconnect_server_oauth" => servers::disconnect_server_oauth(state, args).await,

        // --- ACP agents -----------------------------------------------------
        "list_acp_agents" => acp::list_acp_agents(state, args).await,
        "list_acp_presets" => acp::list_acp_presets(state, args).await,
        "list_acp_agents_for_chat" => acp::list_acp_agents_for_chat(state, args).await,
        "create_acp_agent" => acp::create_acp_agent(state, args).await,
        "update_acp_agent" => acp::update_acp_agent(state, args).await,
        "delete_acp_agent" => acp::delete_acp_agent(state, args).await,
        "set_acp_agent_enabled" => acp::set_acp_agent_enabled(state, args).await,
        "test_acp_agent" => acp::test_acp_agent(state, args).await,
        "set_conversation_acp_agent" => acp::set_conversation_acp_agent(state, args).await,
        "get_conversation_acp_agent" => acp::get_conversation_acp_agent(state, args).await,
        "get_agent_commands" => acp::get_agent_commands(state, args).await,
        "get_agent_config" => acp::get_agent_config(state, args).await,
        "list_agent_sessions" => acp::list_agent_sessions(state, args).await,
        "attach_agent_session" => acp::attach_agent_session(state, args).await,
        "set_agent_config" => acp::set_agent_config(state, args).await,

        // --- workspace, attachments, settings, skills, updates --------------
        "list_workspace_dir" => misc::list_workspace_dir(state, args).await,
        "read_workspace_file" => misc::read_workspace_file(state, args).await,
        "stage_attachments" => misc::stage_attachments(state, args).await,
        "stage_attachment_bytes" => misc::stage_attachment_bytes(state, args).await,
        "stage_attachment_upload" => misc::stage_attachment_upload(state, args).await,
        "discard_staged_attachments" => misc::discard_staged_attachments(state, args).await,
        "list_settings" => misc::list_settings(state, args).await,
        "set_setting" => misc::set_setting(state, args).await,
        "delete_setting" => misc::delete_setting(state, args).await,
        "delete_all_data" => misc::delete_all_data(state, args).await,
        "app_info" => misc::app_info(state, args).await,
        "list_skills" => misc::list_skills(state, args).await,
        "set_skill_enabled" => misc::set_skill_enabled(state, args).await,
        "check_for_update" => misc::check_for_update(state, args).await,

        // The two below cannot mean anything on a host with no desktop. They
        // are listed so the error is explicit rather than a bare 501, and the
        // frontend handles both without a server round trip.
        "open_external" => misc::open_external(state, args).await,
        "open_data_dir" => Err("There is no file manager on this host.".into()),
        "read_clipboard_image" => {
            Err("The clipboard belongs to the browser; paste the image instead.".into())
        }

        other => Err(format!("`{other}` is not available in server mode.")),
    }
}

/// Deserialize the whole argument object, mirroring how Tauri maps a JS object
/// onto the command's parameters.
pub(crate) fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|error| format!("Invalid arguments: {error}"))
}

pub(crate) fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| format!("Could not encode the result: {error}"))
}

pub(crate) fn err(error: mcp_chat_core::CoreError) -> String {
    tracing::debug!(error = ?error, "command failed");
    error.user_message()
}
