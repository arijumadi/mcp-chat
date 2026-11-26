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

/// What the agent is waiting for after it asked permission.
struct PendingTurn {
    /// The `session/prompt` request id, answered once permission arrives.
    prompt_id: Value,
    answer: String,
}

/// Run the mock agent until stdin closes.
///
/// `scenario` selects the scripted behaviour:
/// - `echo` (default): answer each prompt, one chunk per turn.
/// - `tool`: emit a tool call and then a short answer.
/// - `permission`: request permission, wait for the answer, then report which
///   option the client selected.
/// - `crash`: exit in the middle of the first prompt.
///
/// A second argument of `close` makes the agent advertise
/// `sessionCapabilities.close`, so the client is allowed to call it.
pub fn run(scenario: &str) {
    let supports_close = std::env::args().nth(2).as_deref() == Some("close");
    let capabilities = if supports_close {
        json!({
            "loadSession": false,
            "sessionCapabilities": { "close": {} }
        })
    } else {
        json!({ "loadSession": false })
    };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut turn = 0_u32;
    let mut pending: Option<PendingTurn> = None;
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
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

        // A response to our permission request: report the chosen option so a
        // test can assert exactly which one the client picked.
        if method.is_empty() {
            if let Some(waiting) = pending.take() {
                let outcome = message
                    .get("result")
                    .and_then(|result| result.get("outcome"));
                let kind = outcome
                    .and_then(|outcome| outcome.get("outcome"))
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let option = outcome
                    .and_then(|outcome| outcome.get("optionId"))
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                chunk(&format!(
                    "{} permission={kind} option={option}",
                    waiting.answer
                ));
                finish(&waiting.prompt_id, "end_turn");
            }
            continue;
        }

        match method {
            "initialize" => write(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": capabilities,
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
                        // `allow_always` is offered on purpose: the client must
                        // never pick it.
                        write(json!({
                            "jsonrpc": "2.0",
                            "method": "session/request_permission",
                            "id": 9001,
                            "params": {
                                "sessionId": "mock-session",
                                "toolCall": {
                                    "toolCallId": "call-1",
                                    "title": "Run tests",
                                    "kind": "execute",
                                    "locations": [
                                        { "path": "/tmp/example/src/main.rs", "line": 4 }
                                    ],
                                    "content": [
                                        {
                                            "type": "diff",
                                            "path": "/tmp/example/src/main.rs",
                                            "oldText": "fn main() {\n    old();\n}\n",
                                            "newText": "fn main() {\n    new();\n}\n"
                                        }
                                    ]
                                },
                                "options": [
                                    { "optionId": "allow-once", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "allow-always", "name": "Always", "kind": "allow_always" },
                                    { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" }
                                ]
                            }
                        }));
                        pending = Some(PendingTurn {
                            prompt_id: id,
                            answer: format!("turn {turn} answer: {prompt}"),
                        });
                    }
                    _ => {
                        chunk(&format!("turn {turn} answer: {prompt}"));
                        finish(&id, "end_turn");
                    }
                }
            }
            "session/close" => {
                write(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                return;
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
