/// Incremental Server-Sent Events parser.
///
/// Handles CRLF or LF frame separators, comments, multi-line `data:` fields and
/// arbitrary UTF-8 chunk boundaries. It intentionally does not implement the
/// reconnection/id semantics; request-scoped streams in MCP and LLM APIs are
/// single-shot for this app.
#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((idx, sep_len)) = find_separator(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..idx + sep_len).collect();
            let body = &frame[..idx];
            if let Some(event) = parse_frame(body) {
                events.push(event);
            }
        }
        events
    }

    /// Returns any remaining frame at EOF. A stream that ends without a blank
    /// line still yields its last event in some servers.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.buffer.is_empty() {
            return None;
        }
        let body: Vec<u8> = std::mem::take(&mut self.buffer);
        parse_frame(&body)
    }
}

fn find_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i + 1 < buffer.len() {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

fn parse_frame(body: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(body);
    let mut event = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_owned()),
            "data" => data_lines.push(value),
            _ => {}
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data_lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_data_and_comments() {
        let mut parser = SseParser::new();
        let events = parser.push(b": keepalive\nevent: message\ndata: first\ndata: second\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "first\nsecond");
    }

    #[test]
    fn handles_split_chunks_and_crlf() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: {\"a\":").is_empty());
        let events = parser.push(b"1}\r\n\r\ndata: two\r\n\r\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[1].data, "two");
    }

    #[test]
    fn preserves_utf8_split_across_chunks() {
        let mut parser = SseParser::new();
        let full = "data: héllo ✓\n\n".as_bytes();
        let (a, b) = full.split_at(7);
        assert!(parser.push(a).is_empty());
        let events = parser.push(b);
        assert_eq!(events[0].data, "héllo ✓");
    }

    #[test]
    fn finish_flushes_unterminated_frame() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: tail").is_empty());
        assert_eq!(parser.finish().unwrap().data, "tail");
    }
}
