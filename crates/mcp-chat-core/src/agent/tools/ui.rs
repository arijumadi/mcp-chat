//! UI-only metadata for tool calls, such as the diff shown in the transcript.

use super::ToolExecutor;
use super::workspace::{
    display_relative, read_text_file, resolve_write_path, workspace_root_for_conversation,
};
use serde_json::Value;

/// Remove and return the non-empty `diff` string from a tool's structured output.
pub(super) fn take_ui_diff(structured: &mut Option<Value>) -> Option<String> {
    let object = structured.as_mut()?.as_object_mut()?;
    match object.remove("diff")? {
        Value::String(diff) if !diff.is_empty() => Some(diff),
        _ => None,
    }
}

impl ToolExecutor {
    /// Build a read-only diff for built-in file mutations. Outside-workspace
    /// paths are deliberately skipped: their contents must not be read before
    /// the user grants the separate outside-write approval.
    pub(super) async fn approval_diff(
        &self,
        conversation_id: &str,
        server_id: &str,
        tool_name: &str,
        args: &Value,
    ) -> Option<String> {
        if !server_id.is_empty() {
            return None;
        }
        let workspace = workspace_root_for_conversation(&self.db.pool, conversation_id)
            .await
            .ok()?;
        match tool_name {
            "edit" => {
                let raw = args.get("filePath").and_then(Value::as_str)?;
                let old = args.get("oldString").and_then(Value::as_str)?;
                let new = args.get("newString").and_then(Value::as_str)?;
                let replace_all = args
                    .get("replaceAll")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let content = read_text_file(&path).await.ok()?;
                let outcome = crate::agent::edit::apply(&content, old, new, replace_all).ok()?;
                Some(crate::agent::diff::file_diff(
                    &display_relative(&workspace, &path),
                    &content,
                    &outcome.content,
                ))
                .filter(|diff| !diff.is_empty())
            }
            "apply_patch" => {
                let patch = args.get("patchText").and_then(Value::as_str)?;
                crate::agent::patch::preview_with_scope(&workspace, patch, false)
                    .await
                    .ok()
                    .filter(|diff| !diff.is_empty())
            }
            "write" => {
                let raw = args.get("filePath").and_then(Value::as_str)?;
                let content = args.get("content").and_then(Value::as_str)?;
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let previous = if path.exists() {
                    read_text_file(&path).await.ok()?
                } else {
                    String::new()
                };
                let diff = crate::agent::diff::file_diff(
                    &display_relative(&workspace, &path),
                    &previous,
                    content,
                );
                (!diff.is_empty()).then_some(diff)
            }
            _ => None,
        }
    }
}
