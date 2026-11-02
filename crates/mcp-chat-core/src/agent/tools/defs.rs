//! Built-in tool definitions (schemas and descriptions) and default effects.

use crate::agent::permissions::Effect;
use crate::llm::ToolDef;
use crate::mcp::types::ToolInfo;
use serde_json::json;

pub fn builtin_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_tool_output".into(),
            description: "Read a truncated tool result by call_id. Use offset for character paging, or line + maxLines for line-based paging.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "call_id": {"type": "string", "description": "The tool call id to read."},
                    "offset": {"type": "integer", "minimum": 0, "description": "Character offset. Default 0. Pages end on line boundaries when possible."},
                    "line": {"type": "integer", "minimum": 1, "description": "Optional 1-based line to start from. Takes precedence over offset."},
                    "maxLines": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines when using line-based paging. Default 200."}
                },
                "required": ["call_id"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "bash".into(),
            description: "Run a shell command for build, test, lint, git, package managers, process/system/disk/network inspection, file management (rename/move/delete), or running scripts. Do not use bash to read or modify file contents; use read, write, or apply_patch instead. Before calling bash, ask: does this command change the contents of a file? If yes, use a file tool.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run. Do not wrap it in another invocation of the same shell."},
                    "shell": {
                        "type": "string",
                        "enum": ["auto", "cmd", "powershell", "pwsh", "sh", "bash"],
                        "description": "Default auto: powershell on Windows, sh elsewhere. For PowerShell send the script body only."
                    },
                    "cwd": {"type": "string", "description": "Optional workspace-relative working directory."},
                    "maxLines": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Optional maximum lines of stdout/stderr to return."},
                    "tail": {"type": "boolean", "description": "Keep the last maxLines instead of the first. Default false."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."},
                    "allowFileWrites": {"type": "boolean", "description": "Allow redirects, tee, sed -i or Set-Content to write file contents. Default false (blocked)."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "python".into(),
            description: "Run Python code through stdin, with no shell quoting. For data/spreadsheet processing and verification scripts. Starts in the workspace with a timeout; approved like a shell command.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "Complete Python source to execute."},
                    "cwd": {"type": "string", "description": "Optional workspace directory."},
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "edit".into(),
            description: "Replace one exact string in one existing file. Use edit for a single exact replacement; use apply_patch for multiple hunks, multiple files, add/delete/move, or context-anchored changes. Matching tolerates CRLF/LF, typographic punctuation, and trailing whitespace. Outside-workspace targets need approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "oldString": {"type": "string", "description": "Text to replace. It must occur exactly once unless replaceAll is true."},
                    "newString": {"type": "string", "description": "Replacement text. Use an empty string to delete."},
                    "replaceAll": {"type": "boolean", "description": "Replace every occurrence. Default false."}
                },
                "required": ["filePath", "oldString", "newString"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "apply_patch".into(),
            description: "Apply a multi-file patch with add/update/delete/move. Use this for existing-file changes, multiple hunks, multiple files, or context anchors. Format: `*** Begin Patch`, Add/Update/Delete File markers, `*** End Patch`. Update hunks start with `@@` (optional context anchor); `*** End of File` anchors EOF. Context/removed/added lines use space/-/+; Add lines use +. Optional `*** Move to: path`. Outside paths need approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patchText": {"type": "string", "description": "Patch text using the marker format above."}
                },
                "required": ["patchText"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "write".into(),
            description: "Create a new file. Existing files are rejected unless overwrite is true. Use apply_patch to modify an existing file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "content": {"type": "string", "description": "Complete file content."},
                    "overwrite": {"type": "boolean", "description": "Replace an existing file entirely. Default false."}
                },
                "required": ["filePath", "content"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "read".into(),
            description: "Read a text file with line numbers, or an image file as a visual attachment. Explicit outside paths need approval unless auto mode is on. Use offset and limit for large text files.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to return (1-based). Default 1."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines to return. Default 2000."}
                },
                "required": ["filePath"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "grep".into(),
            description: "Search text with a regular expression; returns path:line:text. Explicit outside paths need approval unless auto mode is on.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust/PCRE-style regular expression."},
                    "path": {"type": "string", "description": "Optional file or directory, workspace-relative or explicit outside path. Default workspace root."},
                    "include": {"type": "string", "description": "Optional glob limiting files, e.g. **/*.rs."},
                    "caseSensitive": {"type": "boolean", "description": "Default true."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matching lines. Default 200."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files by glob pattern; build and dependency directories are skipped. Explicit outside paths need approval unless auto mode is on.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob such as **/*.rs or src/**/*.ts."},
                    "path": {"type": "string", "description": "Optional directory, workspace-relative or explicit outside path."},
                    "recursive": {"type": "boolean", "description": "Search subdirectories. Default true."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Maximum paths. Default 500."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "skill".into(),
            description: "Load a skill by name. Empty name lists skills. Without file, returns SKILL.md; with file, reads a file or lists a subfolder inside that skill only.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill directory name. Empty lists available skills."},
                    "file": {"type": "string", "description": "Optional path relative to the skill directory. A directory returns a listing."}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "question".into(),
            description: "Ask the user one or more questions and wait for answers. Use this when a decision or missing preference cannot be inferred. Each question can offer options; the user can also type a custom answer.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": {"type": "string", "description": "Short label for the question."},
                                "question": {"type": "string", "description": "The full question text."},
                                "multiple": {"type": "boolean", "description": "Allow selecting more than one option. Default false."},
                                "options": {
                                    "type": "array",
                                    "minItems": 1,
                                    "maxItems": 12,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"],
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "required": ["header", "question", "options"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn builtin_tool_def(name: &str) -> Option<ToolDef> {
    builtin_tool_defs()
        .into_iter()
        .find(|definition| definition.name == name)
}

pub fn builtin_tool_info(name: &str) -> Option<ToolInfo> {
    builtin_tool_def(name).map(|definition| ToolInfo {
        name: definition.name,
        title: None,
        description: definition.description,
        input_schema: definition.input_schema,
        output_schema: None,
        annotations: None,
    })
}

pub fn is_builtin_tool(name: &str) -> bool {
    matches!(
        name,
        "read_tool_output"
            | "bash"
            | "python"
            | "edit"
            | "apply_patch"
            | "write"
            | "read"
            | "grep"
            | "glob"
            | "skill"
            | "question"
    )
}

pub fn builtin_default_effect(name: &str) -> Effect {
    match name {
        "bash" | "python" | "edit" | "apply_patch" | "write" => Effect::Ask,
        _ => Effect::Allow,
    }
}
