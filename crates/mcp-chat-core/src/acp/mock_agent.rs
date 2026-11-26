//! Mock ACP agent for tests.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout, like a real ACP stdio
//! agent, but with scripted turns so tests do not need a model.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn write(message: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

fn text_of(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .map(|block| block.get("text").and_then(Value::as_str).unwrap_or(""))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn chunk(text: &str) {
    write(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "mock-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }
    }));
}

fn finish(id: &Value, stop_reason: &str) {
    write(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "stopReason": stop_reason }
    }));
}

/// Run the mock agent until stdin closes.
///
/// `scenario` selects the scripted behaviour:
/// - `echo` (default): answer each prompt, one chunk per turn.
/// - `tool`: emit a tool call and then a short answer.
/// - `permission`: request permission before answering.
/// - `crash`: exit in the middle of the first prompt.
pub fn run(scenario: &str) {
    let stdin = std::io::stdin();
    let mut turn = 0_u32;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => write(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": { "loadSession": false },
                    "authMethods": [],
                    "agentInfo": { "name": "mock-acp", "version": "0.0.0" }
                }
            })),
            "session/new" => write(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "sessionId": "mock-session" }
            })),
            "session/prompt" => {
                turn += 1;
                let prompt = text_of(&params);
                match scenario {
                    "crash" => {
                        let _ = writeln!(std::io::stderr(), "mock agent crashing mid-turn");
                        std::process::exit(3);
                    }
                    "tool" => {
                        write(json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": "mock-session",
                                "update": {
                                    "sessionUpdate": "tool_call",
                                    "toolCallId": "call-1",
                                    "title": "Read src/main.rs",
                                    "kind": "read",
                                    "status": "pending"
                                }
                            }
                        }));
                        chunk("Checking the file first.");
                        finish(&id, "end_turn");
                    }
                    "permission" => {
                        write(json!({
                            "jsonrpc": "2.0",
                            "method": "session/request_permission",
                            "id": 9001,
                            "params": {
                                "sessionId": "mock-session",
                                "toolCall": { "toolCallId": "call-1", "title": "Run tests" },
                                "options": [
                                    { "optionId": "allow_once", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "reject_once", "name": "Reject", "kind": "reject_once" }
                                ]
                            }
                        }));
                        chunk(&format!("turn {turn} answer: {prompt}"));
                        finish(&id, "end_turn");
                    }
                    _ => {
                        chunk(&format!("turn {turn} answer: {prompt}"));
                        finish(&id, "end_turn");
                    }
                }
            }
            "session/close" => {
                write(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                std::process::exit(0);
            }
            "session/cancel" => {}
            other if !other.is_empty() && !id.is_null() => write(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            })),
            _ => {}
        }
    }
}
