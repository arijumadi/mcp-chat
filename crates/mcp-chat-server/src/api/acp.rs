//! ACP agent registry and per-chat agent options.
//!
//! Ported from `src-tauri/src/commands/acp.rs`. Env values are stored but never
//! returned: the same rule the desktop applies, because they can hold keys.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use mcp_chat_core::acp::config::{AgentPreset, PRESETS};
use mcp_chat_core::acp::{AcpProbe, ConfigOption};
use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::repo::acp_agents::{self, AcpAgentConfig};
use mcp_chat_core::db::repo::conversations;
use mcp_chat_core::models::AcpAgent;

use super::{err, parse, to_value};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpAgentView {
    id: String,
    name: String,
    slug: String,
    command: String,
    args: Vec<String>,
    env_keys: Vec<String>,
    permission_policy: String,
    enabled: bool,
    last_error: Option<String>,
    last_checked_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentCommandView {
    name: String,
    description: String,
    input_hint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentConfigChoiceView {
    value: String,
    name: String,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentConfigOptionView {
    id: String,
    name: String,
    description: Option<String>,
    category: Option<String>,
    current_value: String,
    choices: Vec<AgentConfigChoiceView>,
    kind: String,
}

fn config_view(options: Vec<ConfigOption>) -> Vec<AgentConfigOptionView> {
    options
        .into_iter()
        .map(|option| AgentConfigOptionView {
            id: option.id,
            name: option.name,
            description: option.description,
            category: option.category,
            current_value: option.current_value,
            kind: option.kind,
            choices: option
                .choices
                .into_iter()
                .map(|choice| AgentConfigChoiceView {
                    value: choice.value,
                    name: choice.name,
                    description: choice.description,
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpPresetView {
    name: String,
    slug: String,
    command: String,
    args: Vec<String>,
    notes: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcpAgentInput {
    name: String,
    slug: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Option<BTreeMap<String, String>>,
    permission_policy: String,
    enabled: bool,
}

fn view(agent: &AcpAgent) -> AcpAgentView {
    AcpAgentView {
        id: agent.id.clone(),
        name: agent.name.clone(),
        slug: agent.slug.clone(),
        command: agent.command.clone(),
        args: acp_agents::agent_args(agent).unwrap_or_default(),
        env_keys: acp_agents::agent_env(agent)
            .map(|pairs| pairs.into_iter().map(|(key, _)| key).collect())
            .unwrap_or_default(),
        permission_policy: agent.permission_policy.clone(),
        enabled: agent.enabled,
        last_error: agent.last_error.clone(),
        last_checked_at: agent.last_checked_at,
    }
}

fn to_config(input: &AcpAgentInput, env_json: String) -> Result<AcpAgentConfig, String> {
    let args_json = serde_json::to_string(&input.args)
        .map_err(|_| "The arguments could not be encoded.".to_string())?;
    Ok(AcpAgentConfig {
        name: input.name.clone(),
        slug: input.slug.clone(),
        command: input.command.clone(),
        args_json,
        env_json,
        permission_policy: input.permission_policy.clone(),
        enabled: input.enabled,
    })
}

pub async fn list_acp_agents(state: &AppState, _args: Value) -> Result<Value, String> {
    let list = acp_agents::list(&state.db.pool).await.map_err(err)?;
    to_value(list.iter().map(view).collect::<Vec<_>>())
}

pub async fn list_acp_presets(_state: &AppState, _args: Value) -> Result<Value, String> {
    let presets = PRESETS
        .iter()
        .map(|preset: &AgentPreset| AcpPresetView {
            name: preset.name.to_owned(),
            slug: preset.slug.to_owned(),
            command: preset.command.to_owned(),
            args: preset.args.iter().map(|arg| (*arg).to_owned()).collect(),
            notes: preset.notes.to_owned(),
        })
        .collect::<Vec<_>>();
    to_value(presets)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAcpAgent {
    input: AcpAgentInput,
}

pub async fn create_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: CreateAcpAgent = parse(args)?;
    let env_json = match &params.input.env {
        Some(env) => serde_json::to_string(env).map_err(|_| "Invalid env.".to_string())?,
        None => "{}".into(),
    };
    let config = to_config(&params.input, env_json)?;
    let agent = acp_agents::create(&state.db.pool, &config)
        .await
        .map_err(err)?;
    to_value(view(&agent))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAcpAgent {
    id: String,
    input: AcpAgentInput,
}

pub async fn update_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: UpdateAcpAgent = parse(args)?;
    // Keep stored env when the caller did not supply one: the UI never sees the
    // values, so it cannot round-trip them.
    let env_json = match &params.input.env {
        Some(env) => serde_json::to_string(env).map_err(|_| "Invalid env.".to_string())?,
        None => {
            let current = acp_agents::get(&state.db.pool, &params.id)
                .await
                .map_err(err)?;
            current.env_json
        }
    };
    let config = to_config(&params.input, env_json)?;
    let agent = acp_agents::update(&state.db.pool, &params.id, &config)
        .await
        .map_err(err)?;
    to_value(view(&agent))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcpAgentId {
    id: String,
}

pub async fn delete_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: AcpAgentId = parse(args)?;
    acp_agents::delete(&state.db.pool, &params.id)
        .await
        .map_err(err)?;
    to_value(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAcpEnabled {
    id: String,
    enabled: bool,
}

pub async fn set_acp_agent_enabled(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetAcpEnabled = parse(args)?;
    let agent = acp_agents::set_enabled(&state.db.pool, &params.id, params.enabled)
        .await
        .map_err(err)?;
    to_value(view(&agent))
}

pub async fn test_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: AcpAgentId = parse(args)?;
    let agent = acp_agents::get(&state.db.pool, &params.id)
        .await
        .map_err(err)?;
    let workspace = mcp_chat_core::agent::tools::workspace_root(&state.db.pool)
        .await
        .map_err(err)?;
    let probe: AcpProbe = match ChatEngine::probe_acp_agent(&agent, &workspace).await {
        Ok(probe) => {
            let _ = acp_agents::set_health(
                &state.db.pool,
                &params.id,
                if probe.ok { None } else { Some(&probe.message) },
            )
            .await;
            probe
        }
        Err(error) => {
            let message = error.user_message();
            let _ = acp_agents::set_health(&state.db.pool, &params.id, Some(&message)).await;
            AcpProbe {
                ok: false,
                message,
                session_id: None,
                agent_name: None,
                agent_version: None,
            }
        }
    };
    to_value(probe)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetConversationAgent {
    conversation_id: String,
    #[serde(default)]
    slug: Option<String>,
}

pub async fn set_conversation_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetConversationAgent = parse(args)?;
    state
        .engine
        .set_conversation_agent(&params.conversation_id, params.slug.as_deref())
        .await
        .map_err(err)?;
    to_value(())
}

pub async fn list_acp_agents_for_chat(state: &AppState, _args: Value) -> Result<Value, String> {
    let list = acp_agents::list_enabled(&state.db.pool)
        .await
        .map_err(err)?;
    to_value(list.iter().map(view).collect::<Vec<_>>())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ByConversation {
    conversation_id: String,
}

pub async fn get_conversation_acp_agent(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let conversation = conversations::get(&state.db.pool, &params.conversation_id)
        .await
        .map_err(err)?;
    to_value(conversation.acp_agent)
}

pub async fn get_agent_commands(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let commands = state.engine.agent_commands(&params.conversation_id).await;
    to_value(
        commands
            .into_iter()
            .map(|command| AgentCommandView {
                name: command.name,
                description: command.description,
                input_hint: command.input_hint,
            })
            .collect::<Vec<_>>(),
    )
}

pub async fn get_agent_config(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ByConversation = parse(args)?;
    let options = state.engine.agent_config(&params.conversation_id).await;
    to_value(config_view(options))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetAgentConfig {
    conversation_id: String,
    config_id: String,
    value: String,
}

pub async fn set_agent_config(state: &AppState, args: Value) -> Result<Value, String> {
    let params: SetAgentConfig = parse(args)?;
    let options = state
        .engine
        .set_agent_config(&params.conversation_id, &params.config_id, &params.value)
        .await
        .map_err(err)?;
    to_value(config_view(options))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAgentSessions {
    /// Registry id of the agent to ask.
    agent_id: String,
    /// Filter. Omitted means "every session the agent will show".
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

/// List the sessions an agent already has, including the ones Jarvis did not
/// create.
///
/// Read-only: the agent is started, asked, and stopped. Nothing is bound to a
/// conversation -- that is a separate step, and it must use the `cwd` reported
/// here rather than the chat's workspace.
pub async fn list_agent_sessions(state: &AppState, args: Value) -> Result<Value, String> {
    let params: ListAgentSessions = parse(args)?;
    let agent = acp_agents::get(&state.db.pool, &params.agent_id)
        .await
        .map_err(err)?;
    let agent_args = acp_agents::agent_args(&agent).map_err(err)?;
    let env = acp_agents::agent_env(&agent).map_err(err)?;

    let cwd = params.cwd.map(std::path::PathBuf::from);
    let page = mcp_chat_core::acp::list_sessions(
        &state.engine.acp,
        &agent.command,
        &agent_args,
        &env,
        cwd.as_deref(),
        params.cursor,
        std::time::Duration::from_secs(30),
    )
    .await
    .map_err(|error| error.user_message())?;

    to_value(page)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachSession {
    conversation_id: String,
    agent_id: String,
    session_id: String,
    /// The session's own directory, as reported by `session/list`. Required
    /// because the agent's `session/load` compares against it.
    cwd: String,
}

/// Bind a chat to a session that already exists on the agent.
///
/// The conversation adopts that session: the next turn continues it instead of
/// starting a new one. Jarvis does not copy the history -- the agent replays it
/// on load, and the pump drops the replay because the transcript already has it.
pub async fn attach_agent_session(state: &AppState, args: Value) -> Result<Value, String> {
    let params: AttachSession = parse(args)?;
    let agent = acp_agents::get(&state.db.pool, &params.agent_id)
        .await
        .map_err(err)?;
    state
        .engine
        .attach_conversation_session(
            &params.conversation_id,
            &agent.slug,
            &params.session_id,
            &params.cwd,
        )
        .await
        .map_err(|error| error.user_message())?;
    to_value(())
}
