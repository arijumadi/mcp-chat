//! The capability table: one source of truth for built-in tool routing.
//!
//! `shared/capabilities.json` describes every built-in tool once: its label,
//! risk class, description, guard, and what the UI should do with it. This
//! module loads and validates that file, and renders the text that used to be
//! copied between places:
//!
//! - tool descriptions (`builtin_tool_defs`)
//! - the routing table and self-check in the harness rules
//! - the refusal messages of the `write` and `bash` guards
//! - default approval effects, dangerous-call rules, outside-path checks
//!
//! The frontend imports the same file, so labels and "this tool produces a
//! diff" never drift from the backend either.
//!
//! Text uses `{role}` placeholders (for example `{modify_file}`), so the fact
//! that existing files are changed with one tool is written down exactly once.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::permissions::Effect;

const SOURCE: &str = include_str!("../../../../shared/capabilities.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    Interactive,
    FileWrite,
    Shell,
    CodeExec,
}

/// How a tool's output is limited and summarized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    Shell,
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessKind {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessFormat {
    Path,
    Patch,
}

/// The argument that names the files a tool touches.
#[derive(Debug, Clone, Deserialize)]
pub struct Access {
    pub kind: AccessKind,
    pub arg: String,
    pub format: AccessFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerKind {
    ShellCommand,
    PatchDelete,
}

/// A call that keeps asking for approval even when auto mode is on.
#[derive(Debug, Clone, Deserialize)]
pub struct Danger {
    pub kind: DangerKind,
    pub arg: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardKind {
    /// Refuse shell commands that write file contents.
    ShellContentWrite,
    /// Refuse writing over a file that already exists.
    NoOverwrite,
}

/// A refusal rule with an explicit, reviewable escape hatch.
#[derive(Debug, Clone)]
pub struct Guard {
    pub kind: GuardKind,
    /// Name of the boolean argument that lifts the guard.
    pub override_param: String,
    pub override_description: String,
    /// What the model is told when the guard refuses the call.
    pub blocked_message: String,
    /// Whether using the override must still be approved in auto mode.
    pub override_needs_review: bool,
}

#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub label: String,
    pub risk: Risk,
    pub default_effect: Effect,
    pub description: String,
    pub produces_diff: bool,
    pub output: Option<OutputKind>,
    pub access: Option<Access>,
    pub danger: Option<Danger>,
    pub guard: Option<Guard>,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub task: String,
    /// Tool names, already resolved from roles.
    pub tools: Vec<String>,
}

#[derive(Debug)]
pub struct Capabilities {
    pub tools: Vec<Capability>,
    pub routes: Vec<Route>,
    pub self_check: String,
    roles: BTreeMap<String, String>,
}

impl Capabilities {
    pub fn get(&self, name: &str) -> Option<&Capability> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// The tool that fills a role such as `modify_file`.
    pub fn role(&self, role: &str) -> Option<&str> {
        self.roles.get(role).map(String::as_str)
    }

    /// One line per route, for the system prompt: ``Baca file -> `read` ``.
    pub fn routing_lines(&self) -> Vec<String> {
        self.routes
            .iter()
            .map(|route| {
                let tools = route
                    .tools
                    .iter()
                    .map(|tool| format!("`{tool}`"))
                    .collect::<Vec<_>>()
                    .join("/");
                format!("{} -> {tools}", route.task)
            })
            .collect()
    }
}

static TABLE: LazyLock<Capabilities> = LazyLock::new(|| {
    load(SOURCE).unwrap_or_else(|error| panic!("invalid capabilities.json: {error}"))
});

/// The validated table. Panics on first use if the embedded file is invalid;
/// `table_is_valid` keeps that from reaching a release.
pub fn table() -> &'static Capabilities {
    &TABLE
}

// ---- parsing -----------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    version: u32,
    roles: BTreeMap<String, String>,
    risks: BTreeMap<Risk, String>,
    routes: Vec<RawRoute>,
    #[serde(rename = "selfCheck")]
    self_check: String,
    tools: Vec<RawTool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoute {
    task: String,
    #[serde(rename = "use")]
    roles: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    name: String,
    label: String,
    risk: Risk,
    description: Vec<String>,
    #[serde(default)]
    diff: bool,
    #[serde(default)]
    output: Option<OutputKind>,
    #[serde(default)]
    access: Option<Access>,
    #[serde(default)]
    dangerous: Option<Danger>,
    #[serde(default)]
    guard: Option<RawGuard>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuard {
    kind: GuardKind,
    #[serde(rename = "override")]
    override_param: String,
    #[serde(rename = "overrideDescription")]
    override_description: String,
    #[serde(rename = "blockedMessage")]
    blocked_message: String,
    #[serde(rename = "overrideNeedsReview")]
    override_needs_review: bool,
}

/// Replace `{role}` and `{override}` placeholders. Unknown placeholders are an error.
fn render(
    template: &str,
    roles: &BTreeMap<String, String>,
    override_param: Option<&str>,
) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed placeholder in {template:?}"))?;
        let key = &after[..end];
        let value = if key == "override" {
            override_param
                .ok_or_else(|| format!("{{override}} used without a guard in {template:?}"))?
        } else {
            roles
                .get(key)
                .map(String::as_str)
                .ok_or_else(|| format!("unknown placeholder {{{key}}} in {template:?}"))?
        };
        out.push_str(value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn effect_from(value: &str) -> Result<Effect, String> {
    match value {
        "allow" => Ok(Effect::Allow),
        "ask" => Ok(Effect::Ask),
        other => Err(format!("unknown effect {other:?}; use allow or ask")),
    }
}

/// Parse and validate the table. Exposed for tests; use [`table`] elsewhere.
pub fn load(source: &str) -> Result<Capabilities, String> {
    let raw: RawTable = serde_json::from_str(source).map_err(|error| error.to_string())?;
    if raw.version != 1 {
        return Err(format!("unsupported version {}", raw.version));
    }

    let mut seen = BTreeSet::new();
    for tool in &raw.tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(format!("duplicate tool {}", tool.name));
        }
    }
    for (role, tool) in &raw.roles {
        if !seen.contains(tool.as_str()) {
            return Err(format!("role {role} points at unknown tool {tool}"));
        }
    }

    let mut tools = Vec::new();
    let mut overrides = BTreeSet::new();
    for tool in &raw.tools {
        let effect = raw
            .risks
            .get(&tool.risk)
            .ok_or_else(|| {
                format!(
                    "risk {:?} of {} has no default effect",
                    tool.risk, tool.name
                )
            })
            .and_then(|value| effect_from(value))?;
        let guard = tool
            .guard
            .as_ref()
            .map(|guard| -> Result<Guard, String> {
                if !overrides.insert(guard.override_param.as_str()) {
                    return Err(format!("override {} is used twice", guard.override_param));
                }
                Ok(Guard {
                    kind: guard.kind,
                    override_param: guard.override_param.clone(),
                    override_description: guard.override_description.clone(),
                    blocked_message: render(
                        &guard.blocked_message,
                        &raw.roles,
                        Some(&guard.override_param),
                    )?,
                    override_needs_review: guard.override_needs_review,
                })
            })
            .transpose()?;
        let override_param = guard.as_ref().map(|guard| guard.override_param.as_str());
        if tool.description.is_empty() {
            return Err(format!("{} has no description", tool.name));
        }
        let description = tool
            .description
            .iter()
            .map(|part| render(part, &raw.roles, override_param))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        if tool.diff && tool.risk != Risk::FileWrite {
            return Err(format!(
                "{} produces a diff but is not a file_write tool",
                tool.name
            ));
        }
        if let Some(access) = &tool.access {
            if tool.risk == Risk::FileWrite && access.kind != AccessKind::Write {
                return Err(format!(
                    "{} is file_write but its access is not write",
                    tool.name
                ));
            }
        }
        if let Some(danger) = &tool.dangerous {
            if danger.arg.is_empty() {
                return Err(format!(
                    "{} has a danger rule without an argument",
                    tool.name
                ));
            }
        }
        tools.push(Capability {
            name: tool.name.clone(),
            label: tool.label.clone(),
            risk: tool.risk,
            default_effect: effect,
            description,
            produces_diff: tool.diff,
            output: tool.output,
            access: tool.access.clone(),
            danger: tool.dangerous.clone(),
            guard,
        });
    }

    let mut routes = Vec::new();
    for route in &raw.routes {
        let mut names = Vec::new();
        for role in &route.roles {
            let tool = raw
                .roles
                .get(role)
                .ok_or_else(|| format!("route {:?} uses unknown role {role}", route.task))?;
            names.push(tool.clone());
        }
        if names.is_empty() {
            return Err(format!("route {:?} has no tools", route.task));
        }
        routes.push(Route {
            task: route.task.clone(),
            tools: names,
        });
    }

    let self_check = render(&raw.self_check, &raw.roles, None)?;
    Ok(Capabilities {
        tools,
        routes,
        self_check,
        roles: raw.roles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_valid() {
        let table = load(SOURCE).expect("capabilities.json must load");
        assert_eq!(table.tools.len(), 12);
        assert_eq!(table.routes.len(), 9);
    }

    #[test]
    fn descriptions_are_rendered_from_roles() {
        let table = table();
        let bash = table.get("bash").unwrap();
        assert!(bash.description.contains(
            "Do not use bash to read or modify file contents; use read, write, or apply_patch instead."
        ));
        let write = table.get("write").unwrap();
        assert_eq!(
            write.description,
            "Create a new file. Existing files are rejected unless overwrite is true. Use apply_patch to modify an existing file."
        );
        for tool in &table.tools {
            assert!(
                !tool.description.contains('{') && !tool.description.contains('}'),
                "{} has an unrendered placeholder: {}",
                tool.name,
                tool.description
            );
        }
    }

    #[test]
    fn guard_messages_are_the_ones_the_model_is_told() {
        let table = table();
        assert_eq!(
            table
                .get("bash")
                .unwrap()
                .guard
                .as_ref()
                .unwrap()
                .blocked_message,
            "BLOCKED: bash tidak boleh mengubah isi file. Pakai write (file baru) atau apply_patch (file existing). Untuk operasi yang memang disengaja, set allowFileWrites=true."
        );
        assert_eq!(
            table
                .get("write")
                .unwrap()
                .guard
                .as_ref()
                .unwrap()
                .blocked_message,
            "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file."
        );
    }

    #[test]
    fn routing_table_and_self_check_come_from_the_table() {
        let table = table();
        assert_eq!(
            table.routing_lines(),
            [
                "Baca file -> `read`",
                "Cari pola/file -> `grep`/`glob`",
                "File baru -> `write`",
                "Ubah file existing -> `apply_patch`",
                "Satu exact replacement -> `edit`",
                "Build/test/git/package manager -> `bash`",
                "Rename/move/delete massal -> `bash`",
                "Proses/port/disk/env -> `bash`",
                "Klarifikasi blocking -> `question`",
            ]
        );
        assert_eq!(
            table.self_check,
            "Sebelum bash: apakah command ini mengubah isi file? Kalau ya, tool-nya salah."
        );
    }

    #[test]
    fn question_has_one_route_and_the_file_and_shell_routes_do_not_use_it() {
        let table = table();
        assert_eq!(table.role("ask_user"), Some("question"));
        for route in &table.routes {
            let asks = route.tools.iter().any(|tool| tool == "question");
            assert_eq!(
                asks,
                route.task == "Klarifikasi blocking",
                "route {:?} -> {:?}",
                route.task,
                route.tools
            );
        }
        let clarify = table
            .routes
            .iter()
            .find(|route| route.task == "Klarifikasi blocking")
            .expect("a route for blocking clarification");
        assert_eq!(clarify.tools, ["question"]);
    }

    #[test]
    fn question_description_says_when_to_ask_and_when_not_to() {
        let question = table().get("question").unwrap();
        assert_eq!(question.risk, Risk::Interactive);
        assert_eq!(question.default_effect, Effect::Allow);
        for expected in [
            "Ask the user one or more blocking questions when the answer changes what you do next and cannot be inferred from the workspace.",
            "Options are optional; omit them for free-form answers.",
            "Do not use this for status updates, explanations, or questions you can answer yourself.",
            "Examples:",
        ] {
            assert!(
                question.description.contains(expected),
                "missing {expected:?} in {}",
                question.description
            );
        }
    }

    #[test]
    fn risk_classes_decide_the_default_effect() {
        let table = table();
        for name in ["bash", "python", "edit", "apply_patch", "write"] {
            assert_eq!(
                table.get(name).unwrap().default_effect,
                Effect::Ask,
                "{name}"
            );
        }
        for name in [
            "read",
            "grep",
            "glob",
            "skill",
            "question",
            "read_tool_output",
        ] {
            assert_eq!(
                table.get(name).unwrap().default_effect,
                Effect::Allow,
                "{name}"
            );
        }
    }

    #[test]
    fn only_file_write_tools_produce_diffs() {
        let diff: Vec<_> = table()
            .tools
            .iter()
            .filter(|tool| tool.produces_diff)
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(diff, ["edit", "apply_patch", "write"]);
    }

    fn broken(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Capabilities, String> {
        let mut value: serde_json::Value = serde_json::from_str(SOURCE).unwrap();
        edit(&mut value);
        load(&value.to_string())
    }

    #[test]
    fn a_broken_table_is_rejected_with_a_clear_reason() {
        let error = |result: Result<Capabilities, String>| result.expect_err("should be rejected");

        assert!(
            error(broken(
                |v| v["tools"][1]["description"][1] = "use {nope}".into()
            ))
            .contains("unknown placeholder {nope}")
        );
        assert!(
            error(broken(|v| v["roles"]["modify_file"] = "ghost".into()))
                .contains("unknown tool ghost")
        );
        assert!(
            error(broken(|v| v["routes"][0]["use"][0] = "ghost_role".into()))
                .contains("unknown role ghost_role")
        );
        assert!(
            error(broken(|v| v["tools"][1]["name"] = "read".into()))
                .contains("duplicate tool read")
        );
        assert!(
            error(broken(|v| v["tools"][1]["risk"] = "shell_ish".into()))
                .contains("unknown variant")
        );
        assert!(error(broken(|v| v["risks"]["shell"] = "maybe".into())).contains("unknown effect"));
        assert!(error(broken(|v| v["version"] = 2.into())).contains("unsupported version"));
        assert!(
            error(broken(|v| v["tools"][0]["surprise"] = true.into())).contains("unknown field")
        );
        // A diff-producing tool must be a file_write tool.
        assert!(
            error(broken(|v| v["tools"][0]["diff"] = true.into()))
                .contains("not a file_write tool")
        );
    }
}
