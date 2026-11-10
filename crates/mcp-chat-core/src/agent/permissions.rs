use globset::GlobBuilder;
use serde_json::Value;

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

/// What an "Always allow" click should save. Shell commands may use a
/// conservative prefix pattern; everything else keeps the exact resource.
pub fn save_pattern(action: &str, resource: &str, args: &Value) -> String {
    if action == "builtin.bash" {
        if let Some(command) = args.get("command").and_then(Value::as_str) {
            if let Some(pattern) = shell_save_pattern(command) {
                if pattern.chars().count() <= 200 && glob_match(&pattern, resource) {
                    return pattern;
                }
            }
        }
    }
    resource.to_owned()
}

fn shell_save_pattern(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || has_shell_control(command) {
        return None;
    }
    let words = command_words(command)?;
    let first = words.first()?.to_ascii_lowercase();
    let count = pattern_word_count(&first, &words)?;
    if words.len() < count {
        return None;
    }
    let prefix = &words[..count];
    if prefix.iter().any(|word| {
        word.is_empty()
            || word
                .chars()
                .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '$' | '`'))
    }) {
        return None;
    }
    Some(format!("{} *", prefix.join(" ")))
}

fn pattern_word_count(first: &str, words: &[String]) -> Option<usize> {
    const SIMPLE: &[&str] = &[
        "cat", "head", "tail", "wc", "stat", "file", "ls", "tree", "du", "grep", "rg", "which",
        "where", "whoami", "pwd", "date", "uname", "df", "free", "ps", "echo",
    ];
    if SIMPLE.contains(&first) {
        return Some(1);
    }
    let second = words.get(1)?.to_ascii_lowercase();
    match first {
        "git" => matches!(
            second.as_str(),
            "status"
                | "diff"
                | "log"
                | "show"
                | "rev-parse"
                | "ls-files"
                | "grep"
                | "blame"
                | "shortlog"
                | "describe"
        )
        .then_some(2),
        "npm" | "pnpm" | "yarn" | "bun" => {
            if matches!(second.as_str(), "run" | "exec" | "dlx") {
                Some(3)
            } else if matches!(
                second.as_str(),
                "test" | "ls" | "list" | "view" | "info" | "outdated" | "why" | "audit"
            ) {
                Some(2)
            } else {
                None
            }
        }
        "cargo" => matches!(
            second.as_str(),
            "build" | "check" | "test" | "clippy" | "fmt" | "metadata" | "tree" | "bench"
        )
        .then_some(2),
        "go" => matches!(second.as_str(), "build" | "test" | "vet" | "list").then_some(2),
        "docker" => matches!(
            second.as_str(),
            "ps" | "images" | "logs" | "inspect" | "version" | "info"
        )
        .then_some(2),
        "kubectl" => matches!(
            second.as_str(),
            "get" | "describe" | "logs" | "version" | "explain"
        )
        .then_some(2),
        "terraform" => {
            if matches!(second.as_str(), "plan" | "show" | "validate" | "output") {
                Some(2)
            } else if second == "state" {
                Some(3)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn has_shell_control(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            ';' | '|'
                | '&'
                | '<'
                | '>'
                | '`'
                | '$'
                | '('
                | ')'
                | '{'
                | '}'
                | '\n'
                | '\r'
                | '*'
                | '?'
                | '['
                | ']'
        )
    })
}

fn command_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                } else if ch == '\\' && active == '"' {
                    escaped = true;
                } else {
                    current.push(ch);
                }
                started = true;
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    started = true;
                }
                '\\' => {
                    escaped = true;
                    started = true;
                }
                ch if ch.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(ch);
                    started = true;
                }
            },
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    if started {
        words.push(current);
    }
    Some(words)
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

    fn shell_pattern(command: &str) -> String {
        save_pattern(
            "builtin.bash",
            command,
            &serde_json::json!({"command": command}),
        )
    }

    #[test]
    fn shell_always_allow_uses_a_safe_prefix() {
        assert_eq!(shell_pattern("cat /logs/log_1.log"), "cat *");
        assert_eq!(shell_pattern("git status --short"), "git status *");
        assert_eq!(shell_pattern("npm run test --watch"), "npm run test *");
        assert_eq!(shell_pattern("cat \"a file.txt\""), "cat *");
        assert_eq!(shell_pattern("echo hello"), "echo *");
    }

    #[test]
    fn risky_or_unknown_commands_keep_the_exact_command() {
        for command in [
            "git reset --hard",
            "rm -rf /tmp/x",
            "cat a && rm b",
            "cat *.log",
            "mycli do-something",
            "cat",
        ] {
            assert_eq!(shell_pattern(command), command, "{command}");
        }
    }

    #[test]
    fn non_shell_permissions_keep_the_exact_resource() {
        assert_eq!(
            save_pattern(
                "builtin.edit",
                "src/app.ts",
                &serde_json::json!({"filePath": "src/app.ts"})
            ),
            "src/app.ts"
        );
    }
}
