//! The `apply_patch` tool. The patch engine itself lives in `agent::patch`.

use super::workspace::workspace_root_for_conversation;
use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::{CoreError, Result};
use serde_json::Value;

impl ToolExecutor {
    pub(super) async fn run_apply_patch(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let patch_text = req
            .args
            .get("patchText")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("patchText is required"))?;
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let paths =
            crate::agent::patch::target_paths(&workspace, patch_text, req.allow_outside_workspace)?;
        let _guards = self.file_locks.acquire(paths).await;
        let result = crate::agent::patch::apply_with_scope(
            &workspace,
            patch_text,
            req.allow_outside_workspace,
        )
        .await?;
        Ok(Rendered {
            content: result.render(),
            structured: Some(result.structured()),
            ok: true,
        })
    }
}
