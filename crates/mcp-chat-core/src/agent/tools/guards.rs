//! Pre-approval guards and the rules that decide when a call needs approval.

use super::workspace::{resolve_write_path, workspace_root_for_conversation};
use super::{ToolExecRequest, ToolExecutor};
use crate::agent::permissions::Effect;
use regex::RegexBuilder;
use serde_json::Value;

/// Returned when `write` would replace a file that already exists.
pub const WRITE_EXISTS_MESSAGE: &str = "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file.";

/// The model's refusal message when a bash command writes file contents and
/// `allowFileWrites` is not set. `None` means the command may run.
pub(super) fn bash_write_block(args: &Value) -> Option<&'static str> {
    let command = args.get("command").and_then(Value::as_str)?;
    let allowed = args
        .get("allowFileWrites")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (!allowed && crate::agent::shell_guard::writes_file_contents(command))
        .then_some(crate::agent::shell_guard::BLOCKED_MESSAGE)
}

/// Shell commands that are destructive enough to keep asking even when
/// auto-approve is on for the conversation.
pub fn is_dangerous_command(command: &str) -> bool {
    let normalized = command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    let destructive =
        RegexBuilder::new(r"(^|[;&|]\s*)(sudo\s+)?(rm|del|erase|rmdir|rd|unlink|shred)\b")
            .case_insensitive(true)
            .build()
            .map(|regex| regex.is_match(&normalized))
            .unwrap_or(false);
    if destructive {
        return true;
    }
    const PATTERNS: &[&str] = &[
        "format c:",
        "format /",
        "diskpart",
        "mkfs",
        "dd if=",
        "of=/dev/sd",
        "shutdown",
        "reboot",
        "poweroff",
        "git reset --hard",
        "git clean -fd",
        "git clean -f",
        "git checkout --",
        "remove-item",
        "remove-item -recurse",
        "remove-item -force",
        "invoke-expression",
        "taskkill",
        "stop-process",
        "reg delete",
        "reg add",
        "bcdedit",
        "bootrec",
        "takeown",
        "icacls",
        "chmod -r",
        "rd /s",
        "rmdir /s",
        "chown -r",
        "sc delete",
        "net user",
        "netsh ",
        "set-executionpolicy",
        "| sh",
        "| bash",
        "| powershell",
        "| iex",
        ":(){",
    ];
    PATTERNS.iter().any(|pattern| normalized.contains(pattern))
}

pub(super) fn is_dangerous_tool_call(req: &ToolExecRequest) -> bool {
    if req.remote_name == "bash" {
        return req
            .args
            .get("command")
            .and_then(Value::as_str)
            .map(is_dangerous_command)
            .unwrap_or(true);
    }
    if req.remote_name == "apply_patch" {
        return req
            .args
            .get("patchText")
            .and_then(Value::as_str)
            .map(|patch| {
                patch
                    .lines()
                    .any(|line| line.starts_with("*** Delete File:"))
            })
            .unwrap_or(true);
    }
    false
}

pub(super) fn auto_effective_effect(req: &ToolExecRequest, effect: Effect) -> Effect {
    if req.auto_approve && effect == Effect::Ask && !is_dangerous_tool_call(req) {
        // An explicit shell file-write escape must remain a reviewed action,
        // even when the chat is in auto mode. Otherwise the model can bypass
        // the routing guard by setting allowFileWrites itself.
        if req.remote_name == "bash"
            && req
                .args
                .get("allowFileWrites")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return Effect::Ask;
        }
        Effect::Allow
    } else {
        effect
    }
}

impl ToolExecutor {
    /// Guards that need no approval to evaluate. Outside-workspace paths are
    /// skipped here: they are checked after the separate outside-write approval.
    pub(super) async fn precheck_builtin(&self, req: &ToolExecRequest) -> Option<String> {
        match req.remote_name.as_str() {
            "bash" => bash_write_block(&req.args).map(str::to_owned),
            "write" => {
                let overwrite = req
                    .args
                    .get("overwrite")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if overwrite {
                    return None;
                }
                let raw = req.args.get("filePath").and_then(Value::as_str)?;
                let workspace =
                    workspace_root_for_conversation(&self.db.pool, &req.conversation_id)
                        .await
                        .ok()?;
                let path = resolve_write_path(&workspace, raw, false).ok()?;
                let exists = tokio::fs::metadata(&path)
                    .await
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false);
                exists.then(|| WRITE_EXISTS_MESSAGE.to_owned())
            }
            _ => None,
        }
    }
}
