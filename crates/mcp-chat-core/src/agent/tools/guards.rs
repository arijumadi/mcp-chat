//! Pre-approval guards and the rules that decide when a call needs approval.

use super::workspace::{resolve_write_path, workspace_root_for_conversation};
use super::{ToolExecRequest, ToolExecutor};
use crate::agent::capabilities::{DangerKind, Guard, GuardKind, table};
use crate::agent::permissions::Effect;
use regex::RegexBuilder;
use serde_json::Value;

/// Whether this call sets the guard's override flag (`overwrite: true`,
/// `allowFileWrites: true`).
pub(super) fn override_set(guard: &Guard, args: &Value) -> bool {
    args.get(&guard.override_param)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The refusal for a guarded shell tool, if its command writes file contents
/// and the override is not set. The wording comes from the capability table.
pub(super) fn shell_write_refusal(tool: &str, args: &Value) -> Option<String> {
    let guard = table().get(tool)?.guard.as_ref()?;
    if guard.kind != GuardKind::ShellContentWrite || override_set(guard, args) {
        return None;
    }
    let command = args.get("command").and_then(Value::as_str)?;
    crate::agent::shell_guard::writes_file_contents(command).then(|| guard.blocked_message.clone())
}

/// The refusal for a tool that must not replace an existing file, when the
/// override is not set. The caller checks whether the file exists.
pub(super) fn overwrite_guard(tool: &str, args: &Value) -> Option<&'static Guard> {
    let guard = table().get(tool)?.guard.as_ref()?;
    (guard.kind == GuardKind::NoOverwrite && !override_set(guard, args)).then_some(guard)
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
    let Some(danger) = table()
        .get(&req.remote_name)
        .and_then(|capability| capability.danger.as_ref())
    else {
        return false;
    };
    let value = req.args.get(&danger.arg).and_then(Value::as_str);
    match danger.kind {
        DangerKind::ShellCommand => value.map(is_dangerous_command).unwrap_or(true),
        DangerKind::PatchDelete => value
            .map(|patch| {
                patch
                    .lines()
                    .any(|line| line.starts_with("*** Delete File:"))
            })
            .unwrap_or(true),
    }
}

pub(super) fn auto_effective_effect(req: &ToolExecRequest, effect: Effect) -> Effect {
    if req.auto_approve && effect == Effect::Ask && !is_dangerous_tool_call(req) {
        // An explicit escape from a routing guard must remain a reviewed
        // action, even when the chat is in auto mode. Otherwise the model can
        // bypass the guard by setting the flag itself.
        let escaped = table()
            .get(&req.remote_name)
            .and_then(|capability| capability.guard.as_ref())
            .is_some_and(|guard| guard.override_needs_review && override_set(guard, &req.args));
        if escaped {
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
        let guard = table().get(&req.remote_name)?.guard.as_ref()?;
        match guard.kind {
            GuardKind::ShellContentWrite => shell_write_refusal(&req.remote_name, &req.args),
            GuardKind::NoOverwrite => {
                overwrite_guard(&req.remote_name, &req.args)?;
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
                exists.then(|| guard.blocked_message.clone())
            }
        }
    }
}
