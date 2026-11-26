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

/// Two models, so a test can change one and observe the result.
fn mock_config_options(current: &str) -> Value {
    json!([
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current,
            "options": [
                { "value": "mock-model-a", "name": "Mock Model A" },
                { "value": "mock-model-b", "name": "Mock Model B" }
            ]
        },
        {
            "id": "mode",
            "name": "Mode",
            "category": "mode",
            "type": "select",
            "currentValue": "build",
            "options": [
                { "value": "build", "name": "Build" },
                { "value": "plan", "name": "Plan" }
            ]
        }
    ])
}

fn chunk(session: &str, text: &str) {
    write(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }
    }));
}

/// Announce the slash commands this mock understands.
///
/// Real agents send this right after a session is created, which is before any
/// prompt, so the client has to read updates while idle to see it.
fn announce_commands(session: &str) {
    write(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session,
            "update": {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    { "name": "review", "description": "Review the changes" },
                    {
                        "name": "search",
                        "description": "Search the codebase",
                        "input": { "hint": "<query>" }
                    }
                ]
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
    // Resume is always advertised so a test can exercise the restore path;
    // close stays opt-in so the capability gate keeps getting tested.
    let capabilities = if supports_close {
        json!({
            "loadSession": false,
            "promptCapabilities": { "image": true, "embeddedContext": true },
            "sessionCapabilities": { "close": {}, "resume": {} }
        })
    } else {
        json!({
            "loadSession": false,
            "promptCapabilities": { "image": true, "embeddedContext": true },
            "sessionCapabilities": { "resume": {} }
        })
    };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut turn = 0_u32;
    let mut session_id = "mock-session".to_owned();
    let mut session_config = "mock-model-a".to_owned();
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
                chunk(
                    &session_id,
                    &format!("{} permission={kind} option={option}", waiting.answer),
                );
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
            "session/new" => {
                session_id = "mock-session".to_owned();
                write(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "sessionId": session_id,
                        "configOptions": mock_config_options(&session_config)
                    }
                }));
                announce_commands(&session_id);
            }
            "session/set_config_option" => {
                let config_id = params
                    .get("configId")
                    .and_then(Value::as_str)
                    .unwrap_or("model");
                let value = params
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("mock-model-a");
                if config_id == "model" {
                    session_config = value.to_owned();
                }
                write(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "configOptions": mock_config_options(&session_config) }
                }));
            }
            "session/resume" => {
                // Echo the id the client asked for, as a real agent does.
                session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("mock-session")
                    .to_owned();
                write(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "sessionId": session_id,
                        "configOptions": mock_config_options(&session_config)
                    }
                }));
                announce_commands(&session_id);
            }
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
                                "sessionId": session_id,
                                "update": {
                                    "sessionUpdate": "tool_call",
                                    "toolCallId": "call-1",
                                    "title": "Read src/main.rs",
                                    "kind": "read",
                                    "status": "pending"
                                }
                            }
                        }));
                        chunk(&session_id, "Checking the file first.");
                        finish(&id, "end_turn");
                    }
                    "permission" | "permission-delete" => {
                        // A delete is the one action Auto must still ask about.
                        let is_delete = scenario == "permission-delete";
                        // `allow_always` is offered on purpose: the client must
                        // never pick it.
                        write(json!({
                            "jsonrpc": "2.0",
                            "method": "session/request_permission",
                            "id": 9001,
                            "params": {
                                "sessionId": session_id,
                                "toolCall": {
                                    "toolCallId": "call-1",
                                    "title": if is_delete { "Delete build/" } else { "Run tests" },
                                    "kind": if is_delete { "delete" } else { "execute" },
                                    "locations": [
                                        { "path": "/tmp/example/src/main.rs", "line": 4 }
                                    ],
                                    // A delete has no diff to preview; every
                                    // other permission request shows one.
                                    "content": if is_delete { json!([]) } else { json!([
                                        {
                                            "type": "diff",
                                            "path": "/tmp/example/src/main.rs",
                                            "oldText": "fn main() {\n    old();\n}\n",
                                            "newText": "fn main() {\n    new();\n}\n"
                                        }
                                    ]) }
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
                    other if other.starts_with("stop:") => {
                        // `stop:<reason>` answers with that stop reason, so a
                        // test can drive refusal, cancellation and limits.
                        let reason = other.trim_start_matches("stop:");
                        chunk(&session_id, &format!("turn {turn} answer: {prompt}"));
                        finish(&id, reason);
                    }
                    _ => {
                        chunk(&session_id, &format!("turn {turn} answer: {prompt}"));
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
