//! Spilled tool output: storage, paging and cleanup.

use super::{ToolExecResult, ToolExecutor};
use crate::db::repo::messages;
use crate::models::PartPayload;
use crate::{CoreError, Result};
use serde_json::{Value, json};
use std::path::Path;

pub(super) const READ_PAGE_CHARS: usize = 24_000;

/// Page formatted tool output without ending a page in the middle of a line.
/// Offsets returned by a previous page are line-aligned. A single line longer
/// than the page limit still gets a hard cut so paging always makes progress.
pub(super) fn page_text(content: &str, offset: usize) -> (usize, String, usize, usize) {
    let total = content.chars().count();
    if offset >= total {
        return (total, String::new(), total, total);
    }

    let start = offset;

    let mut chunk: String = content.chars().skip(start).take(READ_PAGE_CHARS).collect();
    let mut next = start + chunk.chars().count();
    if next < total {
        if let Some(last_newline) = chunk.rfind('\n') {
            chunk.truncate(last_newline + 1);
            next = start + chunk.chars().count();
        }
    }
    (start, chunk, next, total)
}

pub(super) async fn read_spilled_content(dir: &Path, call_id: &str) -> Result<String> {
    if call_id.is_empty()
        || call_id.len() > 128
        || !call_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(CoreError::invalid("invalid call_id"));
    }
    let path = dir.join(format!("{call_id}.json"));
    let raw = tokio::fs::read_to_string(&path).await.map_err(|_| {
        CoreError::Message(format!("No spilled output exists for call_id {call_id:?}."))
    })?;
    let body: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({"content": raw}));
    Ok(body
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            body.get("structured")
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
        })
        .unwrap_or_default())
}

pub async fn read_spilled(dir: &Path, call_id: &str, offset: usize) -> Result<String> {
    let content = read_spilled_content(dir, call_id).await?;
    let (start, chunk, next, total) = page_text(&content, offset);
    if chunk.is_empty() {
        return Ok(json!({
            "content": "",
            "offset": total,
            "next_offset": total,
            "total": total,
            "message": "Offset is at or beyond the end of this output."
        })
        .to_string());
    }
    Ok(json!({
        "content": chunk,
        "offset": start,
        "next_offset": next,
        "total": total,
        "truncated": next < total
    })
    .to_string())
}

pub async fn read_spilled_lines(
    dir: &Path,
    call_id: &str,
    start_line: usize,
    max_lines: usize,
) -> Result<String> {
    let content = read_spilled_content(dir, call_id).await?;
    let total_lines = if content.is_empty() {
        0
    } else {
        content.lines().count()
    };
    let start = start_line.max(1) - 1;
    if start >= total_lines {
        return Ok(json!({
            "content": "",
            "line": total_lines + 1,
            "next_line": total_lines + 1,
            "total_lines": total_lines,
            "message": "Line is at or beyond the end of this output."
        })
        .to_string());
    }
    let max_lines = max_lines.clamp(1, 5_000);
    let end = (start + max_lines).min(total_lines);
    let chunk = content
        .lines()
        .skip(start)
        .take(end - start)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "content": chunk,
        "line": start + 1,
        "next_line": if end < total_lines { end + 1 } else { total_lines },
        "total_lines": total_lines,
        "truncated": end < total_lines
    })
    .to_string())
}

/// Persist a tool result message. Results are appended in original call order,
/// never completion order.
pub async fn persist_result(
    executor: &ToolExecutor,
    conversation_id: &str,
    _assistant_message_id: &str,
    result: &ToolExecResult,
) -> Result<String> {
    let message = messages::create(&executor.db.pool, conversation_id, "tool", "completed").await?;
    let payload = PartPayload::ToolResult {
        call_id: result.call_id.clone(),
        name: result.exposed_name.clone(),
        server: result.server_name.clone(),
        ok: result.ok,
        content: result.content.clone(),
        structured: result.structured.clone(),
        full_result_ref: result.full_result_ref.clone(),
        truncated: result.truncated,
    };
    messages::append_part(&executor.db.pool, &message.id, "tool_result", &payload).await?;
    Ok(message.id)
}

pub async fn remove_spilled(dir: &Path, call_id: &str) -> Result<()> {
    if call_id.is_empty()
        || call_id.len() > 128
        || !call_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(CoreError::invalid("invalid call_id"));
    }
    let path = dir.join(format!("{call_id}.json"));
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CoreError::Io(err)),
    }
}

impl ToolExecutor {
    pub(super) async fn spill(
        &self,
        call_id: &str,
        content: &str,
        structured: &Option<Value>,
    ) -> Result<String> {
        tokio::fs::create_dir_all(&self.output_dir).await?;
        let path = self.output_dir.join(format!("{call_id}.json"));
        let body = json!({"content": content, "structured": structured});
        tokio::fs::write(&path, serde_json::to_vec_pretty(&body)?).await?;
        Ok(format!("tool-output/{call_id}.json"))
    }
}
