//! The `skill` tool and the skill catalog.

use super::workspace::{read_text_file, workspace_root_for_conversation};
use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::db::repo::settings;
use crate::util::truncate_chars;
use crate::{CoreError, Result};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const DISABLED_SKILLS_SETTING: &str = "skills.disabled";

/// A skill exposed to the settings UI. Duplicate names keep the first root,
/// which is the same precedence the `skill` tool and prompt catalog use.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
}

pub(super) async fn skill_listing(dir: &Path, name: &str, relative: &str) -> Result<String> {
    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(dir).await?;
    let mut truncated = false;
    while let Some(entry) = reader.next_entry().await? {
        if entries.len() >= 200 {
            truncated = true;
            break;
        }
        let metadata = entry.metadata().await.ok();
        let file_type = entry.file_type().await.ok();
        let label = entry.file_name().to_string_lossy().replace('\\', "/");
        let kind = if file_type.as_ref().is_some_and(|value| value.is_dir()) {
            "dir"
        } else if file_type.as_ref().is_some_and(|value| value.is_file()) {
            "file"
        } else {
            "link"
        };
        let size = metadata
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default();
        entries.push((label, kind, size));
    }
    entries.sort_by_key(|entry| entry.0.to_lowercase());
    let label = if relative.is_empty() || relative == "." {
        name.to_owned()
    } else {
        format!("{name}/{relative}")
    };
    let mut out = format!("# Skill: {label}\n");
    for (entry, kind, size) in entries {
        if kind == "dir" {
            out.push_str(&format!("- {entry}/\n"));
        } else if kind == "file" {
            out.push_str(&format!("- {entry} ({size} B)\n"));
        } else {
            out.push_str(&format!("- {entry} (link)\n"));
        }
    }
    if truncated {
        out.push_str("- ... (more entries not shown)\n");
    }
    Ok(out)
}

pub fn skill_roots(workspace: &Path, output_dir: &Path) -> Vec<PathBuf> {
    skill_roots_with_home(workspace, output_dir, home_dir().as_deref())
}

/// The user-level skill directory shared by every conversation.
pub fn global_skills_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".config").join("mcp-chat").join("skills"))
}

fn skill_roots_with_sources(
    workspace: &Path,
    output_dir: &Path,
    home: Option<&Path>,
) -> Vec<(PathBuf, &'static str)> {
    let mut roots = vec![
        (workspace.join(".mcp-chat").join("skills"), "workspace"),
        (workspace.join(".opencode").join("skills"), "workspace"),
    ];
    if let Some(parent) = output_dir.parent() {
        roots.push((parent.join("skills"), "app"));
    }
    if let Some(home) = home {
        roots.push((
            home.join(".config").join("opencode").join("skills"),
            "opencode",
        ));
        roots.push((
            home.join(".config").join("mcp-chat").join("skills"),
            "global",
        ));
    }
    roots
}

pub(super) fn skill_roots_with_home(
    workspace: &Path,
    output_dir: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    skill_roots_with_sources(workspace, output_dir, home)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

pub(super) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

pub(super) fn skill_description(path: &Path) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let content = content.chars().take(8_000).collect::<String>();
    let mut frontmatter = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "---" {
            frontmatter = !frontmatter;
            continue;
        }
        if frontmatter {
            if let Some(value) = line.strip_prefix("description:") {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return truncate_chars(value, 1024).0;
                }
            }
        }
    }
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("---") {
            continue;
        }
        return truncate_chars(line, 1024).0;
    }
    String::new()
}

pub fn skill_catalog_infos(workspace: &Path, output_dir: &Path) -> Vec<SkillInfo> {
    let mut skills = BTreeMap::new();
    for (root, source) in skill_roots_with_sources(workspace, output_dir, home_dir().as_deref()) {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let manifest = path.join("SKILL.md");
            if !manifest.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            skills.entry(name.to_owned()).or_insert_with(|| SkillInfo {
                name: name.to_owned(),
                description: skill_description(&manifest),
                path: manifest.display().to_string(),
                source: source.to_owned(),
            });
        }
    }
    skills.into_values().collect()
}

pub fn skill_catalog(workspace: &Path, output_dir: &Path) -> Vec<(String, String)> {
    skill_catalog_infos(workspace, output_dir)
        .into_iter()
        .map(|skill| (skill.name, skill.description))
        .collect()
}

/// Names disabled in Settings. Missing or malformed values mean no disabled
/// skills, so a bad setting never removes the whole catalog.
pub async fn disabled_skill_names(pool: &SqlitePool) -> Result<HashSet<String>> {
    let Some(raw) = settings::get(pool, DISABLED_SKILLS_SETTING).await? else {
        return Ok(HashSet::new());
    };
    let names = serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default();
    Ok(names
        .into_iter()
        .filter(|name| valid_skill_name(name))
        .collect())
}

pub async fn set_skill_enabled(pool: &SqlitePool, name: &str, enabled: bool) -> Result<()> {
    let name = name.trim();
    if !valid_skill_name(name) {
        return Err(CoreError::invalid("Invalid skill name."));
    }
    let mut names = disabled_skill_names(pool)
        .await?
        .into_iter()
        .collect::<Vec<_>>();
    if enabled {
        names.retain(|item| item != name);
    } else if !names.iter().any(|item| item == name) {
        names.push(name.to_owned());
    }
    names.sort();
    settings::set(
        pool,
        DISABLED_SKILLS_SETTING,
        &serde_json::to_string(&names)?,
    )
    .await
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

impl ToolExecutor {
    pub(super) async fn run_skill(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let name = req
            .args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let file = req
            .args
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let workspace =
            workspace_root_for_conversation(&self.db.pool, &req.conversation_id).await?;
        let roots = skill_roots(&workspace, &self.output_dir);
        let disabled = disabled_skill_names(&self.db.pool).await?;
        let catalog = skill_catalog(&workspace, &self.output_dir)
            .into_iter()
            .filter(|(name, _)| !disabled.contains(name))
            .collect::<Vec<_>>();
        if name.is_empty() {
            if file.is_some() {
                return Err(CoreError::invalid(
                    "A skill name is required when reading a skill file.",
                ));
            }
            let content = if catalog.is_empty() {
                "No skills enabled. Add <name>/SKILL.md to .mcp-chat/skills, .opencode/skills, app data, or ~/.config/opencode/skills, then enable it in Skills.".into()
            } else {
                format!(
                    "Available skills:\n{}",
                    catalog
                        .iter()
                        .map(|(name, description)| if description.is_empty() {
                            format!("- {name}")
                        } else {
                            format!("- {name}: {description}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            return Ok(Rendered {
                content,
                structured: Some(
                    json!({"skills": catalog.iter().map(|(name, _)| name).collect::<Vec<_>>()}),
                ),
                ok: true,
            });
        }
        if !valid_skill_name(name) {
            return Err(CoreError::invalid("Invalid skill name."));
        }
        let skill_dir = roots
            .iter()
            .map(|root| root.join(name))
            .find(|path| path.join("SKILL.md").is_file());
        if skill_dir.is_some() && disabled.contains(name) {
            return Err(CoreError::invalid(format!(
                "Skill {name:?} is disabled. Enable it in Skills settings."
            )));
        }
        let skill_dir = skill_dir.ok_or_else(|| {
            CoreError::invalid(format!(
                "Skill {name:?} was not found. Available skills: {}",
                if catalog.is_empty() {
                    "(none)".into()
                } else {
                    catalog
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ))
        })?;
        let canonical_dir = skill_dir.canonicalize()?;
        let requested = file.unwrap_or("SKILL.md");
        if requested.len() > 512 || Path::new(requested).is_absolute() {
            return Err(CoreError::invalid("Invalid skill file path."));
        }
        let target = skill_dir.join(requested);
        let target = target
            .canonicalize()
            .map_err(|_| CoreError::invalid(format!("Skill file not found: {requested}")))?;
        if !target.starts_with(&canonical_dir) {
            return Err(CoreError::invalid(
                "Skill file must stay inside the skill directory.",
            ));
        }
        let relative = target
            .strip_prefix(&canonical_dir)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        if target.is_dir() {
            let listing = skill_listing(&target, name, &relative).await?;
            return Ok(Rendered {
                content: listing,
                structured: None,
                ok: true,
            });
        }
        if !target.is_file() {
            return Err(CoreError::invalid("Skill path is not a file or directory."));
        }
        let content = read_text_file(&target).await?;
        let label = if file.is_none() {
            name.to_owned()
        } else {
            format!("{name}/{relative}")
        };
        Ok(Rendered {
            content: format!("# Skill: {label}\n\n{content}"),
            structured: None,
            ok: true,
        })
    }
}
