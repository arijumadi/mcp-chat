use crate::{CoreError, Result};

const CRLF: &str = "\r\n";

#[derive(Debug)]
pub struct EditOutcome {
    pub content: String,
    pub replacements: usize,
}

#[derive(Debug, Clone, Copy)]
struct Match {
    start: usize,
    end: usize,
}

#[derive(Debug)]
enum Replacement {
    Exact(String),
    Normalized(String),
}

pub fn apply(content: &str, old: &str, new: &str, replace_all: bool) -> Result<EditOutcome> {
    if old.is_empty() {
        return Err(CoreError::invalid(
            "oldString must not be empty. Use write to create or overwrite a file.",
        ));
    }
    if old == new {
        return Err(CoreError::invalid(
            "No changes to apply: oldString and newString are identical.",
        ));
    }

    // Strings that differ only by \r ask to change line endings themselves.
    // Every other edit is matched against the LF view that read shows the model.
    let endings_only = old.replace('\r', "") == new.replace('\r', "");
    let (matches, replacement) = if endings_only {
        (
            find_occurrences(content, old),
            Replacement::Exact(new.to_owned()),
        )
    } else {
        let view = content.replace(CRLF, "\n");
        let search = old.replace(CRLF, "\n");
        let replacement = new.replace(CRLF, "\n");
        let view_matches = find_view_matches(&view, &search);
        let removed = removed_carriage_returns(content);
        let matches = view_matches
            .into_iter()
            .map(|item| Match {
                start: to_source_offset(&removed, item.start),
                end: to_source_offset(&removed, item.end),
            })
            .collect();
        (matches, Replacement::Normalized(replacement))
    };

    if matches.is_empty() {
        return Err(CoreError::invalid(
            "oldString was not found. It must match exactly, including whitespace and indentation.",
        ));
    }
    if matches.len() > 1 && !replace_all {
        return Err(CoreError::invalid(format!(
            "oldString occurs {} times; set replaceAll to true or use a more specific string.",
            matches.len()
        )));
    }

    let selected: Vec<Match> = if replace_all {
        matches.clone()
    } else {
        matches[..1].to_vec()
    };
    let mut updated = content.to_owned();
    for item in selected.iter().rev() {
        let value = match &replacement {
            Replacement::Exact(value) => value.clone(),
            Replacement::Normalized(value) => {
                let ending = region_line_ending(content, item.start, item.end);
                value.replace('\n', ending)
            }
        };
        updated.replace_range(item.start..item.end, &value);
    }
    if updated == content {
        return Err(CoreError::invalid(
            "No changes to apply: the edit leaves the file unchanged.",
        ));
    }
    Ok(EditOutcome {
        content: updated,
        replacements: selected.len(),
    })
}

fn find_occurrences(content: &str, search: &str) -> Vec<Match> {
    if search.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = 0;
    while let Some(found) = content[offset..].find(search) {
        let start = offset + found;
        result.push(Match {
            start,
            end: start + search.len(),
        });
        offset = start + search.len();
    }
    result
}

fn find_view_matches(view: &str, search: &str) -> Vec<Match> {
    let exact = find_occurrences(view, search);
    if !exact.is_empty() {
        return exact;
    }
    let normalized = find_normalized_occurrences(view, search);
    if !normalized.is_empty() {
        return normalized;
    }
    find_line_occurrences(view, search)
}

/// Normalize typographic punctuation and Unicode spaces that models often
/// reproduce differently from the file. Every replacement is one-to-one, so
/// character positions still map back to the source.
fn normalize_char(value: char) -> char {
    match value {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        '\u{00a0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
        | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200a}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' => ' ',
        _ => value,
    }
}

pub(crate) fn normalize_for_match(value: &str) -> String {
    value.chars().map(normalize_char).collect()
}

fn find_normalized_occurrences(view: &str, search: &str) -> Vec<Match> {
    let pattern: Vec<char> = search.chars().map(normalize_char).collect();
    if pattern.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = view.char_indices().collect();
    let normalized: Vec<char> = chars
        .iter()
        .map(|(_, value)| normalize_char(*value))
        .collect();
    let mut result = Vec::new();
    let mut offset = 0;
    while offset + pattern.len() <= normalized.len() {
        if normalized[offset..offset + pattern.len()] == pattern[..] {
            let start = chars[offset].0;
            let last = &chars[offset + pattern.len() - 1];
            result.push(Match {
                start,
                end: last.0 + last.1.len_utf8(),
            });
            offset += pattern.len();
        } else {
            offset += 1;
        }
    }
    result
}

#[derive(Debug)]
struct ViewLine<'a> {
    start: usize,
    end: usize,
    content_end: usize,
    text: &'a str,
    newline: bool,
}

fn find_line_occurrences(view: &str, search: &str) -> Vec<Match> {
    let trailing_newline = search.ends_with('\n');
    let mut expected: Vec<&str> = search.split('\n').collect();
    if trailing_newline {
        expected.pop();
    }
    if expected.is_empty() {
        return Vec::new();
    }
    let lines = view_lines(view);
    if expected.len() > lines.len() {
        return Vec::new();
    }
    let key = |value: &str| normalize_for_match(value.trim_end());
    let wanted: Vec<String> = expected.iter().map(|line| key(line)).collect();
    let mut candidates = Vec::new();
    for start in 0..=lines.len() - expected.len() {
        let last = &lines[start + expected.len() - 1];
        if trailing_newline && !last.newline {
            continue;
        }
        let matches = lines[start..start + expected.len()]
            .iter()
            .zip(wanted.iter())
            .all(|(line, wanted)| key(line.text) == *wanted);
        if !matches {
            continue;
        }
        candidates.push(Match {
            start: lines[start].start,
            end: if trailing_newline {
                last.end
            } else {
                last.content_end
            },
        });
    }
    candidates.dedup_by(|left, right| left.start < right.end && right.start < left.end);
    candidates
}

fn view_lines(view: &str) -> Vec<ViewLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for part in view.split_inclusive('\n') {
        let newline = part.ends_with('\n');
        let content_end = start + part.len() - usize::from(newline);
        lines.push(ViewLine {
            start,
            end: start + part.len(),
            content_end,
            text: &view[start..content_end],
            newline,
        });
        start += part.len();
    }
    if view.is_empty() {
        // An empty file still has one conceptual line, but a non-empty search
        // cannot match it.
    }
    lines
}

fn removed_carriage_returns(content: &str) -> Vec<usize> {
    let mut result = Vec::new();
    let bytes = content.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'\r' && bytes[index + 1] == b'\n' {
            result.push(index);
            index += 2;
        } else {
            index += 1;
        }
    }
    result
}

fn to_source_offset(removed: &[usize], offset: usize) -> usize {
    offset + removed.partition_point(|position| *position < offset)
}

fn region_line_ending(content: &str, start: usize, end: usize) -> &'static str {
    let region = &content[start..end];
    let crlf = region.matches(CRLF).count();
    let lf = region.matches('\n').count() - crlf;
    if crlf > lf {
        CRLF
    } else if lf > 0 {
        "\n"
    } else {
        dominant_line_ending(content)
    }
}

fn dominant_line_ending(content: &str) -> &'static str {
    let crlf = content.matches(CRLF).count();
    let lf = content.matches('\n').count() - crlf;
    if crlf > lf { CRLF } else { "\n" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_edit_works() {
        let result = apply("hello world\n", "world", "there", false).unwrap();
        assert_eq!(result.content, "hello there\n");
        assert_eq!(result.replacements, 1);
    }

    #[test]
    fn crlf_file_matches_lf_text_and_preserves_crlf() {
        let result = apply("one\r\ntwo\r\nthree\r\n", "two\nthree", "TWO\nTHREE", false).unwrap();
        assert_eq!(result.content, "one\r\nTWO\r\nTHREE\r\n");
    }

    #[test]
    fn can_change_line_endings_explicitly() {
        let result = apply("one\r\ntwo\r\n", "one\r\n", "one\n", false).unwrap();
        assert_eq!(result.content, "one\ntwo\r\n");
    }

    #[test]
    fn normalizes_typographic_characters() {
        let result = apply(
            "say ‘hello’ — now",
            "say 'hello' - now",
            "say 'hi' - later",
            false,
        )
        .unwrap();
        assert_eq!(result.content, "say 'hi' - later");
    }

    #[test]
    fn ignores_trailing_whitespace_per_line() {
        let result = apply("one   \ntwo\n", "one\ntwo", "ONE\nTWO", false).unwrap();
        assert_eq!(result.content, "ONE\nTWO\n");
    }

    #[test]
    fn replace_all_and_ambiguity() {
        let result = apply("same same same", "same", "done", true).unwrap();
        assert_eq!(result.content, "done done done");
        assert_eq!(result.replacements, 3);
        assert!(apply("same same", "same", "done", false).is_err());
    }

    #[test]
    fn rejects_identical_and_missing() {
        assert!(apply("hello", "hello", "hello", false).is_err());
        assert!(apply("hello", "missing", "there", false).is_err());
    }
}
