//! Unit tests for the tool executor and its helpers.

use super::defs::{
    builtin_default_effect, builtin_tool_def, builtin_tool_defs, builtin_tool_info, is_builtin_tool,
};
use super::files::{line_prefix, strip_utf8_bom};
use super::guards::{auto_effective_effect, is_dangerous_command};
use super::output::{READ_PAGE_CHARS, page_text, read_spilled_lines};
use super::shell::{
    PS_PROLOG, clean_powershell_stderr, clip_lines, echo_suspect, normalize_shell_text,
    powershell_script, shell_echoed, shell_launch_error,
};
use super::skills::{set_skill_enabled, skill_roots_with_home};
use super::ui::take_ui_diff;
use super::workspace::{
    outside_read_target, path_is_outside, resolve_in_workspace, resolve_write_path, workspace_root,
    workspace_root_for_conversation,
};
use super::{Rendered, ToolExecRequest, ToolExecResult, ToolExecutor, validate_args};
use crate::agent::approvals::{ApprovalDecision, ApprovalRegistry, InputAnswer, InputRegistry};
use crate::agent::file_locks::FileLocks;
use crate::agent::permissions::Effect;
use crate::db::Db;
use crate::db::repo::{conversations, messages, permissions as rules_repo, settings};
use crate::events::CoreEvent;
use crate::mcp::manager::McpManager;
use crate::models::PartPayload;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

// The exact wording the model is told. Kept literal on purpose: it must not
// change just because the capability table does.
const WRITE_EXISTS_MESSAGE: &str = "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file.";
const BLOCKED_MESSAGE: &str = "BLOCKED: bash tidak boleh mengubah isi file. Pakai write (file baru) atau apply_patch (file existing). Untuk operasi yang memang disengaja, set allowFileWrites=true.";

#[test]
fn workspace_paths_cannot_escape() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    assert!(resolve_in_workspace(&workspace, "src/main.rs").is_ok());
    assert!(resolve_in_workspace(&workspace, "../outside.txt").is_err());
    assert!(resolve_in_workspace(&workspace, "src/../../outside.txt").is_err());
}

#[test]
fn absolute_paths_inside_the_workspace_are_accepted() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let target = workspace.join("notes").join("inside.txt");
    let resolved = resolve_in_workspace(&workspace, target.to_str().unwrap()).unwrap();
    assert_eq!(resolved, target);
}

#[test]
fn builtin_tool_schema_stays_compact() {
    let chars: usize = builtin_tool_defs()
        .iter()
        .map(|tool| {
            tool.name.len()
                + tool.description.len()
                + serde_json::to_string(&tool.input_schema)
                    .map(|schema| schema.len())
                    .unwrap_or(0)
        })
        .sum();
    println!("builtin tool schema chars: {chars}");
    assert!(
        chars < 7_800,
        "builtin tool schema grew unexpectedly: {chars} characters"
    );
}

#[test]
fn pager_pages_align_to_lines_when_possible() {
    let content = (1..=1_000)
        .map(|index| format!("{index:05} | {}\n", "x".repeat(100)))
        .collect::<String>();
    let (start, first, next, total) = page_text(&content, 0);
    assert_eq!(start, 0);
    assert!(next < total);
    assert!(first.ends_with('\n'));
    assert!(first.chars().count() <= READ_PAGE_CHARS);

    let (second_start, second, second_next, _) = page_text(&content, next);
    assert_eq!(second_start, next);
    assert!(second.ends_with('\n'));
    assert!(content.starts_with(&format!("{first}{second}")));

    let (third_start, third, _, _) = page_text(&content, second_next);
    assert_eq!(third_start, second_next);
    assert!(third.ends_with('\n'));
}

#[test]
fn pager_hard_cuts_a_single_long_line_and_counts_unicode() {
    let content = "\u{e9}".repeat(READ_PAGE_CHARS + 100);
    let (start, page, next, total) = page_text(&content, 0);
    assert_eq!(start, 0);
    assert_eq!(page.chars().count(), READ_PAGE_CHARS);
    assert_eq!(next, READ_PAGE_CHARS);
    assert_eq!(total, READ_PAGE_CHARS + 100);

    let (second_start, tail, _, _) = page_text(&content, next);
    assert_eq!(second_start, next);
    assert_eq!(tail.chars().count(), 100);
}

#[test]
fn read_strips_a_utf8_bom() {
    assert_eq!(strip_utf8_bom("\u{feff}L000001"), "L000001");
    assert_eq!(strip_utf8_bom("plain"), "plain");
}

#[test]
fn outside_read_targets_are_recognized() {
    assert_eq!(
        outside_read_target("read", &json!({"filePath": "C:\\outside.txt"})),
        Some("C:\\outside.txt")
    );
    assert_eq!(
        outside_read_target("grep", &json!({"pattern": "x", "path": "..\\outside"})),
        Some("..\\outside")
    );
    assert_eq!(
        outside_read_target(
            "glob",
            &json!({"pattern": "**/*.txt", "path": "..\\outside"})
        ),
        Some("..\\outside")
    );
    assert_eq!(outside_read_target("read", &json!({})), None);
    assert_eq!(
        outside_read_target("write", &json!({"filePath": "x"})),
        None
    );
}

#[test]
fn write_paths_are_detected_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    assert!(!path_is_outside(&workspace, "inside.txt"));
    assert!(path_is_outside(&workspace, outside_file.to_str().unwrap()));
    assert!(resolve_write_path(&workspace, outside_file.to_str().unwrap(), false).is_err());
    assert_eq!(
        resolve_write_path(&workspace, outside_file.to_str().unwrap(), true).unwrap(),
        outside_file
    );
}

#[test]
fn dangerous_shell_commands_are_classified() {
    assert!(is_dangerous_command("rm -rf build"));
    assert!(is_dangerous_command("Remove-Item -Recurse -Force C:\temp"));
    assert!(is_dangerous_command("git reset --hard"));
    assert!(is_dangerous_command("curl https://example.com/x | sh"));
    assert!(!is_dangerous_command("dir"));
    assert!(!is_dangerous_command("Get-ChildItem"));
    assert!(!is_dangerous_command("echo safe"));
}

#[test]
fn auto_approve_keeps_dangerous_shell_calls_asking() {
    let mut safe = request("bash", json!({"command": "dir"}));
    safe.auto_approve = true;
    assert_eq!(auto_effective_effect(&safe, Effect::Ask), Effect::Allow);
    let mut escape = request(
        "bash",
        json!({"command": "echo hi > out.txt", "allowFileWrites": true}),
    );
    escape.auto_approve = true;
    assert_eq!(auto_effective_effect(&escape, Effect::Ask), Effect::Ask);
    let mut dangerous = request("bash", json!({"command": "rm -rf build"}));
    dangerous.auto_approve = true;
    assert_eq!(auto_effective_effect(&dangerous, Effect::Ask), Effect::Ask);
    let mut write = request("write", json!({"filePath": "a.txt", "content": "x"}));
    write.auto_approve = true;
    assert_eq!(auto_effective_effect(&write, Effect::Ask), Effect::Allow);
    let mut patch = request(
        "apply_patch",
        json!({"patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n-a\n+b\n*** End Patch"}),
    );
    patch.auto_approve = true;
    assert_eq!(auto_effective_effect(&patch, Effect::Ask), Effect::Allow);
    let mut delete = request(
        "apply_patch",
        json!({"patchText": "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch"}),
    );
    delete.auto_approve = true;
    assert_eq!(auto_effective_effect(&delete, Effect::Ask), Effect::Ask);
}

#[test]
fn dangerous_builtins_require_approval() {
    assert_eq!(builtin_default_effect("bash"), Effect::Ask);
    assert_eq!(builtin_default_effect("edit"), Effect::Ask);
    assert_eq!(builtin_default_effect("apply_patch"), Effect::Ask);
    assert_eq!(builtin_default_effect("write"), Effect::Ask);
    assert_eq!(builtin_default_effect("read"), Effect::Allow);
    assert_eq!(builtin_default_effect("question"), Effect::Allow);
}

#[test]
fn powershell_progress_clixml_is_removed() {
    let raw = "#< CLIXML\n<Objs Version=\"1.1.0.1\"><Obj S=\"progress\"><MS><AV>Preparing modules for first use.</AV></MS></Obj></Objs>\n";
    assert_eq!(clean_powershell_stderr(raw), "");
}

#[test]
fn powershell_error_clixml_is_preserved_and_decoded() {
    let raw = "#< CLIXML\n<Objs Version=\"1.1.0.1\"><Obj S=\"error\"><S S=\"Error\">boom_x000D__x000A_after</S></Obj></Objs>";
    let cleaned = clean_powershell_stderr(raw);
    assert!(cleaned.contains("boom\r\nafter"));
}

#[test]
fn missing_shell_has_a_clear_error() {
    let error = shell_launch_error("bash", std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(error.user_message().contains("Shell 'bash' was not found"));
}

#[test]
fn powershell_wrapper_keeps_stderr_clean() {
    let wrapped = powershell_script("Get-ChildItem nofile.txt");
    assert!(wrapped.contains("$LASTEXITCODE"));
    assert!(wrapped.contains("[Console]::Error.WriteLine"));
    assert!(!wrapped.contains("Write-Error"));
    assert!(wrapped.contains("Get-ChildItem nofile.txt"));
}

#[test]
fn echo_detection_is_limited_to_cmd() {
    assert!(echo_suspect("cmd", "cmd /C dir", "cmd /C dir"));
    assert!(!echo_suspect("powershell", "1+1", "1+1"));
    assert!(!echo_suspect("pwsh", "ok", "ok"));
}

#[test]
fn powershell_plain_stderr_keeps_literal_escape_text() {
    assert_eq!(
        clean_powershell_stderr("literal _x000D_ marker"),
        "literal _x000D_ marker"
    );
}

#[test]
fn powershell_prolog_is_filtered_from_stderr() {
    let raw = format!("{PS_PROLOG}\nreal error");
    assert_eq!(clean_powershell_stderr(&raw), "real error");
}

#[test]
fn wrapped_powershell_prolog_is_filtered_from_stderr() {
    let wrapped = format!("{}\nreal error", PS_PROLOG.replace(';', ";\n"));
    assert_eq!(clean_powershell_stderr(&wrapped), "real error");
}

#[test]
fn shell_echo_normalization_ignores_whitespace_and_edge_quotes() {
    assert_eq!(normalize_shell_text("  cmd   /C  dir \n"), "cmd /C dir");
    assert_eq!(normalize_shell_text("\"echo hi\""), "echo hi");
    assert!(shell_echoed("cmd /C dir", "cmd /C dir\r\n"));
    assert!(!shell_echoed("cmd /C dir", "Volume in drive C"));
}

#[test]
fn line_clipping_supports_head_and_tail() {
    let text = "a\nb\nc\nd\n";
    assert_eq!(clip_lines(text, 2, false).0, "a\nb");
    assert_eq!(clip_lines(text, 2, true).0, "c\nd");
    assert_eq!(clip_lines(text, 0, true).0, text);
    assert_eq!(clip_lines(text, 2, true).1, 4);
    assert!(clip_lines(text, 2, true).2);
}

#[test]
fn line_numbers_start_at_the_requested_offset() {
    let rendered = line_prefix("alpha\nbeta", 41);
    assert!(rendered.starts_with("   41 | alpha\n   42 | beta"));
}

#[tokio::test]
async fn workspace_root_uses_the_saved_setting() {
    let db = Db::open_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    settings::set(&db.pool, "workspace.root", &workspace.display().to_string())
        .await
        .unwrap();
    assert_eq!(workspace_root(&db.pool).await.unwrap(), workspace);
}

#[tokio::test]
async fn conversation_workspace_overrides_the_global_default() {
    let db = Db::open_memory().await.unwrap();
    let global = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    settings::set(
        &db.pool,
        "workspace.root",
        &global.path().canonicalize().unwrap().display().to_string(),
    )
    .await
    .unwrap();
    let conversation = conversations::create(&db.pool, "Test", None, None)
        .await
        .unwrap();
    conversations::set_workspace(
        &db.pool,
        &conversation.id,
        Some(&local.path().canonicalize().unwrap().display().to_string()),
    )
    .await
    .unwrap();
    assert_eq!(
        workspace_root_for_conversation(&db.pool, &conversation.id)
            .await
            .unwrap(),
        local.path().canonicalize().unwrap()
    );
}

async fn test_executor(workspace: &Path) -> ToolExecutor {
    let db = Db::open_memory().await.unwrap();
    sqlx::query(
        "INSERT INTO conversations (id, title, created_at, updated_at, archived, max_steps, tool_mode, workspace_root) VALUES (?1, 'Test', 0, 0, 0, 25, 'lazy', ?2)",
    )
    .bind("test-conversation")
    .bind(workspace.display().to_string())
    .execute(&db.pool)
    .await
    .unwrap();
    let secrets = Arc::new(crate::secrets::MemoryStore::default());
    let (events, _) = broadcast::channel(16);
    ToolExecutor {
        db: db.clone(),
        mcp: Arc::new(McpManager::new(db, secrets)),
        approvals: Arc::new(ApprovalRegistry::default()),
        inputs: Arc::new(InputRegistry::default()),
        events,
        output_dir: workspace.join("tool-output"),
        attachments_dir: workspace.join("attachments"),
        limiter: Arc::new(Semaphore::new(4)),
        file_locks: FileLocks::default(),
    }
}

fn request(name: &str, args: Value) -> ToolExecRequest {
    ToolExecRequest {
        conversation_id: "test-conversation".into(),
        message_id: "m".into(),
        part_id: "p".into(),
        call_id: format!("call-{name}"),
        index: 0,
        exposed_name: name.into(),
        remote_name: name.into(),
        server_id: String::new(),
        server_slug: "builtin".into(),
        server_name: "mcp-chat".into(),
        args,
        tool: builtin_tool_info(name),
        default_effect: builtin_default_effect(name),
        auto_approve: false,
        allow_outside_workspace: false,
        disabled: false,
        cancel: CancellationToken::new(),
        active_server_slugs: Vec::new(),
    }
}

#[tokio::test]
async fn builtin_apply_patch_updates_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("a.txt"), "one\ntwo\n").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_apply_patch(&request(
            "apply_patch",
            json!({
                "patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n one\n-two\n+three\n*** Add File: b.txt\n+new\n*** End Patch\n"
            }),
        ))
        .await
        .unwrap();
    assert!(result.content.contains("M a.txt"));
    assert!(result.content.contains("A b.txt"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "one\nthree\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn builtin_file_tools_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;

    let write = executor
        .run_write(&request(
            "write",
            json!({"filePath": "notes/a.txt", "content": "hello world"}),
        ))
        .await
        .unwrap();
    assert!(write.content.contains("Wrote notes/a.txt"));

    let read = executor
        .run_read(&request("read", json!({"filePath": "notes/a.txt"})))
        .await
        .unwrap();
    assert!(read.content.contains("hello world"));
    assert!(read.content.contains("   1 |"));

    let edit = executor
        .run_edit(&request(
            "edit",
            json!({
                "filePath": "notes/a.txt",
                "oldString": "world",
                "newString": "there"
            }),
        ))
        .await
        .unwrap();
    assert!(edit.content.contains("replaced 1"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes/a.txt")).unwrap(),
        "hello there"
    );
}

#[tokio::test]
async fn builtin_edit_matches_lf_text_in_a_crlf_file() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("windows.txt"), "one\r\ntwo\r\n").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_edit(&request(
            "edit",
            json!({
                "filePath": "windows.txt",
                "oldString": "one\ntwo",
                "newString": "ONE\nTWO"
            }),
        ))
        .await
        .unwrap();
    assert!(result.content.contains("replaced 1"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("windows.txt")).unwrap(),
        "ONE\r\nTWO\r\n"
    );
}

#[tokio::test]
async fn builtin_edit_rejects_a_noop() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("noop.txt"), "same\n").unwrap();
    let executor = test_executor(&workspace).await;
    let error = match executor
        .run_edit(&request(
            "edit",
            json!({
                "filePath": "noop.txt",
                "oldString": "same",
                "newString": "same"
            }),
        ))
        .await
    {
        Ok(_) => panic!("no-op edit should fail"),
        Err(error) => error,
    };
    assert!(error.user_message().contains("identical"));
}

#[tokio::test]
async fn approval_diff_shows_an_edit_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("preview.txt"), "old\n").unwrap();
    let executor = test_executor(&workspace).await;
    let diff = executor
        .approval_diff(
            "test-conversation",
            "",
            "edit",
            &json!({
                "filePath": "preview.txt",
                "oldString": "old",
                "newString": "new"
            }),
        )
        .await
        .unwrap();
    assert!(diff.contains("-old"));
    assert!(diff.contains("+new"));
    assert_eq!(
        std::fs::read_to_string(workspace.join("preview.txt")).unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn edit_returns_a_diff_built_before_the_file_is_written() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("preview.txt"), "keep\nold\n").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_edit(&request(
            "edit",
            json!({
                "filePath": "preview.txt",
                "oldString": "old",
                "newString": "new"
            }),
        ))
        .await
        .unwrap();
    let structured = result.structured.as_ref().unwrap();
    assert_eq!(structured["path"], "preview.txt");
    assert_eq!(structured["replacements"], 1);
    let diff = structured["diff"].as_str().unwrap();
    assert!(diff.contains("--- a/preview.txt"), "{diff}");
    assert!(diff.contains("+++ b/preview.txt"), "{diff}");
    assert!(diff.contains("-old"), "{diff}");
    assert!(diff.contains("+new"), "{diff}");
    // The file is already rewritten. A diff made from it would be empty, so
    // the removed line proves the before-text was captured before the write.
    let on_disk = std::fs::read_to_string(workspace.join("preview.txt")).unwrap();
    assert_eq!(on_disk, "keep\nnew\n");
    assert!(!on_disk.contains("old"));
    // The model-facing text carries no diff.
    assert!(!result.content.contains("@@"), "{}", result.content);
    assert!(!result.content.contains("+new"), "{}", result.content);
}

#[tokio::test]
async fn write_returns_a_diff_against_the_previous_content() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("a.txt"), "keep\nold\n").unwrap();
    std::fs::write(workspace.join("blob.bin"), b"\0\x01\x02").unwrap();
    let executor = test_executor(&workspace).await;

    let overwrite = executor
        .run_write(&request(
            "write",
            json!({"filePath": "a.txt", "content": "keep\nnew\n", "overwrite": true}),
        ))
        .await
        .unwrap();
    let diff = overwrite.structured.as_ref().unwrap()["diff"]
        .as_str()
        .unwrap();
    assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");

    let created = executor
        .run_write(&request(
            "write",
            json!({"filePath": "fresh.txt", "content": "hello\n"}),
        ))
        .await
        .unwrap();
    let diff = created.structured.as_ref().unwrap()["diff"]
        .as_str()
        .unwrap();
    assert!(diff.contains("+++ b/fresh.txt"), "{diff}");
    assert!(diff.contains("+hello"), "{diff}");

    // A binary file cannot be diffed as text. The write must still succeed.
    let binary = executor
        .run_write(&request(
            "write",
            json!({"filePath": "blob.bin", "content": "text\n", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert!(binary.ok);
    assert!(binary.structured.as_ref().unwrap()["diff"].is_null());
    assert_eq!(
        std::fs::read_to_string(workspace.join("blob.bin")).unwrap(),
        "text\n"
    );
}

#[tokio::test]
async fn write_creates_a_new_file_and_its_parent_folders() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_write(&request(
            "write",
            json!({"filePath": "deep/er/new.txt", "content": "hello\n"}),
        ))
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(
        std::fs::read_to_string(workspace.join("deep/er/new.txt")).unwrap(),
        "hello\n"
    );
    assert!(
        result.structured.as_ref().unwrap()["diff"]
            .as_str()
            .unwrap()
            .contains("+hello")
    );
}

#[tokio::test]
async fn write_refuses_to_replace_an_existing_file_by_default() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("keep.txt"), "original\n").unwrap();
    let executor = test_executor(&workspace).await;

    for args in [
        json!({"filePath": "keep.txt", "content": "clobbered\n"}),
        json!({"filePath": "keep.txt", "content": "clobbered\n", "overwrite": false}),
    ] {
        let error = executor
            .run_write(&request("write", args))
            .await
            .err()
            .expect("an existing file must be rejected");
        assert_eq!(error.user_message(), WRITE_EXISTS_MESSAGE);
        assert_eq!(
            std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
            "original\n"
        );
    }
    assert_eq!(
        WRITE_EXISTS_MESSAGE,
        "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file."
    );
}

#[tokio::test]
async fn write_with_overwrite_replaces_the_file_and_diffs_the_old_content() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("keep.txt"), "line\nold\n").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_write(&request(
            "write",
            json!({"filePath": "keep.txt", "content": "line\nnew\n", "overwrite": true}),
        ))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
        "line\nnew\n"
    );
    let diff = result.structured.as_ref().unwrap()["diff"]
        .as_str()
        .unwrap();
    assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");
}

#[tokio::test]
async fn a_rejected_overwrite_is_refused_before_any_approval_prompt() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("keep.txt"), "original\n").unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    // `write` asks for approval by default, so a missing precheck would hang here.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.execute(request(
            "write",
            json!({"filePath": "keep.txt", "content": "clobbered\n"}),
        )),
    )
    .await
    .expect("the call should fail without waiting for approval");
    assert!(!result.ok);
    assert!(!result.executed);
    assert_eq!(result.content, WRITE_EXISTS_MESSAGE);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, CoreEvent::Approval { .. }));
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
        "original\n"
    );
}

#[tokio::test]
async fn bash_refuses_to_write_file_contents() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    for command in [
        "echo hi > out.txt",
        "echo hi >> out.txt",
        "echo hi | tee out.txt",
        "sed -i 's/a/b/' out.txt",
        "Set-Content -Path out.txt -Value hi",
        "'hi' | Out-File out.txt",
    ] {
        let error = executor
            .run_bash(&request("bash", json!({"command": command})))
            .await
            .err()
            .unwrap_or_else(|| panic!("should have been blocked: {command}"));
        let message = error.user_message();
        assert!(
            message.starts_with("BLOCKED: bash tidak boleh mengubah isi file."),
            "{message}"
        );
        assert!(message.contains("allowFileWrites=true"));
        assert!(!workspace.join("out.txt").exists(), "{command}");
    }
}

#[tokio::test]
async fn bash_still_runs_commands_that_do_not_write_file_contents() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let echo = executor
        .run_bash(&request("bash", json!({"command": "echo ok"})))
        .await
        .unwrap();
    assert!(echo.content.contains("ok"));
    // File management is not a content write.
    executor
        .run_bash(&request("bash", json!({"command": "mkdir made"})))
        .await
        .unwrap();
    assert!(workspace.join("made").is_dir());
    // The quoted redirect is text, not a redirect.
    let quoted = executor
        .run_bash(&request("bash", json!({"command": "echo \"a > b\""})))
        .await
        .unwrap();
    assert!(quoted.content.contains("a > b"));
    assert!(!workspace.join("b").exists());
}

#[tokio::test]
async fn allow_file_writes_lets_a_deliberate_bash_write_through() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_bash(&request(
            "bash",
            json!({"command": "echo hi > out.txt", "allowFileWrites": true}),
        ))
        .await
        .unwrap();
    assert!(result.ok);
    assert!(workspace.join("out.txt").is_file());
    // An explicit false is the same as the default.
    assert!(
        executor
            .run_bash(&request(
                "bash",
                json!({"command": "echo hi > other.txt", "allowFileWrites": false}),
            ))
            .await
            .is_err()
    );
    assert!(!workspace.join("other.txt").exists());
}

#[tokio::test]
async fn a_blocked_bash_command_is_refused_before_any_approval_prompt() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.execute(request("bash", json!({"command": "echo hi > out.txt"}))),
    )
    .await
    .expect("the call should fail without waiting for approval");
    assert!(!result.ok);
    assert!(!result.executed);
    assert_eq!(result.content, BLOCKED_MESSAGE);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, CoreEvent::Approval { .. }));
    }
    assert!(!workspace.join("out.txt").exists());
}

#[test]
fn tool_schemas_accept_the_new_flags_and_state_the_routing_rules() {
    let defs = builtin_tool_defs();
    let def = |name: &str| defs.iter().find(|tool| tool.name == name).unwrap();

    let write = def("write");
    assert_eq!(
        write.input_schema["properties"]["overwrite"]["type"],
        "boolean"
    );
    assert!(
        validate_args(
            &write.input_schema,
            &json!({"filePath": "a", "content": "b", "overwrite": true})
        )
        .is_ok()
    );
    assert!(
        write
            .description
            .contains("Existing files are rejected unless overwrite is true")
    );
    assert!(
        write
            .description
            .contains("Use apply_patch to modify an existing file")
    );

    let bash = def("bash");
    assert_eq!(
        bash.input_schema["properties"]["allowFileWrites"]["type"],
        "boolean"
    );
    assert!(
        validate_args(
            &bash.input_schema,
            &json!({"command": "ls", "allowFileWrites": true})
        )
        .is_ok()
    );
    assert!(
        bash.description
            .contains("Do not use bash to read or modify file contents")
    );
    assert!(
        bash.description
            .contains("does this command change the contents of a file? If yes, use a file tool.")
    );

    let edit = def("edit");
    assert!(
        edit.description
            .contains("Replace one exact string in one existing file")
    );
    assert!(edit.description.contains("use apply_patch for multiple hunks, multiple files, add/delete/move, or context-anchored changes"));
    // The matching rules stay in the description.
    assert!(edit.description.contains("CRLF"));
    assert!(edit.description.contains("typographic punctuation"));
    assert!(edit.description.contains("trailing whitespace"));

    let patch = def("apply_patch");
    assert!(
        patch
            .description
            .contains("Apply a multi-file patch with add/update/delete/move")
    );
    assert!(
        patch
            .description
            .contains("existing-file changes, multiple hunks, multiple files, or context anchors")
    );
}

#[tokio::test]
async fn apply_patch_returns_a_diff_for_every_file_and_a_summary() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(workspace.join("gone.txt"), "bye\n").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_apply_patch(&request(
            "apply_patch",
            json!({
                "patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n one\n-two\n+three\n*** Add File: b.txt\n+new\n*** Delete File: gone.txt\n*** End Patch\n"
            }),
        ))
        .await
        .unwrap();
    let structured = result.structured.as_ref().unwrap();
    let diff = structured["diff"].as_str().unwrap();
    for expected in [
        "--- a/a.txt",
        "-two",
        "+three",
        "+++ b/b.txt",
        "+new",
        "--- a/gone.txt",
        "-bye",
    ] {
        assert!(diff.contains(expected), "missing {expected:?} in {diff}");
    }
    let files = structured["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0]["path"], "a.txt");
    assert_eq!(files[0]["status"], "modified");
    assert_eq!(files[0]["additions"], 1);
    assert_eq!(files[0]["deletions"], 1);
    assert_eq!(files[1]["status"], "added");
    assert_eq!(files[2]["status"], "deleted");
    // The files were really changed, and the diff still shows the old text.
    assert!(!workspace.join("gone.txt").exists());
    assert!(
        !std::fs::read_to_string(workspace.join("a.txt"))
            .unwrap()
            .contains("two")
    );
    assert!(!result.content.contains("@@"), "{}", result.content);
}

async fn tool_call_part(executor: &ToolExecutor, name: &str) -> String {
    let message = messages::create(
        &executor.db.pool,
        "test-conversation",
        "assistant",
        "streaming",
    )
    .await
    .unwrap();
    messages::append_part(
        &executor.db.pool,
        &message.id,
        "tool_call",
        &PartPayload::ToolCall {
            call_id: format!("call-{name}"),
            name: name.into(),
            server: "mcp-chat".into(),
            args: json!({}),
            state: "running".into(),
            started_at: Some(1),
            ended_at: None,
            executed: false,
            approval: None,
            error: None,
            result_preview: None,
            full_result_ref: None,
            truncated: false,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn finish_moves_the_diff_onto_the_tool_call_and_out_of_the_result() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    let mut req = request("edit", json!({}));
    req.part_id = tool_call_part(&executor, "edit").await;
    let part_id = req.part_id.clone();
    let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";

    let result = executor
        .finish(
            req,
            Instant::now(),
            Ok(Rendered {
                content: "Edited a.txt: replaced 1 occurrence(s).".into(),
                structured: Some(json!({
                    "path": "a.txt",
                    "replacements": 1,
                    "diff": diff
                })),
                ok: true,
            }),
        )
        .await;

    // What persists as the tool_result, and so what the model can see.
    assert_eq!(result.content, "Edited a.txt: replaced 1 occurrence(s).");
    let structured = result.structured.as_ref().unwrap();
    assert_eq!(structured["path"], "a.txt");
    assert!(structured.get("diff").is_none(), "{structured}");

    // The transcript renders the tool_call part, so the diff lives there.
    let stored = messages::get_part(&executor.db.pool, &part_id)
        .await
        .unwrap()
        .payload();
    assert_eq!(stored["state"], "completed");
    assert_eq!(stored["diff"], diff);

    // The live event carries it so the UI does not wait for a reload.
    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Tool { state, diff, .. } = event {
            if state == "completed" {
                seen = diff;
            }
        }
    }
    assert_eq!(seen.as_deref(), Some(diff));
}

#[tokio::test]
async fn finish_sends_the_shell_exit_code_on_the_live_event() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    let mut req = request("bash", json!({"command": "false"}));
    req.part_id = tool_call_part(&executor, "bash").await;
    let part_id = req.part_id.clone();

    executor
        .finish(
            req,
            Instant::now(),
            Ok(Rendered {
                content: "[exit code 3]".into(),
                structured: Some(json!({"exitCode": 3, "success": false})),
                ok: true,
            }),
        )
        .await;

    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Tool {
            state, exit_code, ..
        } = event
        {
            if state == "completed" {
                seen = exit_code;
            }
        }
    }
    assert_eq!(
        seen,
        Some(3),
        "the row shows the exit code without a reload"
    );
    let stored = messages::get_part(&executor.db.pool, &part_id)
        .await
        .unwrap()
        .payload();
    assert_eq!(stored["exitCode"], 3, "and it is stored too");

    // A tool without an exit code sends none.
    let mut read = request("read", json!({"filePath": "x"}));
    read.part_id = tool_call_part(&executor, "read").await;
    executor
        .finish(
            read,
            Instant::now(),
            Ok(Rendered {
                content: "x".into(),
                structured: None,
                ok: true,
            }),
        )
        .await;
    let mut none_seen = false;
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Tool {
            state, exit_code, ..
        } = event
        {
            if state == "completed" {
                none_seen = exit_code.is_none();
            }
        }
    }
    assert!(none_seen);
}

#[tokio::test]
async fn finish_leaves_an_external_servers_diff_field_alone() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let mut req = request("edit", json!({}));
    req.server_slug = "other".into();
    req.part_id = tool_call_part(&executor, "edit").await;
    let part_id = req.part_id.clone();

    let result = executor
        .finish(
            req,
            Instant::now(),
            Ok(Rendered {
                content: "ok".into(),
                structured: Some(json!({"diff": "server data"})),
                ok: true,
            }),
        )
        .await;

    assert_eq!(result.structured.as_ref().unwrap()["diff"], "server data");
    let stored = messages::get_part(&executor.db.pool, &part_id)
        .await
        .unwrap()
        .payload();
    assert!(stored.get("diff").is_none());
}

#[test]
fn take_ui_diff_ignores_missing_empty_and_non_string_values() {
    let mut none = None;
    assert_eq!(take_ui_diff(&mut none), None);
    let mut empty = Some(json!({"diff": ""}));
    assert_eq!(take_ui_diff(&mut empty), None);
    assert!(empty.unwrap().get("diff").is_none());
    let mut number = Some(json!({"diff": 3}));
    assert_eq!(take_ui_diff(&mut number), None);
    let mut ok = Some(json!({"diff": "x", "path": "p"}));
    assert_eq!(take_ui_diff(&mut ok).as_deref(), Some("x"));
    assert_eq!(ok.unwrap(), json!({"path": "p"}));
}

#[tokio::test]
async fn builtin_shell_runs_in_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let command = if cfg!(windows) {
        "echo shell-ok"
    } else {
        "printf shell-ok"
    };
    let result = executor
        .run_bash(&request("bash", json!({"command": command})))
        .await
        .unwrap();
    assert!(result.content.contains("shell-ok"));
    assert!(result.content.contains("[shell]"));
    assert!(result.content.contains("[cwd]"));
    assert!(result.content.contains("[stdout]"));
    assert!(result.content.contains("[stderr]"));
    assert!(result.content.contains("[exit code 0]"));
}

#[tokio::test]
async fn python_runs_multiline_code_without_shell_quoting() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_python(&request(
            "python",
            json!({
                "code": "state = 'python-ok'\nprint(state)\nprint('unicode: ✓')\n"
            }),
        ))
        .await
        .unwrap();
    assert!(result.ok, "{}", result.content);
    assert!(result.content.contains("python-ok"));
    assert!(result.content.contains("unicode: ✓"));
    assert!(result.content.contains("[exit code 0]"));
}

#[tokio::test]
async fn python_nonzero_exit_is_output_not_a_tool_failure() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_python(&request(
            "python",
            json!({"code": "import sys\nprint('bad')\nsys.exit(1)\n"}),
        ))
        .await
        .unwrap();
    assert!(result.ok, "{}", result.content);
    assert!(result.content.contains("[exit code 1]"));
    assert_eq!(result.structured.as_ref().unwrap()["exitCode"], 1);
}

#[tokio::test]
async fn read_stages_images_as_visual_tool_results() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let image = workspace.join("pixel.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\npixel-bytes").unwrap();
    let executor = test_executor(&workspace).await;
    let result = executor
        .run_read(&request(
            "read",
            json!({"filePath": image.to_string_lossy()}),
        ))
        .await
        .unwrap();
    assert!(result.ok);
    assert!(result.content.contains("Image loaded"));
    let image = result
        .structured
        .as_ref()
        .and_then(|value| value.get("image"))
        .expect("image metadata");
    assert_eq!(image["mime"], "image/png");
    let path = image["path"].as_str().unwrap();
    assert!(std::path::Path::new(path).is_file());
    assert!(path.contains("read-images"));
}

#[tokio::test]
async fn read_image_metadata_is_sent_on_the_live_tool_event() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let file = workspace.join("pixel.png");
    std::fs::write(&file, b"\x89PNG\r\n\x1a\npixel-bytes").unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    let mut req = request("read", json!({"filePath": file.to_string_lossy()}));
    req.part_id = tool_call_part(&executor, "read").await;

    let rendered = executor.run_read(&req).await.unwrap();
    executor.finish(req, Instant::now(), Ok(rendered)).await;

    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Tool { state, image, .. } = event {
            if state == "completed" {
                seen = image;
            }
        }
    }
    let image = seen.expect("the completed event carries the image metadata");
    assert_eq!(image["mime"], "image/png");
    assert_eq!(image["name"], "pixel.png");
}

#[tokio::test]
async fn shell_exit_code_is_output_not_a_tool_failure() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let (shell, command) = if cfg!(windows) {
        ("cmd", "exit /b 1")
    } else {
        ("sh", "exit 1")
    };
    let result = executor
        .run_bash(&request(
            "bash",
            json!({"command": command, "shell": shell}),
        ))
        .await
        .unwrap();
    assert!(result.ok);
    assert!(result.content.contains("[exit code 1]"));
    assert_eq!(result.structured.as_ref().unwrap()["exitCode"], 1);
}

#[tokio::test]
async fn read_tool_output_supports_line_paging() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("call.json"),
        json!({"content": "a\nb\nc\nd"}).to_string(),
    )
    .unwrap();
    let page = read_spilled_lines(dir.path(), "call", 2, 2).await.unwrap();
    let page: Value = serde_json::from_str(&page).unwrap();
    assert_eq!(page["line"], 2);
    assert_eq!(page["content"], "b\nc");
    assert_eq!(page["next_line"], 4);
    assert_eq!(page["total_lines"], 4);
    assert_eq!(page["truncated"], true);

    let last = read_spilled_lines(dir.path(), "call", 4, 2).await.unwrap();
    let last: Value = serde_json::from_str(&last).unwrap();
    assert_eq!(last["content"], "d");
    assert_eq!(last["truncated"], false);
}

#[tokio::test]
async fn search_tools_honor_jarvisignore_and_skip_binary_files() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::create_dir_all(workspace.join("hidden")).unwrap();
    std::fs::create_dir_all(workspace.join("visible")).unwrap();
    std::fs::write(
        workspace.join(".jarvisignore"),
        "hidden/\n*.secret\nqa_neg_*.txt\n!qa_neg_keep.txt\n",
    )
    .unwrap();
    std::fs::write(workspace.join("hidden").join("a.txt"), "needle hidden\n").unwrap();
    std::fs::write(workspace.join("visible").join("b.txt"), "needle visible\n").unwrap();
    std::fs::write(workspace.join("root.secret"), "needle secret\n").unwrap();
    std::fs::write(workspace.join("root.bin"), [0u8, 1, 2, 3]).unwrap();
    std::fs::write(workspace.join("qa_neg_drop.txt"), "needle drop\n").unwrap();
    std::fs::write(workspace.join("qa_neg_keep.txt"), "needle keep\n").unwrap();
    let executor = test_executor(&workspace).await;

    let result = executor
        .run_glob(&request("glob", json!({"pattern": "**/*.txt"})))
        .await
        .unwrap();
    assert!(result.content.contains("visible/b.txt"));
    assert!(!result.content.contains("hidden/a.txt"));
    assert!(result.content.contains("qa_neg_keep.txt"));
    assert!(!result.content.contains("qa_neg_drop.txt"));

    let result = executor
        .run_grep(&request("grep", json!({"pattern": "needle"})))
        .await
        .unwrap();
    assert!(result.content.contains("visible/b.txt"));
    assert!(!result.content.contains("hidden/a.txt"));
    assert!(!result.content.contains("root.bin"));
    assert!(result.content.contains("qa_neg_keep.txt"));
    assert!(!result.content.contains("qa_neg_drop.txt"));

    let result = executor
        .run_glob(&request(
            "glob",
            json!({"pattern": "*.txt", "recursive": false}),
        ))
        .await
        .unwrap();
    assert!(!result.content.contains("visible/b.txt"));
}

#[tokio::test]
async fn builtin_search_tools_find_workspace_files() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    executor
        .run_write(&request(
            "write",
            json!({"filePath": "src/main.txt", "content": "needle here"}),
        ))
        .await
        .unwrap();
    executor
        .run_write(&request(
            "write",
            json!({"filePath": "src/other.txt", "content": "nothing"}),
        ))
        .await
        .unwrap();

    let glob = executor
        .run_glob(&request("glob", json!({"pattern": "**/*.txt"})))
        .await
        .unwrap();
    assert!(glob.content.contains("src/main.txt"));
    assert!(glob.content.contains("src/other.txt"));

    let grep = executor
        .run_grep(&request("grep", json!({"pattern": "needle"})))
        .await
        .unwrap();
    assert!(grep.content.contains("src/main.txt:1:"));
    assert!(grep.content.contains("needle here"));
}

#[tokio::test]
async fn builtin_skill_loads_skill_md() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let skill_dir = workspace.join(".mcp-chat/skills/demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "Use the demo workflow.").unwrap();
    let executor = test_executor(&workspace).await;

    let listed = executor
        .run_skill(&request("skill", json!({})))
        .await
        .unwrap();
    assert!(listed.content.contains("demo"));

    let loaded = executor
        .run_skill(&request("skill", json!({"name": "demo"})))
        .await
        .unwrap();
    assert!(loaded.content.contains("Use the demo workflow."));

    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(skill_dir.join("references/api.md"), "reference content").unwrap();
    let reference = executor
        .run_skill(&request(
            "skill",
            json!({"name": "demo", "file": "references/api.md"}),
        ))
        .await
        .unwrap();
    assert!(reference.content.contains("reference content"));

    let listing = executor
        .run_skill(&request(
            "skill",
            json!({"name": "demo", "file": "references"}),
        ))
        .await
        .unwrap();
    assert!(listing.content.contains("api.md"));

    let escaped = executor
        .run_skill(&request(
            "skill",
            json!({"name": "demo", "file": "../outside.md"}),
        ))
        .await;
    assert!(escaped.is_err());
}

#[tokio::test]
async fn disabled_skill_is_hidden_and_rejected() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let skill_dir = workspace.join(".mcp-chat/skills/demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "Use the demo workflow.").unwrap();
    let executor = test_executor(&workspace).await;

    set_skill_enabled(&executor.db.pool, "demo", false)
        .await
        .unwrap();

    let listed = executor
        .run_skill(&request("skill", json!({})))
        .await
        .unwrap();
    assert!(!listed.content.contains("demo"));

    let disabled = executor
        .run_skill(&request("skill", json!({"name": "demo"})))
        .await;
    let disabled = match disabled {
        Ok(_) => panic!("disabled skill unexpectedly loaded"),
        Err(error) => error,
    };
    assert!(disabled.user_message().contains("disabled"));

    set_skill_enabled(&executor.db.pool, "demo", true)
        .await
        .unwrap();
    let loaded = executor
        .run_skill(&request("skill", json!({"name": "demo"})))
        .await
        .unwrap();
    assert!(loaded.content.contains("Use the demo workflow."));
}

#[tokio::test]
async fn outside_writes_require_explicit_approval_even_in_auto_mode() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside.txt");
    let executor = Arc::new(test_executor(&workspace).await);
    let approvals = executor.approvals.clone();
    let mut events = executor.events.subscribe();

    let mut req = request(
        "write",
        json!({"filePath": target.to_string_lossy(), "content": "approved"}),
    );
    req.auto_approve = true;
    let handle = tokio::spawn(async move { executor.execute(req).await });

    let call_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval event timed out")
            .expect("event channel");
        if let CoreEvent::Approval {
            call_id,
            allow_always,
            ..
        } = event
        {
            assert!(!allow_always);
            break call_id;
        }
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(approvals.respond(
        &call_id,
        crate::agent::approvals::ApprovalAnswer {
            decision: ApprovalDecision::Once,
            feedback: None,
        }
    ));
    let result = handle.await.unwrap();
    assert!(result.ok, "{}", result.content);
    assert_eq!(std::fs::read_to_string(target).unwrap(), "approved");
}

#[test]
fn global_skill_roots_are_discovered() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let output = workspace.path().join("tool-output");
    let roots = skill_roots_with_home(workspace.path(), &output, Some(home.path()));
    assert!(roots.contains(&home.path().join(".config/opencode/skills")));
    assert!(roots.contains(&home.path().join(".config/mcp-chat/skills")));
    assert!(roots.contains(&workspace.path().join(".mcp-chat/skills")));
}

#[tokio::test]
async fn outside_reads_require_explicit_approval_when_auto_is_off() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    let outside_path = outside_file.to_string_lossy().to_string();
    std::fs::write(&outside_file, "outside content").unwrap();
    let executor = Arc::new(test_executor(&workspace).await);
    let approvals = executor.approvals.clone();
    let mut events = executor.events.subscribe();

    let req = request("read", json!({"filePath": outside_path}));
    let handle = tokio::spawn(async move { executor.execute(req).await });

    let (call_id, args) = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval event timed out")
            .expect("event channel");
        if let CoreEvent::Approval {
            call_id,
            args,
            allow_always,
            ..
        } = event
        {
            assert!(!allow_always);
            break (call_id, args);
        }
    };
    assert_eq!(args["filePath"].as_str(), Some(outside_path.as_str()));

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(approvals.respond(
        &call_id,
        crate::agent::approvals::ApprovalAnswer {
            decision: ApprovalDecision::Once,
            feedback: None,
        }
    ));
    let result = handle.await.unwrap();
    assert!(result.ok, "{}", result.content);
    assert!(result.content.contains("outside content"));
}

#[tokio::test]
async fn read_only_tools_can_leave_workspace_in_auto_mode() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, "outside content").unwrap();
    let executor = test_executor(&workspace).await;

    let mut allowed = request("read", json!({"filePath": outside_file.to_string_lossy()}));
    allowed.auto_approve = true;
    let result = executor.run_read(&allowed).await.unwrap();
    assert!(result.content.contains("outside content"));

    let mut grep = request(
        "grep",
        json!({"pattern": "outside", "path": outside.path().to_string_lossy()}),
    );
    grep.auto_approve = true;
    let result = executor.run_grep(&grep).await.unwrap();
    assert!(result.content.contains("outside content"));

    let mut glob = request(
        "glob",
        json!({"pattern": "**/*.txt", "path": outside.path().to_string_lossy()}),
    );
    glob.auto_approve = true;
    let result = executor.run_glob(&glob).await.unwrap();
    assert!(result.content.contains("outside.txt"));
}

// ---- capability table: drift guards -----------------------------------------

#[tokio::test]
async fn every_tool_in_the_table_has_a_handler_and_a_schema() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let table = crate::agent::capabilities::table();

    let defs = builtin_tool_defs();
    assert_eq!(
        defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        table
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        "definitions follow the table, in order"
    );
    for capability in &table.tools {
        assert!(is_builtin_tool(&capability.name), "{}", capability.name);
        // No arguments: every handler must reject this as invalid input. Only a
        // missing handler answers "Unknown built-in tool".
        let outcome = executor
            .run_builtin(&request(&capability.name, json!({})))
            .await;
        if let Err(error) = outcome {
            assert!(
                !error.user_message().contains("Unknown built-in tool"),
                "{} is in the table but not handled",
                capability.name
            );
        }
    }
    assert!(!is_builtin_tool("not_a_tool"));
    assert_eq!(builtin_default_effect("not_a_tool"), Effect::Allow);
}

#[test]
fn guard_flags_are_in_the_schema_and_only_there() {
    let table = crate::agent::capabilities::table();
    for capability in &table.tools {
        let def = builtin_tool_def(&capability.name).unwrap();
        let properties = def.input_schema["properties"].as_object().unwrap();
        match &capability.guard {
            Some(guard) => {
                assert_eq!(properties[&guard.override_param]["type"], "boolean");
                assert_eq!(
                    properties[&guard.override_param]["description"],
                    guard.override_description.as_str()
                );
            }
            None => {
                for other in table.tools.iter().filter_map(|t| t.guard.as_ref()) {
                    assert!(
                        !properties.contains_key(&other.override_param),
                        "{} must not carry another tool's override flag",
                        capability.name
                    );
                }
            }
        }
    }
}

#[test]
fn outside_read_targets_follow_the_table() {
    assert_eq!(
        outside_read_target("read", &json!({"filePath": "/etc/x"})),
        Some("/etc/x")
    );
    assert_eq!(
        outside_read_target("grep", &json!({"path": "/etc"})),
        Some("/etc")
    );
    assert_eq!(
        outside_read_target("glob", &json!({"path": "/etc"})),
        Some("/etc")
    );
    // Write tools are checked by the write path, not the read path.
    assert_eq!(
        outside_read_target("write", &json!({"filePath": "/etc/x"})),
        None
    );
    assert_eq!(outside_read_target("bash", &json!({"command": "ls"})), None);
}

#[tokio::test]
async fn writes_outside_the_workspace_are_detected_from_the_table() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let outside = if cfg!(windows) {
        "C:\\Windows\\x.txt"
    } else {
        "/tmp/x.txt"
    };

    for tool in ["write", "edit"] {
        let inside = request(tool, json!({"filePath": "a.txt"}));
        assert!(!executor.writes_outside_workspace(&inside).await, "{tool}");
        let outside_call = request(tool, json!({"filePath": outside}));
        assert!(
            executor.writes_outside_workspace(&outside_call).await,
            "{tool}"
        );
    }
    let patch_inside = request(
        "apply_patch",
        json!({"patchText": "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch\n"}),
    );
    assert!(!executor.writes_outside_workspace(&patch_inside).await);
    let patch_outside = request(
        "apply_patch",
        json!({"patchText": format!("*** Begin Patch\n*** Add File: {outside}\n+x\n*** End Patch\n")}),
    );
    assert!(executor.writes_outside_workspace(&patch_outside).await);
    // Read tools and shell are not write-access tools.
    assert!(
        !executor
            .writes_outside_workspace(&request("read", json!({"filePath": outside})))
            .await
    );
    assert!(
        !executor
            .writes_outside_workspace(&request("bash", json!({"command": "ls"})))
            .await
    );
}

#[test]
fn dangerous_calls_follow_the_table() {
    let mut rm = request("bash", json!({"command": "rm -rf build"}));
    rm.auto_approve = true;
    assert_eq!(auto_effective_effect(&rm, Effect::Ask), Effect::Ask);

    let mut delete = request(
        "apply_patch",
        json!({"patchText": "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch\n"}),
    );
    delete.auto_approve = true;
    assert_eq!(auto_effective_effect(&delete, Effect::Ask), Effect::Ask);

    let mut add = request(
        "apply_patch",
        json!({"patchText": "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch\n"}),
    );
    add.auto_approve = true;
    assert_eq!(auto_effective_effect(&add, Effect::Ask), Effect::Allow);

    // `overwrite` is not an escape that needs review; `allowFileWrites` is.
    let mut overwrite = request(
        "write",
        json!({"filePath": "a.txt", "content": "x", "overwrite": true}),
    );
    overwrite.auto_approve = true;
    assert_eq!(
        auto_effective_effect(&overwrite, Effect::Ask),
        Effect::Allow
    );
    let mut escape = request(
        "bash",
        json!({"command": "echo hi > out.txt", "allowFileWrites": true}),
    );
    escape.auto_approve = true;
    assert_eq!(auto_effective_effect(&escape, Effect::Ask), Effect::Ask);
}

#[tokio::test]
async fn only_diff_capable_tools_get_an_approval_diff() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    std::fs::write(workspace.join("a.txt"), "old\n").unwrap();
    let executor = test_executor(&workspace).await;
    let edit_args = json!({"filePath": "a.txt", "oldString": "old", "newString": "new"});
    assert!(
        executor
            .approval_diff("test-conversation", "", "edit", &edit_args)
            .await
            .is_some()
    );
    // A tool that is not in the diff set never gets one, whatever its arguments.
    assert!(
        executor
            .approval_diff("test-conversation", "", "bash", &edit_args)
            .await
            .is_none()
    );
}

// ---- the stored approval value never stays "pending" ----------------------

/// Run a `bash` call that asks for approval, answer it with `answer` (or
/// cancel it when `None`), and return the `approval` value saved on its part.
async fn stored_approval_after(answer: Option<ApprovalDecision>) -> Option<String> {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = Arc::new(test_executor(&workspace).await);
    let mut events = executor.events.subscribe();
    let mut req = request("bash", json!({"command": "echo hi"}));
    req.part_id = tool_call_part(&executor, "bash").await;
    let part_id = req.part_id.clone();
    let cancel = req.cancel.clone();
    let runner = executor.clone();
    let handle = tokio::spawn(async move { runner.execute(req).await });

    let call_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval event")
            .expect("event channel");
        if let CoreEvent::Approval { call_id, .. } = event {
            break call_id;
        }
    };
    // While the prompt is open the part says so.
    let pending = messages::get_part(&executor.db.pool, &part_id)
        .await
        .unwrap()
        .payload();
    assert_eq!(pending["approval"], "pending");

    tokio::time::sleep(Duration::from_millis(20)).await;
    match answer {
        Some(decision) => {
            assert!(executor.approvals.respond(
                &call_id,
                crate::agent::approvals::ApprovalAnswer {
                    decision,
                    feedback: None,
                }
            ));
        }
        None => cancel.cancel(),
    }
    handle.await.unwrap();
    messages::get_part(&executor.db.pool, &part_id)
        .await
        .unwrap()
        .payload()
        .get("approval")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[tokio::test]
async fn an_approved_call_is_stored_as_allowed() {
    assert_eq!(
        stored_approval_after(Some(ApprovalDecision::Once))
            .await
            .as_deref(),
        Some("allowed")
    );
    assert_eq!(
        stored_approval_after(Some(ApprovalDecision::Always))
            .await
            .as_deref(),
        Some("allowed")
    );
}

#[tokio::test]
async fn always_allow_saves_the_previewed_shell_pattern() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = Arc::new(test_executor(&workspace).await);
    let mut events = executor.events.subscribe();
    let mut req = request("bash", json!({"command": "echo hi"}));
    req.part_id = tool_call_part(&executor, "bash").await;
    let runner = executor.clone();
    let handle = tokio::spawn(async move { runner.execute(req).await });

    let call_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("approval event")
            .expect("event channel");
        if let CoreEvent::Approval {
            call_id,
            action,
            resource,
            save_pattern,
            ..
        } = event
        {
            assert_eq!(action, "builtin.bash");
            assert_eq!(resource, "echo hi");
            assert_eq!(save_pattern.as_deref(), Some("echo *"));
            break call_id;
        }
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(executor.approvals.respond(
        &call_id,
        crate::agent::approvals::ApprovalAnswer {
            decision: ApprovalDecision::Always,
            feedback: None,
        }
    ));
    handle.await.unwrap();

    let rules = rules_repo::list(&executor.db.pool).await.unwrap();
    let saved = rules
        .iter()
        .find(|rule| rule.action == "builtin.bash")
        .expect("saved bash rule");
    assert_eq!(saved.resource, "echo *");
}

#[tokio::test]
async fn a_rejected_call_is_stored_as_rejected_not_pending() {
    assert_eq!(
        stored_approval_after(Some(ApprovalDecision::Reject))
            .await
            .as_deref(),
        Some("rejected")
    );
}

#[tokio::test]
async fn a_cancelled_approval_is_stored_as_rejected_not_pending() {
    assert_eq!(
        stored_approval_after(None).await.as_deref(),
        Some("rejected")
    );
}

// ---- the question tool ----------------------------------------------------

fn question_schema() -> Value {
    builtin_tool_def("question").unwrap().input_schema
}

fn accepts(questions: Value) -> bool {
    validate_args(&question_schema(), &json!({ "questions": questions })).is_ok()
}

#[test]
fn a_question_without_options_is_valid_and_means_a_free_form_answer() {
    assert!(accepts(json!([
        {"header": "Database", "question": "Which database should I target?"}
    ])));
    // Only the header and the question text are required.
    let schema = question_schema();
    assert_eq!(
        schema["properties"]["questions"]["items"]["required"],
        json!(["header", "question"])
    );
    // Free-form and multiple-choice questions can share one call.
    assert!(accepts(json!([
        {"header": "Database", "question": "Which database should I target?"},
        {
            "header": "Format",
            "question": "Which output format do you want?",
            "multiple": true,
            "options": [{"label": "Markdown"}, {"label": "JSON", "description": "Machine readable"}]
        }
    ])));
}

#[test]
fn options_when_sent_need_one_to_twelve_labelled_items() {
    let with_options =
        |options: Value| accepts(json!([{"header": "h", "question": "q", "options": options}]));
    assert!(with_options(json!([{"label": "A"}])));
    let twelve: Vec<Value> = (1..=12)
        .map(|n| json!({"label": format!("Option {n}")}))
        .collect();
    assert!(with_options(Value::Array(twelve)));

    // An empty list is rejected: leave `options` out for a free-form answer.
    assert!(!with_options(json!([])));
    let thirteen: Vec<Value> = (1..=13)
        .map(|n| json!({"label": format!("Option {n}")}))
        .collect();
    assert!(!with_options(Value::Array(thirteen)));
    assert!(!with_options(json!([{"description": "no label"}])));
    assert!(!with_options(json!([{"label": "A", "extra": true}])));
    assert!(!with_options(json!("Blue")));
}

#[test]
fn a_question_still_needs_a_header_and_its_text() {
    assert!(!accepts(
        json!([{"question": "Which database should I target?"}])
    ));
    assert!(!accepts(json!([{"header": "Database"}])));
    assert!(!accepts(json!([{}])));
    assert!(!accepts(json!([])));
    assert!(!accepts(json!([
        {"header": "Database", "question": "Which one?", "surprise": 1}
    ])));
    assert!(validate_args(&question_schema(), &json!({})).is_err());
}

#[test]
fn a_call_holds_at_most_eight_questions() {
    let many = |count: usize| {
        Value::Array(
            (1..=count)
                .map(|n| json!({"header": format!("Q{n}"), "question": format!("Question {n}?")}))
                .collect(),
        )
    };
    assert!(accepts(many(8)));
    assert!(!accepts(many(9)));
}

/// Run a `question` call through the executor, answer it with `responses`, and
/// return the request the UI received together with the tool result.
async fn ask(args: Value, responses: Value, cancelled: bool) -> (Value, ToolExecResult) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = Arc::new(test_executor(&workspace).await);
    let mut events = executor.events.subscribe();
    let mut req = request("question", args);
    req.part_id = tool_call_part(&executor, "question").await;
    let runner = executor.clone();
    let handle = tokio::spawn(async move { runner.execute(req).await });
    let (call_id, asked) = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("input request event")
            .expect("event channel");
        match event {
            CoreEvent::InputRequest {
                call_id, request, ..
            } => break (call_id, request),
            CoreEvent::Approval { .. } => panic!("a question must not ask for approval"),
            _ => {}
        }
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(executor.inputs.respond(
        &call_id,
        InputAnswer {
            responses,
            cancelled
        }
    ));
    (asked, handle.await.unwrap())
}

#[tokio::test]
async fn a_free_form_question_is_sent_without_options_and_a_typed_answer_comes_back() {
    let questions = json!([{"header": "Database", "question": "Which database should I target?"}]);
    let (asked, result) = ask(
        json!({ "questions": questions }),
        json!({"answers": [{
            "header": "Database",
            "question": "Which database should I target?",
            "answer": "Postgres 16"
        }]}),
        false,
    )
    .await;
    // The UI gets the question exactly as the model wrote it: no options key.
    assert_eq!(asked, json!({ "questions": questions }));
    assert!(asked["questions"][0].get("options").is_none());
    assert!(result.ok, "{}", result.content);
    assert!(result.executed);
    assert_eq!(
        result.content,
        "The user answered:\n- Database: Postgres 16\n  (Which database should I target?)\n"
    );
}

#[tokio::test]
async fn an_answer_that_is_not_a_string_is_still_shown_to_the_model() {
    let (_, result) = ask(
        json!({"questions": [{
            "header": "Format",
            "question": "Which output format do you want?",
            "multiple": true,
            "options": [{"label": "Markdown"}, {"label": "JSON"}]
        }]}),
        json!({"answers": [{
            "header": "Format",
            "question": "Which output format do you want?",
            "answer": ["Markdown", "JSON"]
        }]}),
        false,
    )
    .await;
    assert!(result.ok, "{}", result.content);
    assert_eq!(
        result.content,
        "The user answered:\n- Format: [\"Markdown\",\"JSON\"]\n  (Which output format do you want?)\n"
    );
}

#[tokio::test]
async fn declining_a_question_is_a_tool_error_not_an_answer() {
    let (_, result) = ask(
        json!({"questions": [{"header": "Database", "question": "Which database?"}]}),
        json!({}),
        true,
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.content, "The user cancelled the question.");
}

#[tokio::test]
async fn an_empty_options_list_is_refused_before_the_user_is_asked() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let executor = test_executor(&workspace).await;
    let mut events = executor.events.subscribe();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        executor.execute(request(
            "question",
            json!({"questions": [{"header": "h", "question": "q", "options": []}]}),
        )),
    )
    .await
    .expect("an invalid question should fail without waiting for an answer");
    assert!(!result.ok);
    assert!(!result.executed);
    assert!(
        result.content.starts_with("Invalid tool input:"),
        "{}",
        result.content
    );
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, CoreEvent::InputRequest { .. }));
    }
}
