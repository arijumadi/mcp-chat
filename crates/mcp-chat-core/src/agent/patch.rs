use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::fs;

use crate::agent::diff;
use crate::agent::tools::{display_relative, path_is_outside, resolve_write_path};
use crate::{CoreError, Result};

const MAX_PATCH_BYTES: usize = 1_000_000;
const MAX_PATCH_FILES: usize = 20;
const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_FILE_CHARS: usize = 2_000_000;

#[derive(Debug)]
enum Operation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug)]
struct Hunk {
    old: Vec<String>,
    new: Vec<String>,
    additions: usize,
    deletions: usize,
    context: Option<String>,
    end_of_file: bool,
}

#[derive(Debug)]
struct Change {
    path: PathBuf,
    old: Option<String>,
    new: Option<String>,
}

pub struct PatchResult {
    lines: Vec<String>,
}

pub struct PatchPlan {
    changes: Vec<Change>,
    results: Vec<String>,
}

impl PatchPlan {
    pub fn target_paths(&self) -> Vec<PathBuf> {
        self.changes
            .iter()
            .map(|change| change.path.clone())
            .collect()
    }

    pub async fn commit(&self) -> Result<()> {
        commit(&self.changes).await
    }

    pub fn result(&self) -> PatchResult {
        PatchResult {
            lines: self.results.clone(),
        }
    }

    pub fn diff(&self, workspace: &Path) -> String {
        let mut out = String::new();
        for change in &self.changes {
            let before = change.old.as_deref().unwrap_or("");
            let after = change.new.as_deref().unwrap_or("");
            if before == after {
                continue;
            }
            let part = diff::file_diff(&relative(workspace, &change.path), before, after);
            if part.is_empty() {
                continue;
            }
            out.push_str(&part);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        diff::cap(out)
    }
}

impl PatchResult {
    pub fn render(&self) -> String {
        let mut out = format!("Applied patch ({} file(s)).", self.lines.len());
        for line in &self.lines {
            out.push('\n');
            out.push_str(line);
        }
        out
    }
}

pub async fn apply(workspace: &Path, patch_text: &str) -> Result<PatchResult> {
    apply_with_scope(workspace, patch_text, false).await
}

pub fn touches_outside(workspace: &Path, patch_text: &str) -> bool {
    let Ok(operations) = parse(patch_text) else {
        return false;
    };
    operations.iter().any(|operation| match operation {
        Operation::Add { path, .. } | Operation::Delete { path } => {
            path_is_outside(workspace, path)
        }
        Operation::Update { path, move_to, .. } => {
            path_is_outside(workspace, path)
                || move_to
                    .as_deref()
                    .map(|value| path_is_outside(workspace, value))
                    .unwrap_or(false)
        }
    })
}

pub async fn apply_with_scope(
    workspace: &Path,
    patch_text: &str,
    allow_outside: bool,
) -> Result<PatchResult> {
    let plan = prepare_with_scope(workspace, patch_text, allow_outside).await?;
    plan.commit().await?;
    Ok(plan.result())
}

pub async fn preview_with_scope(
    workspace: &Path,
    patch_text: &str,
    allow_outside: bool,
) -> Result<String> {
    let plan = prepare_with_scope(workspace, patch_text, allow_outside).await?;
    Ok(plan.diff(workspace))
}

/// Resolve and deduplicate every path a patch will touch, without reading or
/// writing files. Callers use this to hold file locks across prepare + commit.
pub fn target_paths(
    workspace: &Path,
    patch_text: &str,
    allow_outside: bool,
) -> Result<Vec<PathBuf>> {
    if patch_text.len() > MAX_PATCH_BYTES {
        return Err(CoreError::invalid(format!(
            "Patch is too large ({} bytes); limit is {} bytes.",
            patch_text.len(),
            MAX_PATCH_BYTES
        )));
    }
    let operations = parse(patch_text)?;
    if operations.len() > MAX_PATCH_FILES {
        return Err(CoreError::invalid(format!(
            "Patch touches too many files ({}); limit is {}.",
            operations.len(),
            MAX_PATCH_FILES
        )));
    }
    let mut paths = Vec::new();
    for operation in operations {
        match operation {
            Operation::Add { path, .. } | Operation::Delete { path } => {
                paths.push(resolve_write_path(workspace, &path, allow_outside)?);
            }
            Operation::Update { path, move_to, .. } => {
                paths.push(resolve_write_path(workspace, &path, allow_outside)?);
                if let Some(destination) = move_to {
                    paths.push(resolve_write_path(workspace, &destination, allow_outside)?);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn prepare_with_scope(
    workspace: &Path,
    patch_text: &str,
    allow_outside: bool,
) -> Result<PatchPlan> {
    if patch_text.len() > MAX_PATCH_BYTES {
        return Err(CoreError::invalid(format!(
            "Patch is too large ({} bytes); limit is {} bytes.",
            patch_text.len(),
            MAX_PATCH_BYTES
        )));
    }
    let operations = parse(patch_text)?;
    if operations.len() > MAX_PATCH_FILES {
        return Err(CoreError::invalid(format!(
            "Patch touches too many files ({}); limit is {}.",
            operations.len(),
            MAX_PATCH_FILES
        )));
    }

    let mut changes = Vec::new();
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for operation in operations {
        match operation {
            Operation::Add { path, lines } => {
                let resolved = resolve_write_path(workspace, &path, allow_outside)?;
                ensure_new_path(&resolved, &mut seen).await?;
                if fs::metadata(&resolved).await.is_ok() {
                    return Err(CoreError::invalid(format!(
                        "File already exists: {}",
                        relative(workspace, &resolved)
                    )));
                }
                let content = if lines.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", lines.join("\n"))
                };
                let additions = lines.len();
                results.push(format!(
                    "A {} (+{additions})",
                    relative(workspace, &resolved)
                ));
                changes.push(Change {
                    path: resolved,
                    old: None,
                    new: Some(content),
                });
            }
            Operation::Delete { path } => {
                let resolved = resolve_write_path(workspace, &path, allow_outside)?;
                ensure_new_path(&resolved, &mut seen).await?;
                let old = read_text(&resolved).await?;
                let deletions = count_lines(&old);
                results.push(format!(
                    "D {} (-{deletions})",
                    relative(workspace, &resolved)
                ));
                changes.push(Change {
                    path: resolved,
                    old: Some(old),
                    new: None,
                });
            }
            Operation::Update {
                path,
                move_to,
                hunks,
            } => {
                let resolved = resolve_write_path(workspace, &path, allow_outside)?;
                ensure_new_path(&resolved, &mut seen).await?;
                let old = read_text(&resolved).await?;
                let (new, additions, deletions) = apply_hunks(&old, &hunks)?;
                let old_relative = relative(workspace, &resolved);
                if let Some(move_to) = move_to {
                    let destination = resolve_write_path(workspace, &move_to, allow_outside)?;
                    ensure_new_path(&destination, &mut seen).await?;
                    if fs::metadata(&destination).await.is_ok() {
                        return Err(CoreError::invalid(format!(
                            "Move destination already exists: {}",
                            relative(workspace, &destination)
                        )));
                    }
                    let new_relative = relative(workspace, &destination);
                    results.push(format!(
                        "R {old_relative} -> {new_relative} (+{additions}/-{deletions})"
                    ));
                    changes.push(Change {
                        path: resolved,
                        old: Some(old),
                        new: None,
                    });
                    changes.push(Change {
                        path: destination,
                        old: None,
                        new: Some(new),
                    });
                } else {
                    results.push(format!("M {old_relative} (+{additions}/-{deletions})"));
                    changes.push(Change {
                        path: resolved,
                        old: Some(old),
                        new: Some(new),
                    });
                }
            }
        }
    }

    Ok(PatchPlan { changes, results })
}

async fn ensure_new_path(path: &Path, seen: &mut HashSet<String>) -> Result<()> {
    let key = path.to_string_lossy().to_lowercase();
    if !seen.insert(key) {
        return Err(CoreError::invalid(format!(
            "Patch touches the same path more than once: {}",
            path.display()
        )));
    }
    Ok(())
}

fn parse(patch_text: &str) -> Result<Vec<Operation>> {
    let normalized = patch_text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let begin = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .ok_or_else(|| CoreError::invalid("Patch must start with *** Begin Patch."))?;
    let end = lines
        .iter()
        .rposition(|line| line.trim() == "*** End Patch")
        .ok_or_else(|| CoreError::invalid("Patch must end with *** End Patch."))?;
    if end <= begin {
        return Err(CoreError::invalid("Patch has no operations."));
    }

    let mut operations = Vec::new();
    let mut index = begin + 1;
    while index < end {
        let line = lines[index].trim_end();
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = path.trim().to_owned();
            if path.is_empty() {
                return Err(CoreError::invalid("Add File requires a path."));
            }
            index += 1;
            let mut content = Vec::new();
            while index < end {
                let line = lines[index];
                if line.starts_with("*** ") {
                    break;
                }
                if let Some(added) = line.strip_prefix('+') {
                    content.push(added.to_owned());
                } else if line.is_empty() {
                    content.push(String::new());
                } else {
                    return Err(CoreError::invalid(
                        "Add File content lines must start with '+'.",
                    ));
                }
                index += 1;
            }
            operations.push(Operation::Add {
                path,
                lines: content,
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = path.trim().to_owned();
            if path.is_empty() {
                return Err(CoreError::invalid("Update File requires a path."));
            }
            index += 1;
            let mut move_to = None;
            if index < end {
                if let Some(destination) = lines[index].strip_prefix("*** Move to: ") {
                    let destination = destination.trim().to_owned();
                    if destination.is_empty() {
                        return Err(CoreError::invalid("Move to requires a path."));
                    }
                    move_to = Some(destination);
                    index += 1;
                }
            }
            let mut hunks = Vec::new();
            let mut current: Option<Hunk> = None;
            while index < end {
                let line = lines[index];
                if let Some(destination) = line.strip_prefix("*** Move to: ") {
                    let destination = destination.trim().to_owned();
                    if destination.is_empty() {
                        return Err(CoreError::invalid("Move to requires a path."));
                    }
                    move_to = Some(destination);
                    index += 1;
                    continue;
                }
                if line.trim() == "*** End of File" {
                    if let Some(hunk) = current.as_mut() {
                        hunk.end_of_file = true;
                    }
                    index += 1;
                    continue;
                }
                if line.starts_with("*** ") {
                    break;
                }
                if line.starts_with("@@") {
                    if let Some(hunk) = current.take() {
                        hunks.push(hunk);
                    }
                    let context = line
                        .trim()
                        .strip_prefix("@@")
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                    current = Some(Hunk {
                        old: Vec::new(),
                        new: Vec::new(),
                        additions: 0,
                        deletions: 0,
                        context,
                        end_of_file: false,
                    });
                    index += 1;
                    continue;
                }
                if line.starts_with('\\') {
                    index += 1;
                    continue;
                }
                if line.is_empty() {
                    if let Some(hunk) = current.as_mut() {
                        hunk.old.push(String::new());
                        hunk.new.push(String::new());
                    }
                    index += 1;
                    continue;
                }
                let (prefix, content) = line.split_at(1);
                let hunk = current.get_or_insert_with(|| Hunk {
                    old: Vec::new(),
                    new: Vec::new(),
                    additions: 0,
                    deletions: 0,
                    context: None,
                    end_of_file: false,
                });
                match prefix {
                    " " => {
                        hunk.old.push(content.to_owned());
                        hunk.new.push(content.to_owned());
                    }
                    "-" => {
                        hunk.old.push(content.to_owned());
                        hunk.deletions += 1;
                    }
                    "+" => {
                        hunk.new.push(content.to_owned());
                        hunk.additions += 1;
                    }
                    _ => {
                        return Err(CoreError::invalid(
                            "Update hunk lines must start with space, '-' or '+'.",
                        ));
                    }
                }
                index += 1;
            }
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            if move_to.is_none() && hunks.is_empty() {
                return Err(CoreError::invalid(
                    "Update File needs at least one hunk or a Move to target.",
                ));
            }
            operations.push(Operation::Update {
                path,
                move_to,
                hunks,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = path.trim().to_owned();
            if path.is_empty() {
                return Err(CoreError::invalid("Delete File requires a path."));
            }
            operations.push(Operation::Delete { path });
            index += 1;
        } else {
            return Err(CoreError::invalid(format!("Unknown patch marker: {line}")));
        }
    }

    if operations.is_empty() {
        return Err(CoreError::invalid("Patch has no operations."));
    }
    Ok(operations)
}

fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<(String, usize, usize)> {
    let had_final_newline = content.ends_with('\n');
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let normalized = content.replace("\r\n", "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
    if had_final_newline {
        lines.pop();
    }

    let mut cursor = 0usize;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for (hunk_index, hunk) in hunks.iter().enumerate() {
        if let Some(context) = &hunk.context {
            let found = seek_lines(&lines, std::slice::from_ref(context), cursor, false)
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "Patch hunk {} context did not match the current file: {context}",
                        hunk_index + 1
                    ))
                })?;
            cursor = found + 1;
        }

        let mut old = hunk.old.clone();
        let mut new = hunk.new.clone();
        let mut found = if old.is_empty() {
            Some(if hunk.end_of_file {
                lines.len()
            } else {
                cursor.min(lines.len())
            })
        } else {
            seek_lines(&lines, &old, cursor, hunk.end_of_file)
        };
        if found.is_none() && old.last().is_some_and(String::is_empty) {
            old.pop();
            if new.last().is_some_and(String::is_empty) {
                new.pop();
            }
            if !old.is_empty() {
                found = seek_lines(&lines, &old, cursor, hunk.end_of_file);
            }
        }
        let Some(start) = found else {
            let detail = if old.iter().all(String::is_empty) {
                "an expected blank line".to_owned()
            } else {
                format!("expected lines were not found:\n{}", hunk.old.join("\n"))
            };
            return Err(CoreError::invalid(format!(
                "Patch hunk {} did not match the current file ({detail}).",
                hunk_index + 1
            )));
        };
        lines.splice(start..start + old.len(), new.clone());
        cursor = start + new.len();
        additions += hunk.additions;
        deletions += hunk.deletions;
    }

    let mut result = lines.join(newline);
    if had_final_newline {
        result.push_str(newline);
    }
    Ok((result, additions, deletions))
}

fn seek_lines(
    lines: &[String],
    pattern: &[String],
    start: usize,
    end_of_file: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return None;
    }
    let comparisons: [fn(&str, &str) -> bool; 4] = [exact, rstrip, trim, normalized];
    if end_of_file {
        let offset = lines.len().checked_sub(pattern.len())?;
        if offset < start {
            return None;
        }
        for compare in comparisons {
            if line_matches(lines, pattern, offset, compare) {
                return Some(offset);
            }
        }
        return None;
    }
    for compare in comparisons {
        for offset in start..=lines.len().saturating_sub(pattern.len()) {
            if line_matches(lines, pattern, offset, compare) {
                return Some(offset);
            }
        }
    }
    None
}

fn line_matches(
    lines: &[String],
    pattern: &[String],
    offset: usize,
    compare: fn(&str, &str) -> bool,
) -> bool {
    pattern
        .iter()
        .enumerate()
        .all(|(index, value)| compare(&lines[offset + index], value))
}

fn exact(left: &str, right: &str) -> bool {
    left == right
}

fn rstrip(left: &str, right: &str) -> bool {
    left.trim_end() == right.trim_end()
}

fn trim(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn normalized(left: &str, right: &str) -> bool {
    crate::agent::edit::normalize_for_match(left.trim())
        == crate::agent::edit::normalize_for_match(right.trim())
}

async fn read_text(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|_| CoreError::invalid(format!("File not found: {}", path.display())))?;
    if !metadata.is_file() {
        return Err(CoreError::invalid(format!(
            "Not a file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(CoreError::invalid(format!(
            "File is too large to patch: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).await?;
    if bytes.contains(&0) {
        return Err(CoreError::invalid("Cannot patch a binary file."));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| CoreError::invalid("Cannot patch a file that is not UTF-8."))?;
    if text.chars().count() > MAX_FILE_CHARS {
        return Err(CoreError::invalid("File is too large to patch."));
    }
    Ok(text)
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn relative(workspace: &Path, path: &Path) -> String {
    display_relative(workspace, path)
}

async fn commit(changes: &[Change]) -> Result<()> {
    let mut applied: Vec<usize> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        if let Some(content) = &change.new {
            if let Some(parent) = change.path.parent() {
                if let Err(error) = fs::create_dir_all(parent).await {
                    rollback(changes, &applied).await;
                    return Err(error.into());
                }
            }
            if let Err(error) = fs::write(&change.path, content).await {
                rollback(changes, &applied).await;
                return Err(error.into());
            }
            applied.push(index);
        }
    }
    for (index, change) in changes.iter().enumerate() {
        if change.new.is_some() {
            continue;
        }
        if let Err(error) = fs::remove_file(&change.path).await {
            rollback(changes, &applied).await;
            return Err(error.into());
        }
        applied.push(index);
    }
    Ok(())
}

async fn rollback(changes: &[Change], applied: &[usize]) {
    for index in applied.iter().rev() {
        let change = &changes[*index];
        match (&change.old, &change.new) {
            (Some(old), _) => {
                let _ = fs::write(&change.path, old).await;
            }
            (None, Some(_)) => {
                let _ = fs::remove_file(&change.path).await;
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(text: &str) -> String {
        format!("*** Begin Patch\n{text}\n*** End Patch\n")
    }

    #[tokio::test]
    async fn applies_add_update_move_and_delete() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("old.txt"), "one\ntwo\n").unwrap();

        let text = patch(
            "*** Add File: src/new.txt\n+hello\n+world\n*** Update File: old.txt\n@@\n one\n-two\n+three\n*** Move to: moved.txt\n*** Delete File: obsolete.txt",
        );
        std::fs::write(workspace.join("obsolete.txt"), "bye\n").unwrap();
        let result = apply(&workspace, &text).await.unwrap();
        assert!(result.render().contains("A src/new.txt"));
        assert!(result.render().contains("R old.txt -> moved.txt"));
        assert!(result.render().contains("D obsolete.txt"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("src/new.txt")).unwrap(),
            "hello\nworld\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("moved.txt")).unwrap(),
            "one\nthree\n"
        );
        assert!(!workspace.join("old.txt").exists());
        assert!(!workspace.join("obsolete.txt").exists());
    }

    #[tokio::test]
    async fn applies_multiple_hunks_in_one_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let text = patch("*** Update File: file.txt\n@@\n a\n-b\n+B\n@@\n d\n-e\n+E");
        apply(&workspace, &text).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "a\nB\nc\nd\nE\n"
        );
    }

    #[tokio::test]
    async fn uses_context_and_end_of_file_anchors() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(
            workspace.join("file.txt"),
            "function first()\nold\nfunction second()\nold\n",
        )
        .unwrap();
        let text = patch("*** Update File: file.txt\n@@ function second()\n-old\n+new");
        apply(&workspace, &text).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "function first()\nold\nfunction second()\nnew\n"
        );
    }

    #[tokio::test]
    async fn end_of_file_anchor_updates_the_last_match() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "block\nmiddle\nblock\n").unwrap();
        let text = patch("*** Update File: file.txt\n@@\n-block\n+last\n*** End of File");
        apply(&workspace, &text).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "block\nmiddle\nlast\n"
        );
    }

    #[tokio::test]
    async fn preview_returns_a_diff_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "old\n").unwrap();
        let text = patch("*** Update File: file.txt\n@@\n-old\n+new");
        let diff = preview_with_scope(&workspace, &text, false).await.unwrap();
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "old\n"
        );
    }

    #[tokio::test]
    async fn rejects_duplicate_paths() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "a\n").unwrap();
        let text =
            patch("*** Update File: file.txt\n@@\n-a\n+b\n*** Update File: file.txt\n@@\n-b\n+c");
        assert!(apply(&workspace, &text).await.is_err());
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "a\n"
        );
    }

    #[tokio::test]
    async fn rejects_nonmatching_hunks_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "actual\n").unwrap();
        let text = patch("*** Update File: file.txt\n@@\n-expected\n+new");
        assert!(apply(&workspace, &text).await.is_err());
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "actual\n"
        );
    }

    #[tokio::test]
    async fn preserves_crlf_and_final_newline() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("file.txt"), "one\r\ntwo\r\n").unwrap();
        let text = patch("*** Update File: file.txt\n@@\n one\n-two\n+three");
        apply(&workspace, &text).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("file.txt")).unwrap(),
            "one\r\nthree\r\n"
        );
    }

    #[test]
    fn detects_patch_paths_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        assert!(!touches_outside(
            &workspace,
            "*** Begin Patch\n*** Add File: inside.txt\n+x\n*** End Patch"
        ));
        assert!(touches_outside(
            &workspace,
            "*** Begin Patch\n*** Add File: ../outside.txt\n+x\n*** End Patch"
        ));
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let text = patch("*** Add File: ../outside.txt\n+bad");
        assert!(apply(&workspace, &text).await.is_err());
    }
}
