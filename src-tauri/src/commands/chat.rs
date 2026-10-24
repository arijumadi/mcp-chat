use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use mcp_chat_core::agent::approvals::{ApprovalAnswer, ApprovalDecision, InputAnswer};
use mcp_chat_core::agent::runner;
use mcp_chat_core::db::repo::{
    conversation_servers, conversations, messages, providers, servers, settings,
};
use mcp_chat_core::models::{Conversation, MessageWithParts, SearchHit};

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationInput {
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub workspace_root: Option<String>,
}

#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> CmdResult<Vec<Conversation>> {
    conversations::list(&state.db.pool, limit.unwrap_or(200), offset.unwrap_or(0))
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn search_conversations(
    state: State<'_, AppState>,
    query: String,
) -> CmdResult<Vec<SearchHit>> {
    conversations::search(&state.db.pool, &query, 30)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn get_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Conversation> {
    conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppState>,
    input: CreateConversationInput,
) -> CmdResult<Conversation> {
    let provider_id = match input.provider_id {
        Some(id) => Some(id),
        None => match settings::get(&state.db.pool, "last_provider_id")
            .await
            .map_err(cmd_err)?
        {
            Some(id) => Some(id),
            None => providers::list(&state.db.pool)
                .await
                .map_err(cmd_err)?
                .first()
                .map(|provider| provider.id.clone()),
        },
    };
    let mut model = input.model;
    if model.is_none() {
        if let Some(provider_id) = provider_id.as_deref() {
            if let Ok(provider) = providers::get(&state.db.pool, provider_id).await {
                model = Some(provider.model);
            }
        }
    }
    let explicit_workspace = match input
        .workspace_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(raw) => Some(
            mcp_chat_core::agent::tools::resolve_workspace_path(raw)
                .map_err(cmd_err)?
                .display()
                .to_string(),
        ),
        None => None,
    };
    let inherited_workspace = if explicit_workspace.is_none() {
        settings::get(&state.db.pool, "workspace.last")
            .await
            .map_err(cmd_err)?
            .or(settings::get(&state.db.pool, "workspace.root")
                .await
                .map_err(cmd_err)?)
            .and_then(|raw| {
                mcp_chat_core::agent::tools::resolve_workspace_path(&raw)
                    .ok()
                    .map(|path| path.display().to_string())
            })
    } else {
        None
    };
    let workspace = explicit_workspace.or(inherited_workspace);
    let mut conversation = conversations::create(
        &state.db.pool,
        "New chat",
        provider_id.as_deref(),
        model.as_deref(),
    )
    .await
    .map_err(cmd_err)?;
    if let Some(value) = workspace {
        conversations::set_workspace(&state.db.pool, &conversation.id, Some(&value))
            .await
            .map_err(cmd_err)?;
        conversation.workspace_root = Some(value);
    }
    Ok(conversation)
}

#[tauri::command]
pub async fn rename_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
    title: String,
) -> CmdResult<()> {
    conversations::update_title(&state.db.pool, &conversation_id, &title)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn set_conversation_pinned(
    state: State<'_, AppState>,
    conversation_id: String,
    pinned: bool,
) -> CmdResult<()> {
    conversations::set_pinned(&state.db.pool, &conversation_id, pinned)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn delete_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<()> {
    state.engine.stop(&conversation_id);
    state.engine.set_auto_approve(&conversation_id, false);
    let history = messages::list_for_conversation(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    for message in &history {
        for part in &message.parts {
            if part.kind != "tool_call" {
                continue;
            }
            if let Some(call_id) = part.payload().get("callId").and_then(Value::as_str) {
                let _ =
                    mcp_chat_core::agent::tools::remove_spilled(&state.output_dir, call_id).await;
            }
        }
    }
    conversations::delete(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    mcp_chat_core::attachments::delete_conversation(
        &state.engine.attachments_dir,
        &conversation_id,
    )
    .await;
    Ok(())
}

#[tauri::command]
pub async fn set_conversation_model(
    state: State<'_, AppState>,
    conversation_id: String,
    provider_id: String,
    model: String,
) -> CmdResult<()> {
    settings::set(&state.db.pool, "last_provider_id", &provider_id)
        .await
        .map_err(cmd_err)?;
    conversations::set_model(
        &state.db.pool,
        &conversation_id,
        Some(&provider_id),
        Some(&model),
    )
    .await
    .map_err(cmd_err)
}

#[tauri::command]
pub async fn set_conversation_workspace(
    state: State<'_, AppState>,
    conversation_id: String,
    workspace_root: String,
) -> CmdResult<String> {
    if state.engine.in_flight(&conversation_id) {
        return Err("Stop the current turn before changing the workspace.".into());
    }
    conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    let path =
        mcp_chat_core::agent::tools::resolve_workspace_path(&workspace_root).map_err(cmd_err)?;
    let value = path.display().to_string();
    conversations::set_workspace(&state.db.pool, &conversation_id, Some(&value))
        .await
        .map_err(cmd_err)?;
    settings::set(&state.db.pool, "workspace.last", &value)
        .await
        .map_err(cmd_err)?;
    let mut recent: Vec<String> = settings::get(&state.db.pool, "workspace.recent")
        .await
        .map_err(cmd_err)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    recent.retain(|item| item != &value);
    recent.insert(0, value.clone());
    recent.truncate(8);
    settings::set(
        &state.db.pool,
        "workspace.recent",
        &serde_json::to_string(&recent).unwrap_or_else(|_| "[]".into()),
    )
    .await
    .map_err(cmd_err)?;
    Ok(value)
}

#[tauri::command]
pub async fn set_system_prompt(
    state: State<'_, AppState>,
    conversation_id: String,
    prompt: Option<String>,
) -> CmdResult<()> {
    conversations::set_system_prompt(&state.db.pool, &conversation_id, prompt.as_deref())
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Vec<MessageWithParts>> {
    messages::list_for_conversation(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    conversation_id: String,
    text: String,
    attachment_ids: Option<Vec<String>>,
) -> CmdResult<MessageWithParts> {
    let message = state
        .engine
        .send_message_with_attachments(
            &conversation_id,
            &text,
            attachment_ids.as_deref().unwrap_or(&[]),
        )
        .await
        .map_err(cmd_err)?;
    let parts = messages::parts_for_message(&state.db.pool, &message.id)
        .await
        .map_err(cmd_err)?;
    Ok(MessageWithParts { message, parts })
}

#[tauri::command]
pub async fn regenerate(state: State<'_, AppState>, conversation_id: String) -> CmdResult<()> {
    state
        .engine
        .regenerate(&conversation_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn continue_turn(state: State<'_, AppState>, conversation_id: String) -> CmdResult<()> {
    state
        .engine
        .continue_turn(&conversation_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn revert_message(
    state: State<'_, AppState>,
    conversation_id: String,
    message_id: String,
) -> CmdResult<mcp_chat_core::agent::RevertedMessage> {
    state
        .engine
        .revert_message(&conversation_id, &message_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn set_auto_approve(
    state: State<'_, AppState>,
    conversation_id: String,
    enabled: bool,
) -> CmdResult<bool> {
    conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    state.engine.set_auto_approve(&conversation_id, enabled);
    Ok(state.engine.auto_approve(&conversation_id))
}

#[tauri::command]
pub async fn get_auto_approve(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<bool> {
    Ok(state.engine.auto_approve(&conversation_id))
}

#[tauri::command]
pub async fn stop_generation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<bool> {
    Ok(state.engine.stop(&conversation_id))
}

#[tauri::command]
pub async fn is_generating(state: State<'_, AppState>, conversation_id: String) -> CmdResult<bool> {
    Ok(state.engine.in_flight(&conversation_id))
}

#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AppState>,
    call_id: String,
    decision: String,
    feedback: Option<String>,
) -> CmdResult<bool> {
    let decision = match decision.as_str() {
        "once" => ApprovalDecision::Once,
        "always" => ApprovalDecision::Always,
        "reject" => ApprovalDecision::Reject,
        _ => return Err("Unknown approval decision".into()),
    };
    Ok(state
        .engine
        .respond_approval(&call_id, ApprovalAnswer { decision, feedback }))
}

#[tauri::command]
pub async fn respond_input(
    state: State<'_, AppState>,
    call_id: String,
    responses: Value,
    cancelled: bool,
) -> CmdResult<bool> {
    Ok(state.engine.respond_input(
        &call_id,
        InputAnswer {
            responses,
            cancelled,
        },
    ))
}

#[tauri::command]
pub async fn read_tool_output(
    state: State<'_, AppState>,
    call_id: String,
    offset: Option<u64>,
) -> CmdResult<String> {
    state
        .engine
        .read_tool_output(&call_id, offset.unwrap_or(0) as usize)
        .await
        .map_err(cmd_err)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub filename: String,
    pub content: String,
    pub mime: String,
}

#[tauri::command]
pub async fn export_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
    format: String,
) -> CmdResult<ExportResult> {
    let conversation = conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    let history = messages::list_for_conversation(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    if format == "json" {
        let value = serde_json::json!({"conversation": conversation, "messages": history});
        let content = serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?;
        return Ok(ExportResult {
            filename: format!("{}.json", safe_filename(&conversation.title)),
            content,
            mime: "application/json".into(),
        });
    }
    if format != "markdown" {
        return Err("Unsupported export format".into());
    }
    let mut out = format!(
        "# {}\n\n_Model: {} · Exported from Jarvis_\n\n",
        conversation.title,
        conversation
            .model
            .clone()
            .unwrap_or_else(|| "unknown".into())
    );
    for message in history {
        if message.message.status == "synthetic" || message.message.role == "tool" {
            continue;
        }
        let role = match message.message.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            "system" => "System",
            other => other,
        };
        out.push_str(&format!("## {role}\n\n"));
        for part in &message.parts {
            let payload = part.payload();
            match part.kind.as_str() {
                "text" => {
                    if let Some(text) = payload.get("text").and_then(Value::as_str) {
                        out.push_str(text);
                        out.push_str("\n\n");
                    }
                }
                "reasoning" => {
                    if let Some(text) = payload.get("text").and_then(Value::as_str) {
                        out.push_str("<details><summary>Reasoning</summary>\n\n");
                        out.push_str(text);
                        out.push_str("\n\n</details>\n\n");
                    }
                }
                "tool_call" => {
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let server = payload.get("server").and_then(Value::as_str).unwrap_or("");
                    let state = payload.get("state").and_then(Value::as_str).unwrap_or("");
                    out.push_str(&format!("**Tool `{name}`** on {server} · {state}\n\n"));
                    if let Some(args) = payload.get("args") {
                        out.push_str("```json\n");
                        out.push_str(&serde_json::to_string_pretty(args).unwrap_or_default());
                        out.push_str("\n```\n\n");
                    }
                }
                "attachment" => {
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("attachment");
                    let file_kind = payload
                        .get("fileKind")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let mime = payload.get("mime").and_then(Value::as_str).unwrap_or("");
                    let size = payload.get("size").and_then(Value::as_i64).unwrap_or(0);
                    out.push_str(&format!(
                        "_Attachment: `{name}` ({file_kind}, {mime}, {size} bytes)_\n\n"
                    ));
                }
                "error" => {
                    if let Some(text) = payload.get("message").and_then(Value::as_str) {
                        out.push_str(&format!("> Error: {text}\n\n"));
                    }
                }
                "summary" => {
                    if let Some(text) = payload.get("text").and_then(Value::as_str) {
                        out.push_str(&format!("> Summary: {text}\n\n"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(ExportResult {
        filename: format!("{}.md", safe_filename(&conversation.title)),
        content: out,
        mime: "text/markdown".into(),
    })
}

fn safe_filename(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars().take(60) {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim().replace(' ', "-");
    if trimmed.is_empty() {
        "conversation".into()
    } else {
        trimmed
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationToolsView {
    pub server_ids: Vec<String>,
    pub all_servers: bool,
    pub tool_mode: String,
}

#[tauri::command]
pub async fn get_conversation_tools(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<ConversationToolsView> {
    let conversation = conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    let server_ids = if conversation.servers_configured {
        conversation_servers::selected(&state.db.pool, &conversation_id)
            .await
            .map_err(cmd_err)?
    } else {
        servers::list_enabled(&state.db.pool)
            .await
            .map_err(cmd_err)?
            .into_iter()
            .map(|server| server.id)
            .collect()
    };
    Ok(ConversationToolsView {
        server_ids,
        all_servers: !conversation.servers_configured,
        tool_mode: conversation.tool_mode,
    })
}

#[tauri::command]
pub async fn set_conversation_tools(
    state: State<'_, AppState>,
    conversation_id: String,
    server_ids: Option<Vec<String>>,
    tool_mode: String,
) -> CmdResult<()> {
    if tool_mode != "direct" && tool_mode != "lazy" {
        return Err("Unknown tool mode".into());
    }
    match server_ids {
        None => {
            conversation_servers::reset_to_all(&state.db.pool, &conversation_id)
                .await
                .map_err(cmd_err)?;
        }
        Some(ids) => {
            let known = servers::list(&state.db.pool)
                .await
                .map_err(cmd_err)?
                .into_iter()
                .map(|server| server.id)
                .collect::<std::collections::HashSet<_>>();
            if let Some(unknown) = ids.iter().find(|id| !known.contains(*id)) {
                return Err(format!("Unknown MCP server id {unknown:?}"));
            }
            conversation_servers::set(&state.db.pool, &conversation_id, &ids)
                .await
                .map_err(cmd_err)?;
        }
    }
    conversations::set_tool_mode(&state.db.pool, &conversation_id, &tool_mode)
        .await
        .map_err(cmd_err)?;
    let _ = state
        .engine
        .events
        .send(mcp_chat_core::events::CoreEvent::ConversationUpdated { conversation_id });
    Ok(())
}

#[tauri::command]
pub async fn context_status(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<runner::ContextStatus> {
    runner::context_status(state.engine.clone(), &conversation_id)
        .await
        .map_err(cmd_err)
}

#[tauri::command]
pub async fn compact_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<()> {
    runner::compact_now(state.engine.clone(), &conversation_id)
        .await
        .map_err(cmd_err)
}
