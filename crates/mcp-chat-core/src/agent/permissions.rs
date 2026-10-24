use globset::GlobBuilder;

use crate::models::PermissionRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Deny,
    Ask,
}

impl Effect {
    pub fn parse(value: &str) -> Self {
        match value {
            "allow" => Self::Allow,
            "deny" => Self::Deny,
            _ => Self::Ask,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }
}

/// Last matching rule wins. `*` is a glob, and the default comes from the
/// server policy. This is intentionally tiny and testable.
pub fn evaluate(action: &str, resource: &str, default: Effect, rules: &[PermissionRule]) -> Effect {
    let mut effect = default;
    for rule in rules {
        if glob_match(&rule.action, action) && glob_match(&rule.resource, resource) {
            effect = Effect::parse(&rule.effect);
        }
    }
    effect
}

fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match GlobBuilder::new(pattern).literal_separator(false).build() {
        Ok(glob) => glob.compile_matcher().is_match(value),
        Err(_) => pattern == value,
    }
}

/// Normalize a tool argument into the resource a permission rule targets.
/// Hosts, paths and channel names are the usual risky surfaces.
pub fn permission_resource(args: &serde_json::Value) -> String {
    const KEYS: &[&str] = &[
        "url",
        "uri",
        "host",
        "hostname",
        "path",
        "channel",
        "repo",
        "repository",
        "owner",
        "database",
        "table",
        "command",
        "filePath",
        "file_path",
        "pattern",
    ];
    for key in KEYS {
        if let Some(value) = args.get(*key) {
            if let Some(text) = value.as_str() {
                if let Ok(url) = url::Url::parse(text) {
                    if let Some(host) = url.host_str() {
                        return host.chars().take(200).collect();
                    }
                }
                return text.chars().take(200).collect();
            }
        }
    }
    "*".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: &str, resource: &str, effect: &str) -> PermissionRule {
        PermissionRule {
            id: "r".into(),
            server_id: None,
            action: action.into(),
            resource: resource.into(),
            effect: effect.into(),
            created_at: 0,
        }
    }

    #[test]
    fn last_matching_rule_wins() {
        let rules = vec![
            rule("mcp.mock.*", "*", "allow"),
            rule("mcp.mock.delete", "*", "deny"),
        ];
        assert_eq!(
            evaluate("mcp.mock.echo", "*", Effect::Ask, &rules),
            Effect::Allow
        );
        assert_eq!(
            evaluate("mcp.mock.delete", "*", Effect::Ask, &rules),
            Effect::Deny
        );
    }

    #[test]
    fn default_policy_is_used_without_matches() {
        assert_eq!(
            evaluate("mcp.mock.echo", "*", Effect::Ask, &[]),
            Effect::Ask
        );
    }

    #[test]
    fn extracts_host_from_url() {
        assert_eq!(
            permission_resource(&serde_json::json!({"url": "https://example.com/path"})),
            "example.com"
        );
    }
}
