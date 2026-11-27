use std::collections::{HashMap, HashSet};

use base64::Engine;
use serde_json::Value;
use tokio::fs;

use crate::llm::{ChatMessage, ChatRole, ContentBlock, ToolDef};
use crate::models::{Conversation, MessageWithParts, Part, Provider};
use crate::util::{message_time_note, truncate_chars};

pub struct TurnContext {
    pub conversation: Conversation,
    pub provider: Provider,
    pub model: String,
    pub api_key: Option<String>,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: i64,
    pub context_window: i64,
}

pub fn latest_summary(history: &[MessageWithParts]) -> Option<(String, String)> {
    let mut latest: Option<(i64, String, String)> = None;
    for message in history {
        for part in &message.parts {
            if part.kind != "summary" {
                continue;
            }
            let payload = part.payload();
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let covers = payload
                .get("coversUntil")
                .or_else(|| payload.get("covers_until"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() || covers.is_empty() {
                continue;
            }
            if latest
                .as_ref()
                .map(|(ts, _, _)| message.message.created_at >= *ts)
                .unwrap_or(true)
            {
                latest = Some((
                    message.message.created_at,
                    covers.to_owned(),
                    text.to_owned(),
                ));
            }
        }
    }
    latest.map(|(_, covers, text)| (covers, text))
}

/// Convert persisted parts into the canonical provider history. Every tool call
/// gets a matching tool result, even if the app crashed before it settled.
async fn flush_tool_images(messages: &mut Vec<ChatMessage>, pending: &mut Vec<ContentBlock>) {
    if pending.is_empty() {
        return;
    }
    let mut blocks = vec![ContentBlock::Text(
        "[Image returned by the read tool]".to_owned(),
    )];
    blocks.append(pending);
    messages.push(ChatMessage {
        role: ChatRole::User,
        content: blocks,
    });
}

fn context_note(text: &str, context: &crate::models::MessageContext) -> String {
    let range = match (context.start_line, context.end_line) {
        (Some(start), Some(end)) if start == end => format!("line {start}"),
        (Some(start), Some(end)) => format!("lines {start} through {end}"),
        _ => "this file".to_owned(),
    };
    let mut out = format!(
        "The user made the following comment regarding {range} of {}:",
        context.path
    );
    if let Some(preview) = context
        .preview
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        out.push_str(&format!("\n```text\n{preview}\n```"));
    }
    out.push('\n');
    out.push_str(&context.comment);
    if text.trim().is_empty() {
        out
    } else {
        format!("{text}\n\n{out}")
    }
}

fn part_context(part: &Part) -> Option<crate::models::MessageContext> {
    if part.kind != "context" {
        return None;
    }
    let payload = part.payload();
    let path = payload.get("path")?.as_str()?.to_owned();
    let comment = payload.get("comment")?.as_str()?.to_owned();
    Some(crate::models::MessageContext {
        path,
        start_line: payload.get("startLine").and_then(Value::as_i64),
        end_line: payload.get("endLine").and_then(Value::as_i64),
        comment,
        preview: payload
            .get("preview")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

pub async fn build_messages(history: &[MessageWithParts]) -> (Vec<ChatMessage>, Option<String>) {
    let summary = latest_summary(history);
    let mut start = 0;
    let mut summary_text = None;
    if let Some((covers, text)) = &summary {
        if let Some(index) = history
            .iter()
            .position(|message| &message.message.id == covers)
        {
            start = index + 1;
            summary_text = Some(text.clone());
        }
    }

    let mut all_results: HashMap<String, &Part> = HashMap::new();
    for message in history {
        if message.message.role != "tool" {
            continue;
        }
        for part in &message.parts {
            if part.kind == "tool_result" {
                let call_id = part
                    .payload()
                    .get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !call_id.is_empty() {
                    all_results.insert(call_id, part);
                }
            }
        }
    }

    let mut messages = Vec::new();
    let mut pending_tool_images: Vec<ContentBlock> = Vec::new();
    let mut synthesized: HashSet<String> = HashSet::new();

    for message in history.iter().skip(start) {
        if message.parts.iter().any(|part| part.kind == "summary") {
            continue;
        }
        match message.message.role.as_str() {
            "user" => {
                flush_tool_images(&mut messages, &mut pending_tool_images).await;
                let mut text = text_parts(&message.parts);
                let contexts = message
                    .parts
                    .iter()
                    .filter_map(part_context)
                    .collect::<Vec<_>>();
                for context in &contexts {
                    text = context_note(&text, context);
                }
                let attachments: Vec<&Part> = message
                    .parts
                    .iter()
                    .filter(|part| part.kind == "attachment")
                    .collect();
                if text.trim().is_empty() && attachments.is_empty() && contexts.is_empty() {
                    continue;
                }
                // Timestamp lives in the user message, not the system
                // prompt. The stored time part (or created_at for older
                // messages) reproduces the exact same bytes on later
                // requests, so provider prefix caching can hit.
                let note = message
                    .parts
                    .iter()
                    .find(|part| part.kind == "time")
                    .and_then(|part| {
                        part.payload()
                            .get("text")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| message_time_note(message.message.created_at));
                let mut blocks = vec![ContentBlock::Text(if text.trim().is_empty() {
                    note
                } else {
                    format!("{note}\n\n{text}")
                })];
                for part in attachments {
                    let payload = part.payload();
                    let name = payload
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("attachment");
                    let mime = payload
                        .get("mime")
                        .and_then(|value| value.as_str())
                        .unwrap_or("application/octet-stream");
                    let kind = payload
                        .get("fileKind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("file");
                    if kind == "image" {
                        let Some(path) = payload.get("path").and_then(|value| value.as_str())
                        else {
                            continue;
                        };
                        match fs::read(path).await {
                            Ok(bytes) => blocks.push(ContentBlock::Image {
                                media_type: mime.to_owned(),
                                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                            }),
                            Err(_) => blocks.push(ContentBlock::Text(format!(
                                "[Attached image could not be read: {name}]"
                            ))),
                        }
                    } else if kind == "text" {
                        let content = payload
                            .get("text")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let truncated = payload
                            .get("truncated")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false);
                        // The path matters even when the text is inlined: it
                        // lets the model re-read ranges, or the original file,
                        // instead of guessing names in the workspace.
                        let path = payload.get("path").and_then(|value| value.as_str());
                        blocks.push(ContentBlock::Text(match path {
                            Some(path) => format!(
                                "[Attached file: {name} ({mime}){}\nStored at: {path}\n{content}\n[/Attached file]",
                                if truncated { ", truncated" } else { "" }
                            ),
                            None => format!(
                                "[Attached file: {name} ({mime}){}]\n{content}\n[/Attached file]",
                                if truncated { ", truncated" } else { "" }
                            ),
                        }));
                    } else {
                        let size = payload
                            .get("size")
                            .and_then(|value| value.as_i64())
                            .unwrap_or(0);
                        // Without the path a binary attachment is only a name
                        // and a size, and the model reasonably concludes the
                        // file is missing. It is on disk; say where.
                        match payload.get("path").and_then(|value| value.as_str()) {
                            Some(path) => blocks.push(ContentBlock::Text(format!(
                                "[Attached file: {name} ({mime}, {size} bytes). Its content is not \
                                 inlined because it is binary. The file exists at: {path}\nThat \
                                 path is readable. `read` only handles text, so use `bash` (for \
                                 example `unzip -l` for an Office file, or a Python library) to \
                                 inspect it.]"
                            ))),
                            None => blocks.push(ContentBlock::Text(format!(
                                "[Attached binary file: {name} ({mime}, {size} bytes). Its binary \
                                 content is not included in this message.]"
                            ))),
                        }
                    }
                }
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    content: blocks,
                });
            }
            "system" => {
                flush_tool_images(&mut messages, &mut pending_tool_images).await;
                let text = text_parts(&message.parts);
                if !text.trim().is_empty() {
                    messages.push(ChatMessage::text(ChatRole::System, text));
                }
            }
            "assistant" => {
                flush_tool_images(&mut messages, &mut pending_tool_images).await;
                let mut blocks = Vec::new();
                let mut calls = Vec::new();
                for part in &message.parts {
                    match part.kind.as_str() {
                        "text" => {
                            if let Some(text) = part.payload().get("text").and_then(|v| v.as_str())
                            {
                                if !text.is_empty() {
                                    blocks.push(ContentBlock::Text(text.to_owned()));
                                }
                            }
                        }
                        "tool_call" => {
                            let payload = part.payload();
                            let call_id = payload
                                .get("callId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_owned();
                            let name = payload
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_owned();
                            let args = payload
                                .get("args")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({}));
                            if !call_id.is_empty() {
                                calls.push(call_id.clone());
                                blocks.push(ContentBlock::ToolCall {
                                    id: call_id,
                                    name,
                                    arguments: args,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: blocks,
                });
                for call_id in calls {
                    if !all_results.contains_key(&call_id) {
                        synthesized.insert(call_id.clone());
                        messages.push(ChatMessage {
                            role: ChatRole::Tool,
                            content: vec![ContentBlock::ToolResult {
                                call_id,
                                name: String::new(),
                                content: "The tool was interrupted before returning a result."
                                    .into(),
                                is_error: true,
                            }],
                        });
                    }
                }
            }
            "tool" => {
                let mut blocks = Vec::new();
                for part in &message.parts {
                    if part.kind != "tool_result" {
                        continue;
                    }
                    let payload = part.payload();
                    let call_id = payload
                        .get("callId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();
                    if call_id.is_empty() || synthesized.contains(&call_id) {
                        continue;
                    }
                    let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(true);
                    let content = payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();
                    blocks.push(ContentBlock::ToolResult {
                        call_id,
                        name: payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_owned(),
                        content,
                        is_error: !ok,
                    });
                }
                if !blocks.is_empty() {
                    messages.push(ChatMessage {
                        role: ChatRole::Tool,
                        content: blocks,
                    });
                }
                for part in &message.parts {
                    if part.kind != "tool_result" {
                        continue;
                    }
                    let payload = part.payload();
                    let Some(image) = payload
                        .get("structured")
                        .and_then(|value| value.get("image"))
                    else {
                        continue;
                    };
                    let Some(path) = image.get("path").and_then(Value::as_str) else {
                        continue;
                    };
                    let mime = image
                        .get("mime")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    match fs::read(path).await {
                        Ok(bytes) => pending_tool_images.push(ContentBlock::Image {
                            media_type: mime.to_owned(),
                            data: base64::engine::general_purpose::STANDARD.encode(bytes),
                        }),
                        Err(_) => pending_tool_images.push(ContentBlock::Text(format!(
                            "[Tool image could not be read: {path}]"
                        ))),
                    }
                }
            }
            _ => {}
        }
    }

    flush_tool_images(&mut messages, &mut pending_tool_images).await;
    (messages, summary_text)
}

pub fn text_parts(parts: &[Part]) -> String {
    parts
        .iter()
        .filter(|part| part.kind == "text")
        .filter_map(|part| {
            part.payload()
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>()
        .join("")
}

pub fn estimate_tokens(messages: &[ChatMessage], tools: &[ToolDef], system: Option<&str>) -> usize {
    let mut chars = system.map(|s| s.chars().count()).unwrap_or(0);
    for message in messages {
        chars += message.text_content().chars().count();
        for block in &message.content {
            match block {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    chars += id.len()
                        + name.len()
                        + serde_json::to_string(arguments)
                            .map(|s| s.len())
                            .unwrap_or(0);
                }
                ContentBlock::ToolResult { content, .. } => {
                    chars += content.chars().count();
                }
                ContentBlock::Text(text) => chars += text.chars().count(),
                ContentBlock::Image { data, .. } => chars += data.len().min(24_000),
            }
        }
    }
    for tool in tools {
        chars += tool.name.len()
            + tool.description.len()
            + serde_json::to_string(&tool.input_schema)
                .map(|s| s.len())
                .unwrap_or(0);
    }
    (chars / 4).max(1)
}

/// A compact transcript to carry a chat across an agent switch.
///
/// Two ACP agents cannot share a session: the session id belongs to one agent,
/// and its history lives inside that agent's own store. Switching therefore
/// starts a fresh session, and without this the new agent would answer as if the
/// conversation had just begun.
///
/// The transcript is built from parts Jarvis already stores, so no model call
/// is involved and switching stays instant and free. Tool output is trimmed hard
/// because the new agent can re-read files itself; what it needs is what was
/// asked, what was decided, and what changed.
pub fn transcript_for_handoff(history: &[MessageWithParts], limit: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for message in history {
        // Synthetic rows and tool transcript rows are attachment points for the
        // provider, not part of the conversation a person would recognise.
        if message.message.status == "synthetic" || message.message.role == "tool" {
            continue;
        }
        let mut text = String::new();
        for part in &message.parts {
            let payload = part.payload();
            match part.kind.as_str() {
                "text" => {
                    if let Some(value) = payload.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(value);
                    }
                }
                "tool_call" => {
                    let name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    let arguments = payload.get("args").cloned().unwrap_or_default();
                    let arguments =
                        truncate_chars(&serde_json::to_string(&arguments).unwrap_or_default(), 160)
                            .0;
                    let state = payload.get("state").and_then(|v| v.as_str()).unwrap_or("");
                    text.push_str(&format!("\n[tool {name} {state} {arguments}]"));
                }
                "summary" => {
                    if let Some(value) = payload.get("text").and_then(|v| v.as_str()) {
                        text.push_str(&format!("\n[earlier summary] {value}"));
                    }
                }
                "context" => {
                    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    text.push_str(&format!("\n[file context {path}]"));
                }
                "attachment" => {
                    let name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("file");
                    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    text.push_str(&format!("\n[attached {name} at {path}]"));
                }
                _ => {}
            }
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let role = match message.message.role.as_str() {
            "assistant" => "Assistant",
            "system" => "System",
            _ => "User",
        };
        lines.push(format!("{role}: {text}"));
    }
    // Keep the end of the conversation: the most recent decisions are what the
    // next agent needs, and the start is usually a greeting. `truncate_chars`
    // cuts from the front, which is the opposite of what a handoff wants, so
    // drop whole messages from the top until the rest fits.
    let mut kept: std::collections::VecDeque<String> = lines.into();
    let mut total: usize = kept.iter().map(|line| line.chars().count() + 2).sum();
    while total > limit && kept.len() > 1 {
        if let Some(dropped) = kept.pop_front() {
            total = total.saturating_sub(dropped.chars().count() + 2);
        }
    }
    let joined = kept.into_iter().collect::<Vec<_>>().join("\n\n");
    if joined.chars().count() <= limit {
        joined
    } else {
        // A single message longer than the whole budget: keep its tail, which
        // is where the conclusion sits.
        let mut chars = joined.chars().rev().take(limit).collect::<Vec<_>>();
        chars.reverse();
        format!("[trimmed] ...{}", chars.into_iter().collect::<String>())
    }
}

/// Split a handoff into the part that must be summarised and the part that fits.
///
/// Returns `(older, recent)` boundaries as indices into `history`. `recent` is
/// the tail that fits the budget; everything before it is what a summary must
/// cover. Empty `older` means the transcript already fits.
pub fn handoff_split(history: &[MessageWithParts], budget_chars: usize) -> (usize, usize) {
    let mut kept = 0usize;
    let mut start = history.len();
    for index in (0..history.len()).rev() {
        let message = &history[index];
        if message.message.role == "tool" || message.message.status == "synthetic" {
            continue;
        }
        let tokens = message_tokens(message);
        let chars = tokens.saturating_mul(4);
        if kept + chars > budget_chars && start < history.len() {
            break;
        }
        kept += chars;
        start = index;
    }
    // Never split a tool row away from the call that produced it.
    while start > 0 && history[start].message.role == "tool" {
        start -= 1;
    }
    (start, history.len())
}

/// Estimate the characters one stored message will occupy in a prompt.
pub fn message_tokens(message: &MessageWithParts) -> usize {
    let mut chars = 0usize;
    for part in &message.parts {
        let payload = part.payload();
        match part.kind.as_str() {
            "text" => {
                chars += payload
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .chars()
                    .count()
            }
            "tool_call" => {
                chars += serde_json::to_string(&payload.get("args").cloned().unwrap_or(Value::Null))
                    .map(|value| value.len())
                    .unwrap_or(0)
            }
            "tool_result" => {
                chars += payload
                    .get("content")
                    .and_then(|value| value.as_str())
                    .map(str::len)
                    .unwrap_or(0)
            }
            "summary" => {
                chars += payload
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::len)
                    .unwrap_or(0)
            }
            _ => {}
        }
    }
    (chars / 4).max(1)
}

/// The block prepended to the first prompt of a newly opened session.
pub fn handoff_block(transcript: &str) -> String {
    format!(
        "You are taking over this conversation from another agent. Nothing you \
         remember is available from it, so here is what has happened so far, \
         oldest first. Carry on from the last message; do not repeat the summary \
         back unless asked.\n\n<previous_conversation>\n{transcript}\n\
         </previous_conversation>"
    )
}

pub fn transcript_for_summary(history: &[MessageWithParts]) -> String {
    let mut out = String::new();
    for message in history {
        let role = match message.message.role.as_str() {
            "assistant" => "assistant",
            "tool" => "tool",
            "system" => "system",
            _ => "user",
        };
        let mut line = String::new();
        for part in &message.parts {
            let payload = part.payload();
            match part.kind.as_str() {
                "text" => {
                    if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                        line.push_str(text);
                    }
                }
                "tool_call" => {
                    let name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool");
                    let args = payload.get("args").cloned().unwrap_or_default();
                    line.push_str(&format!(
                        "\n[tool call {name} {}]",
                        serde_json::to_string(&args).unwrap_or_default()
                    ));
                }
                "tool_result" => {
                    let content = payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(true);
                    line.push_str(&format!(
                        "\n[tool result {} {}]",
                        if ok { "ok" } else { "error" },
                        truncate_chars(content, 800).0
                    ));
                }
                "summary" => {
                    if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                        line.push_str(&format!("\n[previous summary] {text}"));
                    }
                }
                "context" => {
                    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let comment = payload
                        .get("comment")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    line.push_str(&format!("\n[file context {path}] {comment}"));
                }
                "attachment" => {
                    let name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("attachment");
                    let kind = payload
                        .get("fileKind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("file");
                    if kind == "text" {
                        let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        line.push_str(&format!(
                            "\n[attached file {name}]\n{}",
                            truncate_chars(text, 2_000).0
                        ));
                    } else {
                        line.push_str(&format!("\n[attached {kind} {name}]"));
                    }
                }
                _ => {}
            }
        }
        if !line.trim().is_empty() {
            out.push_str(&format!("\n\n{role}: {line}"));
            if out.chars().count() > 24_000 {
                out.push_str("\n[truncated]");
                break;
            }
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Message, Part};
    use serde_json::json;

    fn message_with_parts(parts: Vec<Part>) -> MessageWithParts {
        MessageWithParts {
            message: Message {
                id: "m1".into(),
                conversation_id: "c1".into(),
                role: "user".into(),
                created_at: 0,
                status: "completed".into(),
                error: None,
            },
            parts,
        }
    }

    #[tokio::test]
    async fn image_attachments_become_provider_image_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("pixel.png");
        tokio::fs::write(&image, b"image-bytes").await.unwrap();
        let history = vec![message_with_parts(vec![
            Part {
                id: "text".into(),
                message_id: "m1".into(),
                idx: 0,
                kind: "text".into(),
                payload_json: json!({"text": "describe this"}).to_string(),
            },
            Part {
                id: "attachment".into(),
                message_id: "m1".into(),
                idx: 1,
                kind: "attachment".into(),
                payload_json: json!({
                    "id": "a1",
                    "name": "pixel.png",
                    "mime": "image/png",
                    "size": 11,
                    "fileKind": "image",
                    "path": image.to_string_lossy(),
                })
                .to_string(),
            },
        ])];
        let (messages, _) = build_messages(&history).await;
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0].content[0], ContentBlock::Text(text) if text.contains("describe this"))
        );
        assert!(matches!(
            &messages[0].content[1],
            ContentBlock::Image { media_type, data } if media_type == "image/png" && data == "aW1hZ2UtYnl0ZXM="
        ));
    }

    #[tokio::test]
    async fn file_context_is_inlined_for_the_model() {
        let history = vec![message_with_parts(vec![
            Part {
                id: "text".into(),
                message_id: "m1".into(),
                idx: 0,
                kind: "text".into(),
                payload_json: json!({"text": "please check this"}).to_string(),
            },
            Part {
                id: "context".into(),
                message_id: "m1".into(),
                idx: 1,
                kind: "context".into(),
                payload_json: json!({
                    "path": "src/main.rs",
                    "startLine": 10,
                    "endLine": 12,
                    "comment": "handle the empty case",
                    "preview": "let value = read();\nuse(value);"
                })
                .to_string(),
            },
        ])];
        let (messages, _) = build_messages(&history).await;
        assert_eq!(messages.len(), 1);
        let text = messages[0].text_content();
        assert!(text.contains("please check this"));
        assert!(text.contains("lines 10 through 12 of src/main.rs"));
        assert!(text.contains("handle the empty case"));
        assert!(text.contains("let value = read();"));
    }

    #[tokio::test]
    async fn file_context_without_message_text_still_reaches_the_model() {
        let history = vec![message_with_parts(vec![Part {
            id: "context".into(),
            message_id: "m1".into(),
            idx: 0,
            kind: "context".into(),
            payload_json: json!({
                "path": "README.md",
                "startLine": 3,
                "endLine": 3,
                "comment": "this line is stale",
                "preview": "Old install step"
            })
            .to_string(),
        }])];
        let (messages, _) = build_messages(&history).await;
        assert_eq!(messages.len(), 1);
        let text = messages[0].text_content();
        assert!(text.contains("line 3 of README.md"));
        assert!(text.contains("this line is stale"));
        assert!(text.contains("Old install step"));
    }

    #[tokio::test]
    async fn tool_images_become_a_user_image_block() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("pixel.png");
        tokio::fs::write(&image, b"image-bytes").await.unwrap();
        let history = vec![MessageWithParts {
            message: Message {
                id: "tool-message".into(),
                conversation_id: "c1".into(),
                role: "tool".into(),
                created_at: 0,
                status: "completed".into(),
                error: None,
            },
            parts: vec![Part {
                id: "result".into(),
                message_id: "tool-message".into(),
                idx: 0,
                kind: "tool_result".into(),
                payload_json: json!({
                    "callId": "call-1",
                    "name": "read",
                    "server": "mcp-chat",
                    "ok": true,
                    "content": "Image loaded",
                    "structured": {
                        "image": {
                            "name": "pixel.png",
                            "mime": "image/png",
                            "path": image.to_string_lossy(),
                            "size": 11
                        }
                    },
                    "fullResultRef": null,
                    "truncated": false
                })
                .to_string(),
            }],
        }];
        let (messages, _) = build_messages(&history).await;
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, ChatRole::Tool));
        assert!(matches!(messages[1].role, ChatRole::User));
        assert!(matches!(
            &messages[1].content[1],
            ContentBlock::Image { media_type, data } if media_type == "image/png" && data == "aW1hZ2UtYnl0ZXM="
        ));
    }

    /// Reported: an attached spreadsheet made the model answer "there is no
    /// such file". The prompt only had a name and a size, so there was nothing
    /// to open.
    #[tokio::test]
    async fn a_binary_attachment_tells_the_model_where_it_is() {
        let history = vec![message_with_parts(vec![Part {
            id: "attachment".into(),
            message_id: "m1".into(),
            idx: 0,
            kind: "attachment".into(),
            payload_json: json!({
                "id": "a1",
                "name": "laporan.xlsx",
                "mime": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "size": 41_234,
                "fileKind": "file",
                "path": "/var/lib/jarvis/attachments/c1/m1/laporan.xlsx",
            })
            .to_string(),
        }])];
        let (messages, _) = build_messages(&history).await;
        let text = messages[0].text_content();
        assert!(text.contains("laporan.xlsx"), "{text}");
        assert!(
            text.contains("/var/lib/jarvis/attachments/c1/m1/laporan.xlsx"),
            "the model needs the real path, not just the name: {text}"
        );
        assert!(
            text.contains("readable"),
            "it must know the path is allowed: {text}"
        );
    }

    #[tokio::test]
    async fn text_attachments_are_inlined_for_the_model() {
        let history = vec![message_with_parts(vec![Part {
            id: "attachment".into(),
            message_id: "m1".into(),
            idx: 0,
            kind: "attachment".into(),
            payload_json: json!({
                "id": "a1",
                "name": "notes.txt",
                "mime": "text/plain",
                "size": 5,
                "fileKind": "text",
                "path": "/tmp/notes.txt",
                "text": "hello",
            })
            .to_string(),
        }])];
        let (messages, _) = build_messages(&history).await;
        let text = messages[0].text_content();
        assert!(text.contains("notes.txt"));
        assert!(text.contains("hello"));
    }

    /// Switching agents starts a new ACP session, and a new session knows
    /// nothing. The transcript is what stops the next agent from answering as
    /// if the chat had just begun.
    #[tokio::test]
    async fn a_handoff_carries_the_conversation_forward() {
        let history = vec![
            message_with_parts(vec![Part {
                id: "u1".into(),
                message_id: "m1".into(),
                idx: 0,
                kind: "text".into(),
                payload_json: json!({"text": "refactor the parser"}).to_string(),
            }]),
            MessageWithParts {
                message: Message {
                    id: "m2".into(),
                    conversation_id: "c1".into(),
                    role: "assistant".into(),
                    created_at: 1,
                    status: "completed".into(),
                    error: None,
                },
                parts: vec![
                    Part {
                        id: "a1".into(),
                        message_id: "m2".into(),
                        idx: 0,
                        kind: "text".into(),
                        payload_json: json!({"text": "I moved parse() into parser.rs."})
                            .to_string(),
                    },
                    Part {
                        id: "t1".into(),
                        message_id: "m2".into(),
                        idx: 1,
                        kind: "tool_call".into(),
                        payload_json: json!({
                            "name": "write",
                            "state": "completed",
                            "callId": "c-1",
                            "args": {"filePath": "src/parser.rs"}
                        })
                        .to_string(),
                    },
                ],
            },
        ];

        let transcript = transcript_for_handoff(&history, 24_000);
        assert!(transcript.contains("refactor the parser"), "{transcript}");
        assert!(transcript.contains("moved parse()"), "{transcript}");
        assert!(
            transcript.contains("src/parser.rs"),
            "the new agent needs to know which files were touched: {transcript}"
        );

        let block = handoff_block(&transcript);
        assert!(block.contains("<previous_conversation>"), "{block}");
        assert!(block.contains("taking over"), "{block}");
    }

    /// Tool transcripts and synthetic rows are provider scaffolding, not
    /// conversation; carrying them would spend the budget on noise.
    #[tokio::test]
    async fn a_handoff_skips_tool_and_synthetic_rows() {
        let history = vec![
            MessageWithParts {
                message: Message {
                    id: "t".into(),
                    conversation_id: "c1".into(),
                    role: "tool".into(),
                    created_at: 0,
                    status: "completed".into(),
                    error: None,
                },
                parts: vec![Part {
                    id: "p".into(),
                    message_id: "t".into(),
                    idx: 0,
                    kind: "text".into(),
                    payload_json: json!({"text": "raw provider payload"}).to_string(),
                }],
            },
            MessageWithParts {
                message: Message {
                    id: "s".into(),
                    conversation_id: "c1".into(),
                    role: "user".into(),
                    created_at: 1,
                    status: "synthetic".into(),
                    error: None,
                },
                parts: vec![Part {
                    id: "p2".into(),
                    message_id: "s".into(),
                    idx: 0,
                    kind: "text".into(),
                    payload_json: json!({"text": "continue please"}).to_string(),
                }],
            },
        ];
        assert!(transcript_for_handoff(&history, 24_000).is_empty());
    }

    /// A long chat must not blow the budget; the recent end is what matters.
    #[tokio::test]
    async fn a_handoff_keeps_the_recent_end_when_it_is_long() {
        let mut history = Vec::new();
        for index in 0..50 {
            history.push(message_with_parts(vec![Part {
                id: format!("p{index}"),
                message_id: "m1".into(),
                idx: 0,
                kind: "text".into(),
                payload_json: json!({"text": format!("message number {index}")}).to_string(),
            }]));
        }
        let transcript = transcript_for_handoff(&history, 400);
        assert!(transcript.chars().count() < 600, "{transcript}");
        assert!(
            transcript.contains("message number 49"),
            "the newest messages must survive the trim: {transcript}"
        );
    }

    /// A transcript under the budget is sent whole; one over it is split so the
    /// head can be summarised instead of silently dropped.
    #[tokio::test]
    async fn a_handoff_splits_only_when_the_budget_is_exceeded() {
        let mut history = Vec::new();
        for index in 0..20 {
            history.push(message_with_parts(vec![Part {
                id: format!("p{index}"),
                message_id: "m1".into(),
                idx: 0,
                kind: "text".into(),
                payload_json: json!({"text": format!("message {index} with some words in it")})
                    .to_string(),
            }]));
        }

        // Generous budget: nothing to summarise.
        let (older, _) = handoff_split(&history, 1_000_000);
        assert_eq!(older, 0, "a transcript that fits must not be summarised");

        // Tight budget: an older head is separated from a recent tail.
        let (older, recent) = handoff_split(&history, 200);
        assert!(
            older > 0,
            "a long transcript must produce a head to summarise"
        );
        assert_eq!(recent, history.len());
        assert!(older < history.len(), "the tail must not be empty");
    }
}
