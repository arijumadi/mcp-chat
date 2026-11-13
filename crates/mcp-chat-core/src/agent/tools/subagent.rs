use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::{CoreError, Result};
use serde_json::json;

impl ToolExecutor {
    pub(super) async fn run_subagent(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let description = req
            .args
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::invalid("description is required"))?;
        let prompt = req
            .args
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::invalid("prompt is required"))?;
        if prompt.chars().count() > 100_000 {
            return Err(CoreError::invalid("prompt is too long"));
        }
        let engine =
            self.subagent_host.get().cloned().ok_or_else(|| {
                CoreError::Message("The subagent runner is not available.".into())
            })?;
        let (session_id, report) = engine
            .run_subagent(
                &req.conversation_id,
                description,
                prompt,
                req.cancel.clone(),
            )
            .await?;
        Ok(Rendered {
            content: report,
            structured: Some(json!({
                "sessionId": session_id,
                "status": "completed"
            })),
            ok: true,
        })
    }
}
