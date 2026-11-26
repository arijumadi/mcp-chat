//! ACP agent registry commands.
//!
//! The registry lives in SQLite and is managed here, not in a config file. Env
//! values are stored but never returned to the UI: they can hold API keys, and
//! the same rule already applies to MCP server env.

use serde::{Deserialize, Serialize};
use tauri::State;

use mcp_chat_core::acp::config::{AgentPreset, PRESETS};
use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::repo::acp_agents::{self, AcpAgentConfig};
use mcp_chat_core::db::repo::conversations;
use mcp_chat_core::models::AcpAgent;

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub command: String,
    /// Parsed args, so the UI does not have to decode JSON.
    pub args: Vec<String>,
    /// Env keys only. Values are secrets and stay on the backend.
    pub env_keys: Vec<String>,
    pub permission_policy: String,
    pub enabled: bool,
    pub last_error: Option<String>,
    pub last_checked_at: Option<i64>,
}

/// A slash command the external agent understands.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandView {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

/// One selectable value of an agent config option.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigChoiceView {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// A model, thinking level, mode, or any other option the agent exposes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigOptionView {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub current_value: String,
    pub choices: Vec<AgentConfigChoiceView>,
    pub kind: String,
}

fn config_view(options: Vec<mcp_chat_core::acp::ConfigOption>) -> Vec<AgentConfigOptionView> {
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
pub struct AcpPresetView {
    pub name: String,
    pub slug: String,
    pub command: String,
    pub args: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentInput {
    pub name: String,
    pub slug: String,
    pub command: String,
    pub args: Vec<String>,
    /// Full replacement env. `None` keeps the stored values, so editing a name
    /// does not wipe credentials the UI never received.
    pub env: Option<std::collections::BTreeMap<String, String>>,
    pub permission_policy: String,
    pub enabled: bool,
}

use mcp_chat_core::acp::AcpProbe;

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

#[tauri::command]
pub async fn list_acp_agents(state: State<'_, AppState>) -> CmdResult<Vec<AcpAgentView>> {
    let list = acp_agents::list(&state.db.pool).await.map_err(cmd_err)?;
    Ok(list.iter().map(view).collect())
}

#[tauri::command]
pub fn list_acp_presets() -> Vec<AcpPresetView> {
    PRESETS
        .iter()
        .map(|preset: &AgentPreset| AcpPresetView {
            name: preset.name.to_owned(),
            slug: preset.slug.to_owned(),
            command: preset.command.to_owned(),
            args: preset.args.iter().map(|arg| (*arg).to_owned()).collect(),
            notes: preset.notes.to_owned(),
        })
        .collect()
}

#[tauri::command]
pub async fn create_acp_agent(
    state: State<'_, AppState>,
    input: AcpAgentInput,
) -> CmdResult<AcpAgentView> {
    let env_json = match &input.env {
        Some(env) => serde_json::to_string(env).map_err(|_| "Invalid env.".to_string())?,
        None => "{}".into(),
    };
    let config = to_config(&input, env_json)?;
    let agent = acp_agents::create(&state.db.pool, &config)
        .await
        .map_err(cmd_err)?;
    Ok(view(&agent))
}

#[tauri::command]
pub async fn update_acp_agent(
    state: State<'_, AppState>,
    id: String,
    input: AcpAgentInput,
) -> CmdResult<AcpAgentView> {
    // Keep stored env when the caller did not supply one: the UI never sees
    // the values, so it cannot round-trip them.
    let env_json = match &input.env {
        Some(env) => serde_json::to_string(env).map_err(|_| "Invalid env.".to_string())?,
        None => {
            let current = acp_agents::get(&state.db.pool, &id)
                .await
                .map_err(cmd_err)?;
            current.env_json
        }
    };
    let config = to_config(&input, env_json)?;
    let agent = acp_agents::update(&state.db.pool, &id, &config)
        .await
        .map_err(cmd_err)?;
    Ok(view(&agent))
}

#[tauri::command]
pub async fn delete_acp_agent(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    acp_agents::delete(&state.db.pool, &id)
        .await
        .map_err(cmd_err)?;
    Ok(())
}

#[tauri::command]
pub async fn set_acp_agent_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CmdResult<AcpAgentView> {
    let agent = acp_agents::set_enabled(&state.db.pool, &id, enabled)
        .await
        .map_err(cmd_err)?;
    Ok(view(&agent))
}

/// Start the agent, initialize it, open a session, and close it again.
///
/// This is the only honest way to answer "does this preset work on this
/// machine": a missing binary, a wrong flag, and an unauthenticated agent all
/// look the same from the config alone.
#[tauri::command]
pub async fn test_acp_agent(state: State<'_, AppState>, id: String) -> CmdResult<AcpProbe> {
    let agent = acp_agents::get(&state.db.pool, &id)
        .await
        .map_err(cmd_err)?;
    let workspace = mcp_chat_core::agent::tools::workspace_root(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    match ChatEngine::probe_acp_agent(&agent, &workspace).await {
        Ok(probe) => {
            let _ = acp_agents::set_health(
                &state.db.pool,
                &id,
                if probe.ok { None } else { Some(&probe.message) },
            )
            .await;
            Ok(probe)
        }
        Err(error) => {
            let message = error.user_message();
            let _ = acp_agents::set_health(&state.db.pool, &id, Some(&message)).await;
            Ok(AcpProbe {
                ok: false,
                message,
                session_id: None,
                agent_name: None,
                agent_version: None,
            })
        }
    }
}

/// Bind or clear the ACP agent for a conversation.
#[tauri::command]
pub async fn set_conversation_acp_agent(
    state: State<'_, AppState>,
    conversation_id: String,
    slug: Option<String>,
) -> CmdResult<()> {
    state
        .engine
        .set_conversation_agent(&conversation_id, slug.as_deref())
        .await
        .map_err(cmd_err)?;
    Ok(())
}

/// The agents a chat can be switched to, for the composer picker.
#[tauri::command]
pub async fn list_acp_agents_for_chat(state: State<'_, AppState>) -> CmdResult<Vec<AcpAgentView>> {
    let list = acp_agents::list_enabled(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    Ok(list.iter().map(view).collect())
}

/// The agent bound to a conversation, if any.
#[tauri::command]
pub async fn get_conversation_acp_agent(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Option<String>> {
    let conversation = conversations::get(&state.db.pool, &conversation_id)
        .await
        .map_err(cmd_err)?;
    Ok(conversation.acp_agent)
}

/// Slash commands the agent advertised for this chat.
///
/// The agent executes them; Jarvis only offers them so the user does not have
/// to memorise which external agent supports what.
#[tauri::command]
pub async fn get_agent_commands(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Vec<AgentCommandView>> {
    let commands = state.engine.agent_commands(&conversation_id).await;
    Ok(commands
        .into_iter()
        .map(|command| AgentCommandView {
            name: command.name,
            description: command.description,
            input_hint: command.input_hint,
        })
        .collect())
}

/// The agent's configurable options for this chat.
#[tauri::command]
pub async fn get_agent_config(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Vec<AgentConfigOptionView>> {
    Ok(config_view(
        state.engine.agent_config(&conversation_id).await,
    ))
}

/// Change one agent option, such as the model or mode.
#[tauri::command]
pub async fn set_agent_config(
    state: State<'_, AppState>,
    conversation_id: String,
    config_id: String,
    value: String,
) -> CmdResult<Vec<AgentConfigOptionView>> {
    let options = state
        .engine
        .set_agent_config(&conversation_id, &config_id, &value)
        .await
        .map_err(cmd_err)?;
    Ok(config_view(options))
}
