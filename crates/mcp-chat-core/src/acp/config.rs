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

/// Built-in templates.
///
/// Each command was checked against the published package, its own
/// documentation, or the shipped binary — not guessed:
///
/// - `opencode acp` — opencode.ai/docs/acp.
/// - `npx -y @agentclientprotocol/codex-acp` — the adapter's README documents
///   exactly this, with no extra flag. The older `@zed-industries/codex-acp`
///   package still exists but is not the one ACP lists.
/// - `gemini --acp` — ACP mode is documented in the package's own
///   `docs/cli/acp-mode.md`.
/// - `antigravity acp` — `agy --help` in release 1.5.18 lists `acp` as a
///   subcommand: "Start the Agent Client Protocol (ACP) stdio server". This is
///   Google's own binary, not the third-party `antigravity-acp` wrapper.
///
/// A preset is a template only; it is never inserted automatically.
pub const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        name: "OpenCode",
        slug: "opencode",
        command: "opencode",
        args: &["acp"],
        notes: "Requires the opencode CLI on PATH. Sign in once with `opencode auth login`.",
    },
    AgentPreset {
        name: "Codex",
        slug: "codex",
        command: "npx",
        args: &["-y", "@agentclientprotocol/codex-acp"],
        notes: "Downloads @agentclientprotocol/codex-acp through npx on first run.",
    },
    AgentPreset {
        name: "Gemini",
        slug: "gemini",
        command: "gemini",
        args: &["--acp"],
        notes: "Requires the gemini CLI on PATH.",
    },
    AgentPreset {
        name: "Antigravity",
        slug: "antigravity",
        command: "antigravity",
        args: &["acp"],
        notes: "Requires Google's Antigravity CLI (`agy`) on PATH. Sign in once by running `antigravity`.",
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
        assert!(slugs.contains(&"antigravity"));
    }

    /// These flags were verified against each vendor, so a typo here would be
    /// a real regression rather than a style change.
    #[test]
    fn preset_commands_match_their_vendors() {
        let find = |slug: &str| {
            PRESETS
                .iter()
                .find(|preset| preset.slug == slug)
                .expect("preset exists")
        };
        assert_eq!(find("opencode").args, &["acp"]);
        assert_eq!(find("gemini").args, &["--acp"]);
        assert_eq!(find("antigravity").command, "antigravity");
        assert_eq!(find("antigravity").args, &["acp"]);
        // The official adapter, not the older Zed-hosted package.
        assert_eq!(
            find("codex").args,
            &["-y", "@agentclientprotocol/codex-acp"]
        );
    }

    #[test]
    fn every_preset_says_how_to_get_the_cli() {
        for preset in PRESETS {
            assert!(
                preset.notes.to_lowercase().contains("path")
                    || preset.notes.to_lowercase().contains("npx"),
                "{} must say how to install the command: {}",
                preset.slug,
                preset.notes
            );
        }
    }
}
