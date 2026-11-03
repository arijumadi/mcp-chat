//! Built-in tool definitions (schemas and descriptions) and default effects.

use crate::agent::capabilities::{Capability, table};
use crate::agent::permissions::Effect;
use crate::llm::ToolDef;
use crate::mcp::types::ToolInfo;
use serde_json::{Value, json};

pub fn builtin_tool_defs() -> Vec<ToolDef> {
    table()
        .tools
        .iter()
        .map(|capability| ToolDef {
            name: capability.name.clone(),
            description: capability.description.clone(),
            input_schema: input_schema(capability),
        })
        .collect()
}

/// The JSON schema of a tool: its own parameters, plus the override flag of
/// its guard (`overwrite`, `allowFileWrites`) which the capability table owns.
fn input_schema(capability: &Capability) -> Value {
    let mut schema = base_schema(&capability.name);
    if let Some(guard) = &capability.guard {
        schema["properties"][&guard.override_param] =
            json!({"type": "boolean", "description": guard.override_description});
    }
    schema
}

fn base_schema(name: &str) -> Value {
    match name {
        "read_tool_output" => json!({
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
        "bash" => json!({
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
                "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        "python" => json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "Complete Python source to execute."},
                "cwd": {"type": "string", "description": "Optional workspace directory."},
                "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Timeout in milliseconds. Default 120000."}
            },
            "required": ["code"],
            "additionalProperties": false
        }),
        "edit" => json!({
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
        "apply_patch" => json!({
            "type": "object",
            "properties": {
                "patchText": {"type": "string", "description": "Patch text using the marker format above."}
            },
            "required": ["patchText"],
            "additionalProperties": false
        }),
        "write" => json!({
            "type": "object",
            "properties": {
                "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                "content": {"type": "string", "description": "Complete file content."}
            },
            "required": ["filePath", "content"],
            "additionalProperties": false
        }),
        "read" => json!({
            "type": "object",
            "properties": {
                "filePath": {"type": "string", "description": "Workspace-relative path, or an explicit outside path."},
                "offset": {"type": "integer", "minimum": 1, "description": "First line to return (1-based). Default 1."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 5000, "description": "Maximum lines to return. Default 2000."}
            },
            "required": ["filePath"],
            "additionalProperties": false
        }),
        "grep" => json!({
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
        "glob" => json!({
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
        "skill" => json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Skill directory name. Empty lists available skills."},
                "file": {"type": "string", "description": "Optional path relative to the skill directory. A directory returns a listing."}
            },
            "additionalProperties": false
        }),
        "question" => json!({
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
        other => panic!("capability {other} has no input schema"),
    }
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
    table().get(name).is_some()
}

pub fn builtin_default_effect(name: &str) -> Effect {
    table()
        .get(name)
        .map_or(Effect::Allow, |capability| capability.default_effect)
}
