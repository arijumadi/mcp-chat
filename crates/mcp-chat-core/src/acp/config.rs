//! Permission policy and agent presets for ACP subagents.
//!
//! The registry row itself lives in `db::repo::acp_agents` next to the other
//! SQLite repositories. This module holds the policy decisions and the preset
//! templates that never get inserted automatically.

/// A starting point shown in the UI as a template. Presets are never inserted
/// into the database automatically: a command that is not installed on this
/// machine must not end up in the prompt catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPreset {
    pub name: &'static str,
    pub slug: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub notes: &'static str,
}

/// Built-in templates. The command flags are the ones verified during Phase 0
/// for the package itself; the CLI flag combination must be re-checked against
/// the installed binary before enabling a preset.
pub const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        name: "OpenCode",
        slug: "opencode",
        command: "opencode",
        args: &["acp"],
        notes: "Requires the opencode CLI on PATH.",
    },
    AgentPreset {
        name: "Codex",
        slug: "codex",
        command: "npx",
        args: &["-y", "@zed-industries/codex-acp"],
        notes: "Downloads @zed-industries/codex-acp through npx on first run.",
    },
    AgentPreset {
        name: "Gemini",
        slug: "gemini",
        command: "gemini",
        args: &["--acp"],
        notes: "Requires the gemini CLI on PATH.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_unique_valid_slugs() {
        let mut seen = std::collections::HashSet::new();
        for preset in PRESETS {
            assert!(
                crate::db::repo::acp_agents::is_valid_slug(preset.slug),
                "invalid preset slug: {}",
                preset.slug
            );
            assert!(seen.insert(preset.slug), "duplicate preset slug");
            assert!(!preset.command.trim().is_empty());
        }
    }

    #[test]
    fn presets_cover_the_documented_agents() {
        let slugs: Vec<&str> = PRESETS.iter().map(|preset| preset.slug).collect();
        assert!(slugs.contains(&"opencode"));
        assert!(slugs.contains(&"codex"));
        assert!(slugs.contains(&"gemini"));
    }
}
