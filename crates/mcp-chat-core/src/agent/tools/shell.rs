//! Shell and Python execution, and the output cleanup they need.

use super::guards::shell_write_refusal;
use super::workspace::{resolve_in_workspace, workspace_root_for_conversation};
use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::{CoreError, Result};
use base64::Engine;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

pub(super) const PS_PROLOG: &str = "$ProgressPreference='SilentlyContinue';$env:PYTHONIOENCODING='utf-8';$env:PYTHONUTF8='1';[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false);$OutputEncoding=[Console]::OutputEncoding;";

#[cfg(windows)]
pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(super) fn shell_launch_error(shell: &str, error: std::io::Error) -> CoreError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CoreError::Message(format!(
            "Shell '{shell}' was not found on this system. Install it or choose another shell (cmd, powershell, pwsh, sh or bash)."
        ))
    } else {
        CoreError::Message(format!("Shell '{shell}' could not be started: {error}"))
    }
}

pub(super) fn decode_clixml_escapes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'_'
            && index + 7 <= bytes.len()
            && bytes[index + 1] == b'x'
            && bytes[index + 6] == b'_'
        {
            let hex = &value[index + 2..index + 6];
            if hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        output.push(ch);
                        index += 7;
                        continue;
                    }
                }
            }
        }
        let ch = value[index..].chars().next().expect("valid utf-8");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

pub(super) fn extract_clixml_errors(block: &str) -> String {
    let mut output = String::new();
    let mut rest = block;
    while let Some(start) = rest.find("<S S=\"Error\">") {
        let after = &rest[start + "<S S=\"Error\">".len()..];
        let Some(end) = after.find("</S>") else {
            break;
        };
        let text = decode_clixml_escapes(&after[..end]);
        output.push_str(text.trim_end());
        output.push('\n');
        rest = &after[end + "</S>".len()..];
    }
    output
}

/// PowerShell writes progress records to stderr as CLIXML when stderr is
/// redirected. Strip those blocks, but keep serialized error text if present.
pub(super) fn strip_powershell_prolog(value: &str) -> String {
    let needle = PS_PROLOG.as_bytes();
    let mut output = String::new();
    let mut rest = value;
    loop {
        // Ignore whitespace when locating the prolog so PowerShell's arbitrary
        // line wrapping (and indentation) does not hide the tail.
        let mut compact = Vec::with_capacity(rest.len());
        let mut origins = Vec::with_capacity(rest.len());
        for (index, ch) in rest.char_indices() {
            if ch.is_whitespace() {
                continue;
            }
            let bytes = &rest.as_bytes()[index..index + ch.len_utf8()];
            for (offset, byte) in bytes.iter().enumerate() {
                compact.push(*byte);
                origins.push(index + offset);
            }
        }
        let Some(position) = compact
            .windows(needle.len())
            .position(|window| window == needle)
        else {
            output.push_str(rest);
            break;
        };
        let start = origins[position];
        let end = origins[position + needle.len() - 1] + 1;
        output.push_str(&rest[..start]);
        rest = rest[end..].trim_start_matches(['\r', '\n']);
    }
    output
}

pub(super) fn clean_powershell_stderr(value: &str) -> String {
    let mut output = String::new();
    let mut rest = value;
    loop {
        let marker = rest.find("#< CLIXML");
        let block = rest.find("<Objs");
        let start = match (marker, block) {
            (Some(marker), Some(block)) => marker.min(block),
            (Some(marker), None) => marker,
            (None, Some(block)) => block,
            (None, None) => {
                output.push_str(rest);
                break;
            }
        };
        output.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(objects) = tail.find("<Objs") else {
            break;
        };
        let Some(end) = tail[objects..].find("</Objs>") else {
            break;
        };
        let block_end = objects + end + "</Objs>".len();
        output.push_str(&extract_clixml_errors(&tail[objects..block_end]));
        rest = &tail[block_end..];
    }
    // The encoding/progress prolog is part of the PowerShell error context on
    // some versions, and PowerShell may wrap it across lines.
    strip_powershell_prolog(&output).trim_end().to_owned()
}

pub(super) fn normalize_shell_text(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn powershell_script(command: &str) -> String {
    format!(
        "{PS_PROLOG}\n$ErrorActionPreference = 'Stop'\ntry {{\n{command}\nif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}\nif (-not $?) {{ exit 1 }}\nexit 0\n}} catch {{\n[Console]::Error.WriteLine($_.Exception.Message)\nexit 1\n}}"
    )
}

pub(super) fn shell_echoed(command: &str, stdout: &str) -> bool {
    !stdout.trim().is_empty() && normalize_shell_text(stdout) == normalize_shell_text(command)
}

/// Only cmd.exe is allowed to trigger the echo heuristic. PowerShell's
/// -EncodedCommand does not echo the script, and a command such as `$ "1+1"`
/// legitimately produces output identical to its command text.
pub(super) fn echo_suspect(shell: &str, command: &str, stdout: &str) -> bool {
    shell == "cmd" && shell_echoed(command, stdout)
}

pub(super) fn clip_lines(text: &str, max_lines: usize, tail: bool) -> (String, usize, bool) {
    let total = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    if max_lines == 0 || total <= max_lines {
        return (text.to_owned(), total, false);
    }
    let lines: Vec<&str> = text.lines().collect();
    let selected = if tail {
        &lines[total - max_lines..]
    } else {
        &lines[..max_lines]
    };
    (selected.join("\n"), total, true)
}

impl ToolExecutor {
    pub(super) async fn run_bash(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let command = req
            .args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("command is required"))?;
        if let Some(message) = shell_write_refusal(&req.remote_name, &req.args) {
            return Err(CoreError::invalid(message));
        }
        let timeout_ms = req
            .args
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);
        let max_lines = req
            .args
            .get("maxLines")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let tail = req
            .args
            .get("tail")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let requested_shell = req
            .args
            .get("shell")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let shell = match requested_shell {
            "auto" => {
                if cfg!(windows) {
                    "powershell"
                } else {
                    "sh"
                }
            }
            "cmd" | "powershell" | "pwsh" | "sh" | "bash" => requested_shell,
            other => {
                return Err(CoreError::invalid(format!(
                    "Unknown shell {other:?}. Use auto, cmd, powershell, pwsh, sh or bash."
                )));
            }
        };
        if matches!(shell, "cmd" | "powershell") && !cfg!(windows) {
            return Err(CoreError::invalid(format!(
                "The {shell} shell is only available on Windows."
            )));
        }
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let cwd = match req.args.get("cwd").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => resolve_in_workspace(&workspace, raw)?,
            _ => workspace.clone(),
        };
        if !cwd.is_dir() {
            return Err(CoreError::invalid(format!(
                "cwd is not a directory: {}",
                cwd.display()
            )));
        }

        let mut cleanup: Option<PathBuf> = None;
        let mut child = match shell {
            "cmd" => {
                // A batch file avoids the cmd.exe/MSVC nested-quote problem. The
                // user command is written verbatim, one line at a time.
                let script_dir = std::env::temp_dir().join("mcp-chat-shell");
                tokio::fs::create_dir_all(&script_dir).await?;
                let script_path = script_dir.join(format!("{}.cmd", crate::new_id()));
                let script = format!("@echo off\r\nchcp 65001>nul\r\n{command}\r\n");
                tokio::fs::write(&script_path, script).await?;
                cleanup = Some(script_path.clone());
                let mut process = Command::new("cmd.exe");
                process.arg("/D").arg("/S").arg("/C").arg(&script_path);
                process
            }
            "powershell" | "pwsh" => {
                // -EncodedCommand takes UTF-16LE/base64, so neither Rust nor
                // PowerShell has to guess how nested quotes should be escaped.
                let prepared = powershell_script(command);
                let encoded = base64::engine::general_purpose::STANDARD.encode(
                    prepared
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<u8>>(),
                );
                let executable = if shell == "pwsh" {
                    "pwsh"
                } else {
                    "powershell.exe"
                };
                let mut process = Command::new(executable);
                process.args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &encoded,
                ]);
                process
            }
            "bash" => {
                let mut process = Command::new("bash");
                process.arg("-lc").arg(command);
                process
            }
            _ => {
                let mut process = Command::new("sh");
                process.arg("-lc").arg(command);
                process
            }
        };
        // Release builds use the Windows GUI subsystem, so there is no console
        // to inherit. Without this flag Windows opens a new terminal for every
        // shell tool call.
        #[cfg(windows)]
        child.creation_flags(CREATE_NO_WINDOW);
        child
            .current_dir(&cwd)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());

        let output = tokio::select! {
            _ = req.cancel.cancelled() => {
                if let Some(path) = cleanup {
                    let _ = tokio::fs::remove_file(path).await;
                }
                return Err(CoreError::Cancelled);
            }
            result = tokio::time::timeout(Duration::from_millis(timeout_ms), child.output()) => {
                match result {
                    Ok(Ok(output)) => output,
                    Ok(Err(error)) => {
                        if let Some(path) = cleanup {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        return Err(shell_launch_error(shell, error));
                    }
                    Err(_) => {
                        if let Some(path) = cleanup {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                        return Err(CoreError::Message(format!(
                            "Command timed out after {timeout_ms} ms."
                        )));
                    }
                }
            }
        };
        if let Some(path) = cleanup {
            let _ = tokio::fs::remove_file(path).await;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stderr = if matches!(shell, "powershell" | "pwsh") {
            clean_powershell_stderr(&stderr_raw)
        } else {
            stderr_raw.into_owned()
        };
        let (stdout, stdout_total, stdout_clipped) = clip_lines(&stdout, max_lines, tail);
        let (stderr, stderr_total, stderr_clipped) = clip_lines(&stderr, max_lines, tail);
        let echoed = echo_suspect(shell, command, &stdout);
        let exit_code = output.status.code();
        let exit = exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated".into());
        let mut content = String::new();
        if echoed {
            content.push_str(
                "[warning] The shell echoed the command and did not execute it; this is usually a quoting problem. Change the shell or write a script file.\n",
            );
        }
        content.push_str(&format!(
            "[shell] {shell}\n[cwd] {}\n$ {command}\n[stdout]\n",
            cwd.display()
        ));
        if stdout.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stdout.trim_end());
            content.push('\n');
        }
        if stdout_clipped {
            content.push_str(&format!(
                "... ({} lines total, showing {} {max_lines})\n",
                stdout_total,
                if tail { "last" } else { "first" }
            ));
        }
        content.push_str("[stderr]\n");
        if stderr.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stderr.trim_end());
            content.push('\n');
        }
        if stderr_clipped {
            content.push_str(&format!(
                "... ({} lines total, showing {} {max_lines})\n",
                stderr_total,
                if tail { "last" } else { "first" }
            ));
        }
        content.push_str(&format!("[exit code {exit}]"));
        Ok(Rendered {
            content,
            structured: Some(json!({
                "exitCode": exit_code,
                "success": output.status.success(),
                "echoed": echoed,
            })),
            // A non-zero exit code is command output, not a tool execution
            // failure. Only quoting/echo and termination are tool failures.
            ok: exit_code.is_some() && !echoed,
        })
    }

    pub(super) async fn run_python(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let code = req
            .args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::invalid("code is required"))?;
        let timeout_ms = req
            .args
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let cwd = match req.args.get("cwd").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => resolve_in_workspace(&workspace, raw)?,
            _ => workspace.clone(),
        };
        if !cwd.is_dir() {
            return Err(CoreError::invalid(format!(
                "cwd is not a directory: {}",
                cwd.display()
            )));
        }

        let executable = if cfg!(windows) { "python" } else { "python3" };
        let mut process = Command::new(executable);
        process
            .args(["-u", "-"])
            .current_dir(&cwd)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        process.creation_flags(CREATE_NO_WINDOW);

        let mut child = process.spawn().map_err(|error| {
            CoreError::Message(format!(
                "Python could not be started ({executable}). Install Python 3 or use shell=bash/powershell. {error}"
            ))
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            tokio::io::AsyncWriteExt::write_all(&mut stdin, code.as_bytes()).await?;
        }
        let output =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
                .await
            {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => {
                    return Err(CoreError::Message(format!(
                        "Python process failed: {error}"
                    )));
                }
                Err(_) => {
                    return Err(CoreError::Message(format!(
                        "Python timed out after {timeout_ms} ms."
                    )));
                }
            };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code();
        let exit = exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated".into());
        let mut content = format!("[python]\n[cwd] {}\n[stdout]\n", cwd.display());
        if stdout.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stdout.trim_end());
            content.push('\n');
        }
        content.push_str("[stderr]\n");
        if stderr.trim().is_empty() {
            content.push_str("(empty)\n");
        } else {
            content.push_str(stderr.trim_end());
            content.push('\n');
        }
        content.push_str(&format!("[exit code {exit}]"));
        Ok(Rendered {
            content,
            structured: Some(json!({
                "exitCode": exit_code,
                "success": output.status.success(),
            })),
            ok: exit_code.is_some(),
        })
    }
}
