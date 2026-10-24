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

/// A compact, minute-precision local-time note derived from a persisted
/// message timestamp. It is injected into the user message (not the system
/// prompt) so the static system/tools prefix stays byte-identical. Deriving
/// the note from `created_at` also keeps historical messages byte-identical
/// across turns, which is required for prefix-cache hits.
pub fn message_time_note(created_at_ms: i64) -> String {
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(created_at_ms)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    let local = timestamp.with_timezone(&chrono::Local);
    format!(
        "[Message sent at: {} (UTC{})]",
        local.format("%A, %d %B %Y, %H:%M"),
        local.offset()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_note_is_stable_for_a_message_timestamp() {
        let note = message_time_note(1_700_000_000_000);
        assert!(note.starts_with("[Message sent at: "));
        assert!(note.contains("UTC"));
        assert_eq!(note, message_time_note(1_700_000_000_000));
    }
}
