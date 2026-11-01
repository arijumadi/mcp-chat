use similar::TextDiff;

const MAX_DIFF_CHARS: usize = 48_000;

/// Render a compact unified diff between two text versions.
pub fn file_diff(path: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let diff = TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    cap(diff)
}

pub fn cap(text: String) -> String {
    if text.chars().count() <= MAX_DIFF_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(MAX_DIFF_CHARS).collect();
    out.push_str("\n… diff truncated …\n");
    out
}
