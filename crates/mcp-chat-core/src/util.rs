pub fn truncate_chars(text: &str, limit: usize) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text.to_owned(), false);
    }
    let mut out = text.chars().take(limit).collect::<String>();
    out.push_str("\n[truncated]");
    (out, true)
}

pub fn sanitize_tool_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("tool");
    }
    out.truncate(64);
    out
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "server".into()
    } else {
        trimmed.chars().take(40).collect()
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    // Deliberately conservative: no tokenizer dependency, and most providers
    // average close to four characters per token for English/code.
    (text.chars().count() / 3).max(1)
}

pub fn redact_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "••••".into();
    }
    let prefix: String = chars.iter().take(3).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(3)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}••••{suffix}")
}

/// A compact, minute-precision local-time note. It is appended at the END of
/// the system prompt so the static system prompt, tools and history remain a
/// stable prefix for provider prompt caching.
pub fn current_time_context() -> String {
    let now = chrono::Local::now();
    format!(
        "Current local date and time: {}. UTC offset: {}. Use this as the current moment when the user asks about the day, date, or time; do not assume an outdated date.",
        now.format("%A, %d %B %Y, %H:%M"),
        now.offset()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_context_contains_local_date_and_time() {
        let note = current_time_context();
        assert!(note.contains("Current local date and time:"));
        assert!(note.contains("UTC offset:"));
    }
}
