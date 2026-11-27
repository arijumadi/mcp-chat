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

        let mut child_command = build_command(&resolve_program(command), args);
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
    let Some(pid) = pid else { return };
    // `taskkill /T` walks the child tree, which the SDK does not do. Without
    // CREATE_NO_WINDOW this spawns a visible console window for a moment —
    // the "terminal flashes when I switch away" report.
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

/// Find the program when it is not on `PATH`.
///
/// A desktop session starts its shell from a login profile, so `pip install
/// --user`, `npm -g`, and vendor installers all land on `PATH`. A systemd unit
/// or a container does not: it inherits a minimal `PATH` where `~/.local/bin`
/// and `~/.cargo/bin` are missing, and an agent that is installed and working
/// reports "command not found".
///
/// The user's own bin directories are checked first, then the interactive
/// shell is asked. That last step matters for tools whose installer appends its
/// directory to `PATH` in `.bashrc` (hermes, asdf, nvm, conda): those lines are
/// skipped for non-interactive children, so the directory is not discoverable
/// any other way. An explicit `PATH` entry always wins, and a name that already
/// contains a separator is left alone so an absolute path keeps working.
fn resolve_program(program: &str) -> String {
    let program = program.trim();
    if program.is_empty() || program.contains(std::path::MAIN_SEPARATOR) {
        return program.to_owned();
    }
    #[cfg(windows)]
    {
        // Windows already searches the current directory and PATHEXT, and its
        // per-user install location is not these directories.
        program.to_owned()
    }
    #[cfg(unix)]
    {
        if let Some(found) = search_path(program) {
            return found;
        }
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            for relative in [".local/bin", ".cargo/bin", ".bun/bin", "bin"] {
                let candidate = home.join(relative).join(program);
                if is_executable(&candidate) {
                    return candidate.display().to_string();
                }
            }
        }
        if let Some(found) = resolve_via_shell(program, &shell_program()) {
            return found;
        }
        program.to_owned()
    }
}

/// The user's login shell, which is where `PATH` additions actually live.
#[cfg(unix)]
fn shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
}

/// Ask an interactive shell where the program is.
///
/// `-i` is the point: rc files start with "if not interactive, return", so a
/// plain `sh -c` never sees the directory the installer added. The program is
/// passed through the environment rather than pasted into the script, so a
/// name with quotes or `$` cannot become shell code. A shell that hangs on a
/// prompt is killed after [`SHELL_LOOKUP_TIMEOUT`] and the caller falls back to
/// the bare name.
#[cfg(unix)]
fn resolve_via_shell(program: &str, shell: &str) -> Option<String> {
    use std::time::{Duration, Instant};

    const SHELL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
    const SHELL_LOOKUP_VAR: &str = "JARVIS_RESOLVE_PROGRAM";

    let mut child = std::process::Command::new(shell)
        .args(["-ic", r#"command -v "$JARVIS_RESOLVE_PROGRAM""#])
        .env(SHELL_LOOKUP_VAR, program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + SHELL_LOOKUP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let output = child.wait_with_output().ok()?;
    // rc files may print banners; keep the last absolute path that is really
    // executable, which is what `command -v` prints for a real binary.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('/') && is_executable(Path::new(line)))
        .map(str::to_owned)
}

#[cfg(unix)]
fn search_path(program: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(program);
        if is_executable(&candidate) {
            return Some(candidate.display().to_string());
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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

#[cfg(all(test, unix))]
mod resolve_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// `HOME`, `PATH` and `SHELL` are process-wide, so the tests that touch
    /// them take turns. Cargo runs tests on parallel threads by default.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_program(path: &std::path::Path) {
        std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A systemd unit has a minimal PATH. A vendor agent installed into
    /// `~/.local/bin` is then invisible, and the report is "command not found"
    /// for something that is installed and working.
    #[test]
    fn finds_an_agent_in_the_user_bin_directory() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join("fake-acp-agent");
        write_program(&program);

        // SAFETY: ENV_LOCK serialises every test that reads or writes these.
        let previous_home = std::env::var_os("HOME");
        let previous_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("HOME", home.path());
            // Deliberately without the user bin directory, like systemd.
            std::env::set_var("PATH", "/usr/bin:/bin");
        }

        let resolved = resolve_program("fake-acp-agent");

        unsafe {
            match previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match previous_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }

        assert_eq!(
            resolved,
            program.display().to_string(),
            "a fallback must find the agent the unit cannot"
        );
    }

    /// Hermes and friends install into their own directory and add it to PATH
    /// from `.bashrc`. That line is skipped for non-interactive children, so
    /// neither PATH nor the standard user bin directories can see it.
    #[test]
    fn asks_the_interactive_shell_for_a_program_only_it_knows() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("agent-root").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join("fake-hermes");
        write_program(&program);
        std::fs::write(
            home.path().join(".bashrc"),
            format!("export PATH=\"{}:$PATH\"\n", bin.display()),
        )
        .unwrap();

        // SAFETY: ENV_LOCK serialises every test that reads or writes these.
        let previous_home = std::env::var_os("HOME");
        let previous_path = std::env::var_os("PATH");
        let previous_shell = std::env::var_os("SHELL");
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("PATH", "/usr/bin:/bin");
            std::env::set_var("SHELL", "/bin/bash");
        }

        let resolved = resolve_program("fake-hermes");

        unsafe {
            match previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match previous_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match previous_shell {
                Some(value) => std::env::set_var("SHELL", value),
                None => std::env::remove_var("SHELL"),
            }
        }

        assert_eq!(resolved, program.display().to_string());
    }

    #[test]
    fn leaves_an_absolute_path_alone() {
        assert_eq!(resolve_program("/opt/agent/bin/agy"), "/opt/agent/bin/agy");
    }
}
