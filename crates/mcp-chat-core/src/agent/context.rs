use std::collections::{HashMap, HashSet};

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
pub fn build_messages(history: &[MessageWithParts]) -> (Vec<ChatMessage>, Option<String>) {
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
    let mut synthesized: HashSet<String> = HashSet::new();

    for message in history.iter().skip(start) {
        if message.parts.iter().any(|part| part.kind == "summary") {
            continue;
        }
        match message.message.role.as_str() {
            "user" => {
                let text = text_parts(&message.parts);
                if !text.trim().is_empty() {
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
                    let text = format!("{note}\n\n{text}");
                    messages.push(ChatMessage::text(ChatRole::User, text));
                }
            }
            "system" => {
                let text = text_parts(&message.parts);
                if !text.trim().is_empty() {
                    messages.push(ChatMessage::text(ChatRole::System, text));
                }
            }
            "assistant" => {
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
            }
            _ => {}
        }
    }

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
    (chars / 3).max(1)
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
