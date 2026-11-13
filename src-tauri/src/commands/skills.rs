use serde::Serialize;
use tauri::State;

use mcp_chat_core::agent::tools;

use crate::error::{CmdResult, cmd_err};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
    pub enabled: bool,
}

#[tauri::command]
pub async fn list_skills(
    state: State<'_, AppState>,
    workspace_root: Option<String>,
) -> CmdResult<Vec<SkillView>> {
    let workspace = match workspace_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(raw) => tools::resolve_workspace_path(raw).map_err(cmd_err)?,
        None => tools::workspace_root(&state.db.pool)
            .await
            .map_err(cmd_err)?,
    };
    let disabled = tools::disabled_skill_names(&state.db.pool)
        .await
        .map_err(cmd_err)?;
    Ok(tools::skill_catalog_infos(&workspace, &state.output_dir)
        .into_iter()
        .map(|skill| SkillView {
            name: skill.name.clone(),
            description: skill.description,
            path: skill.path,
            source: skill.source,
            enabled: !disabled.contains(&skill.name),
        })
        .collect())
}

#[tauri::command]
pub async fn set_skill_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> CmdResult<()> {
    tools::set_skill_enabled(&state.db.pool, &name, enabled)
        .await
        .map_err(cmd_err)
}
