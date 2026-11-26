//! Process launching and teardown for ACP agents.
//!
//! Jarvis spawns the child itself instead of using the SDK's `process` feature:
//!
//! - `AcpAgentConfig` has no `cwd`, and agents read their config relative to it,
//! - the SDK only kills the direct child on Windows, so `npx` wrappers leave
//!   orphaned `node` processes (verified in Phase 0),
//! - the MCP stdio wrapper for `npx`/`.cmd` is already proven here.

use std::path::Path;
use std::process::Stdio;

use tokio::process::{Child, Command};

use super::error::AcpError;

/// How many stderr lines to keep for a crash message.
const STDERR_TAIL_LINES: usize = 40;

/// A launched ACP agent process.
pub struct AgentProcess {
    pub child: Child,
    command: String,
}

impl AgentProcess {
    /// Spawn the configured command.
    ///
    /// `cwd` is passed to the child directly. On Unix the child is placed in its
    /// own process group so the whole tree can be terminated; on Windows the
    /// caller must use [`terminate`].
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
    ) -> Result<Self, AcpError> {
        if command.trim().is_empty() {
            return Err(AcpError::SpawnFailed {
                command: command.to_owned(),
                message: "the command is empty".into(),
            });
        }

        let mut child_command = build_command(command, args);
        child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            child_command.current_dir(cwd);
        }
        for (key, value) in env {
            child_command.env(key, value);
        }
        #[cfg(unix)]
        {
            // Own process group: `npx`/`uvx` wrappers have children of their own.
            child_command.process_group(0);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            child_command.creation_flags(CREATE_NO_WINDOW);
        }

        let child = child_command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AcpError::NotFound {
                    command: command.to_owned(),
                }
            } else {
                AcpError::SpawnFailed {
                    command: command.to_owned(),
                    message: error.to_string(),
                }
            }
        })?;

        Ok(Self {
            child,
            command: command.to_owned(),
        })
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Ask the process to exit, wait, then force it.
    ///
    /// The whole process tree is terminated, not just the direct child.
    pub async fn terminate(&mut self) {
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                // Negative pid targets the process group created above.
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(format!("-{pid}"))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            if tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait())
                .await
                .is_err()
            {
                if let Some(pid) = self.child.id() {
                    let _ = std::process::Command::new("kill")
                        .arg("-KILL")
                        .arg(format!("-{pid}"))
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = self.child.wait().await;
            }
        }
        #[cfg(windows)]
        {
            kill_tree(self.child.id());
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait()).await;
            let _ = self.child.start_kill();
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
    }
}

#[cfg(windows)]
fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        // `taskkill /T` walks the child tree, which the SDK does not do.
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Build the platform command, wrapping package runners and `.cmd` shims.
pub fn build_command(program: &str, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let lower = program.to_ascii_lowercase();
        let script = lower.ends_with(".cmd") || lower.ends_with(".bat");
        let package_runner = matches!(
            lower.as_str(),
            "npx" | "npm" | "pnpm" | "yarn" | "bunx" | "uvx"
        );
        if script || package_runner {
            let mut command = Command::new("cmd.exe");
            command
                .arg("/C")
                .arg(if lower == "npx" { "npx.cmd" } else { program });
            command.args(args);
            return command;
        }
    }
    let mut command = Command::new(program);
    command.args(args);
    command
}

/// Keep only the last few stderr lines so a crash message stays bounded.
#[derive(Default)]
pub struct StderrTail {
    lines: std::collections::VecDeque<String>,
}

impl StderrTail {
    pub fn push(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > STDERR_TAIL_LINES {
            self.lines.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

impl std::fmt::Display for StderrTail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                formatter.write_str("\n")?;
            }
            formatter.write_str(line)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_tail_is_bounded_and_keeps_the_newest_lines() {
        let mut tail = StderrTail::default();
        for index in 0..(STDERR_TAIL_LINES + 10) {
            tail.push(format!("line {index}"));
        }
        let text = tail.to_string();
        assert!(!text.contains("line 0\n"));
        assert!(text.contains(&format!("line {}", STDERR_TAIL_LINES + 9)));
        assert_eq!(text.lines().count(), STDERR_TAIL_LINES);
    }

    #[tokio::test]
    async fn missing_binary_reports_not_found() {
        let error = AgentProcess::spawn("jarvis-definitely-missing-agent-binary", &[], &[], None)
            .err()
            .expect("spawn should fail");
        assert!(matches!(error, AcpError::NotFound { .. }), "got {error:?}");
    }

    #[tokio::test]
    async fn spawns_and_terminates_a_real_process() {
        #[cfg(unix)]
        let (program, args) = ("sleep", vec!["30".to_owned()]);
        #[cfg(windows)]
        let (program, args) = (
            "powershell",
            vec![
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "Start-Sleep -Seconds 30".to_owned(),
            ],
        );
        let started = std::time::Instant::now();
        let mut process = AgentProcess::spawn(program, &args, &[], None).unwrap();
        assert!(process.id().is_some());
        process.terminate().await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "terminate should not hang"
        );
    }
}
