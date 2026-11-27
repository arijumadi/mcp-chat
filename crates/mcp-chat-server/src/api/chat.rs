//! Conversations, turns, and everything around them.
//!
//! These bodies are ported from `src-tauri/src/commands/chat.rs` unchanged in
//! behaviour. The only difference is the state parameter: a plain `&AppState`
//! instead of Tauri's extractor.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use mcp_chat_core::agent::approvals::{ApprovalAnswer, ApprovalDecision, InputAnswer};
use mcp_chat_core::agent::runner;
use mcp_chat_core::db::repo::{
    conversation_servers, conversations, messages, providers, servers, settings,
};
use mcp_chat_core::models::{
    Conversation, MessageContext, MessagePage, MessageWithParts, SearchHit,
};

use super::{err, parse, to_value};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListConversations {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

pub async fn list_conversations(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ListConversations = parse(args)?;
    let list = conversations::list(
        &state.db.pool,
        params.limit.unwrap_or(200),
        params.offset.unwrap_or(0),
    )
    .await
    .map_err(err)?;
    to_value(list)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchConversations {
    query: String,
}

pub async fn search_conversations(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SearchConversations = parse(args)?;
    let hits: Vec<SearchHit> = conversations::search(&state.db.pool, &params.query, 30)
        .await
        .map_err(err)?;
    to_value(hits)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ByConversation {
    conversation_id: String,
}

pub async fn get_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let conversation: Conversation = conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    to_value(conversation)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateConversationInput {
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateConversation {
    input: CreateConversationInput,
}

pub async fn create_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: CreateConversation = parse(args)?;
    let input = params.input;
    let provider_id = match input.provider_id {
        Some(id) => Some(id),
        None => match settings::get(&state.db.pool, "last_provider_id")
            .await
            .map_err(err)?
        {
            Some(id) => Some(id),
            None => providers::list(&state.db.pool)
                .await
                .map_err(err)?
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
                .map_err(err)?
                .display()
                .to_string(),
        ),
        None => None,
    };
    let inherited_workspace = if explicit_workspace.is_none() {
        settings::get(&state.db.pool, "workspace.last")
            .await
            .map_err(err)?
            .or(settings::get(&state.db.pool, "workspace.root")
                .await
                .map_err(err)?)
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
    .map_err(err)?;
    if let Some(value) = workspace {
        conversations::set_workspace(&state.db.pool, &conversation.id, Some(&value))
            .await
            .map_err(err)?;
        conversation.workspace_root = Some(value);
    }
    to_value(conversation)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameConversation {
    conversation_id: String,
    title: String,
}

pub async fn rename_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: RenameConversation = parse(args)?;
    conversations::update_title(&state.db.pool, &params.conversation_id, &params.title)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinConversation {
    conversation_id: String,
    pinned: bool,
}

pub async fn set_conversation_pinned(state: &AppState, args: Value) -> Result<Value, String> {
    let params: PinConversation = parse(args)?;
    conversations::set_pinned(&state.db.pool, &params.conversation_id, params.pinned)
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn delete_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let conversation_id = params.conversation_id;
    state.engine.stop(&conversation_id);
    state.engine.close_acp_session(&conversation_id).await;
    state.engine.set_auto_approve(&conversation_id, false);
    let history = messages::list_for_conversation(&state.db.pool, &conversation_id)
        .await
        .map_err(err)?;
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
        .map_err(err)?;
    mcp_chat_core::attachments::delete_conversation(
        &state.engine.attachments_dir,
        &conversation_id,
    )
    .await;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModel {
    conversation_id: String,
    provider_id: String,
    model: String,
}

pub async fn set_conversation_model(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetModel = parse(args)?;
    settings::set(&state.db.pool, "last_provider_id", &params.provider_id)
        .await
        .map_err(err)?;
    conversations::set_model(
        &state.db.pool,
        &params.conversation_id,
        Some(&params.provider_id),
        Some(&params.model),
    )
    .await
    .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEffort {
    conversation_id: String,
    effort: String,
}

pub async fn set_conversation_reasoning_effort(
    state: &AppState,
    args: Value,
) -> Result<Value, String> {
    let params: SetEffort = parse(args)?;
    if !matches!(
        params.effort.as_str(),
        "default" | "low" | "medium" | "high" | "max"
    ) {
        return Err("Unknown reasoning effort".into());
    }
    conversations::set_reasoning_effort(&state.db.pool, &params.conversation_id, &params.effort)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetWorkspace {
    conversation_id: String,
    workspace_root: String,
}

pub async fn set_conversation_workspace(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetWorkspace = parse(args)?;
    if state.engine.in_flight(&params.conversation_id) {
        return Err("Stop the current turn before changing the workspace.".into());
    }
    conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    let path =
        mcp_chat_core::agent::tools::resolve_workspace_path(&params.workspace_root).map_err(err)?;
    let value = path.display().to_string();
    // An ACP session's working directory is fixed when it is opened: the
    // protocol has no way to move it. Reusing the session after a workspace
    // change would keep the agent in the old folder, so drop it and let the
    // next message open a fresh one in the new place.
    state
        .engine
        .close_acp_session(&params.conversation_id)
        .await;
    conversations::set_acp_session_id(&state.db.pool, &params.conversation_id, "")
        .await
        .ok();
    conversations::set_workspace(&state.db.pool, &params.conversation_id, Some(&value))
        .await
        .map_err(err)?;
    settings::set(&state.db.pool, "workspace.last", &value)
        .await
        .map_err(err)?;
    // Re-open in the new folder so the pickers keep their options instead of
    // staying empty until the next message.
    state
        .engine
        .reopen_acp_session_if_bound(&params.conversation_id)
        .await;
    let mut recent: Vec<String> = settings::get(&state.db.pool, "workspace.recent")
        .await
        .map_err(err)?
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
    .map_err(err)?;
    to_value(value)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSystemPrompt {
    conversation_id: String,
    #[serde(default)]
    prompt: Option<String>,
}

pub async fn set_system_prompt(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetSystemPrompt = parse(args)?;
    conversations::set_system_prompt(
        &state.db.pool,
        &params.conversation_id,
        params.prompt.as_deref(),
    )
    .await
    .map_err(err)?;
    to_value(())
}

pub async fn get_messages(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let list = messages::list_for_conversation(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    to_value(list)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagesPage {
    conversation_id: String,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

pub async fn get_messages_page(state: &AppState, args: Value) -> Result<Value, String> {
    let params: MessagesPage = parse(args)?;
    let page: MessagePage = messages::list_page_for_conversation(
        &state.db.pool,
        &params.conversation_id,
        params.before.as_deref(),
        params.limit.unwrap_or(80) as usize,
    )
    .await
    .map_err(err)?;
    to_value(page)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessage {
    conversation_id: String,
    text: String,
    #[serde(default)]
    attachment_ids: Option<Vec<String>>,
    #[serde(default)]
    contexts: Option<Vec<MessageContext>>,
}

pub async fn send_message(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SendMessage = parse(args)?;
    let message = state
        .engine
        .send_message_with_contexts(
            &params.conversation_id,
            &params.text,
            params.attachment_ids.as_deref().unwrap_or(&[]),
            params.contexts.as_deref().unwrap_or(&[]),
        )
        .await
        .map_err(err)?;
    let parts = messages::parts_for_message(&state.db.pool, &message.id)
        .await
        .map_err(err)?;
    to_value(MessageWithParts { message, parts })
}

pub async fn steer_message(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SendMessage = parse(args)?;
    let message = state
        .engine
        .steer_message_with_contexts(
            &params.conversation_id,
            &params.text,
            params.attachment_ids.as_deref().unwrap_or(&[]),
            params.contexts.as_deref().unwrap_or(&[]),
        )
        .await
        .map_err(err)?;
    let parts = messages::parts_for_message(&state.db.pool, &message.id)
        .await
        .map_err(err)?;
    to_value(MessageWithParts { message, parts })
}

pub async fn regenerate(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    state
        .engine
        .regenerate(&params.conversation_id)
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn continue_turn(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    state
        .engine
        .continue_turn(&params.conversation_id)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertMessage {
    conversation_id: String,
    message_id: String,
}

pub async fn revert_message(state: &AppState, args: Value) -> Result<Value, String> {
    let params: RevertMessage = parse(args)?;
    let reverted = state
        .engine
        .revert_message(&params.conversation_id, &params.message_id)
        .await
        .map_err(err)?;
    to_value(reverted)
}

pub async fn stop_generation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    to_value(state.engine.stop(&params.conversation_id))
}

pub async fn is_generating(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    to_value(state.engine.in_flight(&params.conversation_id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondApproval {
    call_id: String,
    decision: String,
    #[serde(default)]
    feedback: Option<String>,
}

pub async fn respond_approval(state: &AppState, args: Value) -> Result<Value, String> {
    let params: RespondApproval = parse(args)?;
    let decision = match params.decision.as_str() {
        "once" => ApprovalDecision::Once,
        "always" => ApprovalDecision::Always,
        "reject" => ApprovalDecision::Reject,
        other => return Err(format!("Unknown approval decision {other:?}")),
    };
    to_value(state.engine.respond_approval(
        &params.call_id,
        ApprovalAnswer {
            decision,
            feedback: params.feedback,
        },
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondInput {
    call_id: String,
    responses: Value,
    cancelled: bool,
}

pub async fn respond_input(state: &AppState, args: Value) -> Result<Value, String> {
    let params: RespondInput = parse(args)?;
    to_value(state.engine.respond_input(
        &params.call_id,
        InputAnswer {
            responses: params.responses,
            cancelled: params.cancelled,
        },
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoApprove {
    conversation_id: String,
    #[serde(default)]
    enabled: bool,
}

pub async fn set_auto_approve(state: &AppState, args: Value) -> Result<Value, String> {
    let params: AutoApprove = parse(args)?;
    conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    state
        .engine
        .set_auto_approve(&params.conversation_id, params.enabled);
    to_value(state.engine.auto_approve(&params.conversation_id))
}

pub async fn get_auto_approve(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    to_value(state.engine.auto_approve(&params.conversation_id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadToolOutput {
    call_id: String,
    #[serde(default)]
    offset: Option<u64>,
}

pub async fn read_tool_output(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ReadToolOutput = parse(args)?;
    let text = state
        .engine
        .read_tool_output(&params.call_id, params.offset.unwrap_or(0) as usize)
        .await
        .map_err(err)?;
    to_value(text)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Compact {
    conversation_id: String,
    #[serde(default)]
    focus: Option<String>,
}

pub async fn compact_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: Compact = parse(args)?;
    runner::compact_now(
        state.engine.clone(),
        &params.conversation_id,
        params.focus.as_deref(),
    )
    .await
    .map_err(err)?;
    to_value(())
}

pub async fn context_status(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let status = runner::context_status(state.engine.clone(), &params.conversation_id)
        .await
        .map_err(err)?;
    to_value(status)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    filename: String,
    content: String,
    mime: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportConversation {
    conversation_id: String,
    format: String,
}

pub async fn export_conversation(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ExportConversation = parse(args)?;
    let conversation = conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    let history = messages::list_for_conversation(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    if params.format == "json" {
        let value = serde_json::json!({"conversation": conversation, "messages": history});
        let content = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
        return to_value(ExportResult {
            filename: format!("{}.json", safe_filename(&conversation.title)),
            content,
            mime: "application/json".into(),
        });
    }
    if params.format != "markdown" {
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
    to_value(ExportResult {
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
struct ConversationToolsView {
    server_ids: Vec<String>,
    all_servers: bool,
    tool_mode: String,
}

pub async fn get_conversation_tools(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let conversation = conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    let server_ids = if conversation.servers_configured {
        conversation_servers::selected(&state.db.pool, &params.conversation_id)
            .await
            .map_err(err)?
    } else {
        servers::list_enabled(&state.db.pool)
            .await
            .map_err(err)?
            .into_iter()
            .map(|server| server.id)
            .collect()
    };
    to_value(ConversationToolsView {
        server_ids,
        all_servers: !conversation.servers_configured,
        tool_mode: conversation.tool_mode,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetConversationTools {
    conversation_id: String,
    #[serde(default)]
    server_ids: Option<Vec<String>>,
    tool_mode: String,
}

pub async fn set_conversation_tools(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetConversationTools = parse(args)?;
    if params.tool_mode != "direct" && params.tool_mode != "lazy" {
        return Err("Unknown tool mode".into());
    }
    match params.server_ids {
        None => {
            conversation_servers::reset_to_all(&state.db.pool, &params.conversation_id)
                .await
                .map_err(err)?;
        }
        Some(ids) => {
            let known = servers::list(&state.db.pool)
                .await
                .map_err(err)?
                .into_iter()
                .map(|server| server.id)
                .collect::<std::collections::HashSet<_>>();
            if let Some(unknown) = ids.iter().find(|id| !known.contains(*id)) {
                return Err(format!("Unknown MCP server id {unknown:?}"));
            }
            conversation_servers::set(&state.db.pool, &params.conversation_id, &ids)
                .await
                .map_err(err)?;
        }
    }
    conversations::set_tool_mode(&state.db.pool, &params.conversation_id, &params.tool_mode)
        .await
        .map_err(err)?;
    let _ = state
        .engine
        .events
        .send(mcp_chat_core::events::CoreEvent::ConversationUpdated {
            conversation_id: params.conversation_id,
        });
    to_value(())
}
