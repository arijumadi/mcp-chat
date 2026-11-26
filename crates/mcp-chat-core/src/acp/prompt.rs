//! Turning a Jarvis user message into ACP prompt content blocks.
//!
//! Kept separate from the turn runner so the mapping can be unit-tested without
//! a process. The rule that matters: never send a block the agent did not
//! advertise during `initialize`, because an unsupported block is a protocol
//! error, not a graceful degradation.

use agent_client_protocol::v1::schema::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent, ResourceLink,
    TextContent, TextResourceContents,
};

use crate::models::{MessageContext, Part};

/// What the agent said it accepts in a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCapabilities {
    pub images: bool,
    pub embedded_context: bool,
}

impl PromptCapabilities {
    /// Text and resource links are mandatory for every agent.
    pub fn minimal() -> Self {
        Self {
            images: false,
            embedded_context: false,
        }
    }
}

/// A file the user attached, already reduced to what a content block needs.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentInput {
    Image {
        mime: String,
        /// Base64 payload, as ACP requires.
        data: String,
        uri: Option<String>,
    },
    /// Text the app already extracted; no file access is needed at send time.
    Text {
        name: String,
        mime: String,
        text: String,
        uri: String,
    },
    /// A file the agent should open itself.
    Link {
        name: String,
        mime: Option<String>,
        size: Option<i64>,
        uri: String,
    },
}

/// Build the prompt for one turn.
///
/// Order matters for readability: the user's words first, then each attachment,
/// then file contexts. An attachment that the agent cannot accept becomes a
/// short text note instead of being dropped silently, so the agent at least
/// knows something was attached and the user sees why it was not delivered.
pub fn build_prompt(
    text: &str,
    attachments: &[AttachmentInput],
    contexts: &[MessageContext],
    caps: PromptCapabilities,
) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(text.to_owned())));
    }
    for attachment in attachments {
        blocks.push(block_for(attachment, caps));
    }
    for context in contexts {
        blocks.push(context_block(context, caps));
    }
    blocks
}

fn block_for(attachment: &AttachmentInput, caps: PromptCapabilities) -> ContentBlock {
    match attachment {
        AttachmentInput::Image { mime, data, uri } => {
            if caps.images {
                let mut image = ImageContent::new(data.clone(), mime.clone());
                image.uri = uri.clone();
                ContentBlock::Image(image)
            } else {
                unsupported_note(
                    uri.as_deref().unwrap_or("the attached image"),
                    "this agent does not accept images",
                )
            }
        }
        AttachmentInput::Text {
            name,
            mime,
            text,
            uri,
        } => {
            if caps.embedded_context {
                ContentBlock::Resource(EmbeddedResource::new(
                    EmbeddedResourceResource::TextResourceContents(
                        TextResourceContents::new(text.clone(), uri.clone())
                            .mime_type(mime.clone()),
                    ),
                ))
            } else {
                // Resource links are mandatory, so the agent can still fetch it.
                ContentBlock::ResourceLink(link(name, Some(mime.clone()), None, uri.clone()))
            }
        }
        AttachmentInput::Link {
            name,
            mime,
            size,
            uri,
        } => ContentBlock::ResourceLink(link(name, mime.clone(), *size, uri.clone())),
    }
}

fn context_block(context: &MessageContext, caps: PromptCapabilities) -> ContentBlock {
    // A context is a file plus the user's comment about it.
    let label = match (context.start_line, context.end_line) {
        (Some(start), Some(end)) => format!("{} (lines {start}-{end})", context.path),
        (Some(start), None) => format!("{} (line {start})", context.path),
        _ => context.path.clone(),
    };
    let mut body = format!("{label}\n{}", context.comment.trim());
    if let Some(preview) = context
        .preview
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        body.push_str("\n\n");
        body.push_str(preview);
    }
    let uri = format!("file://{}", context.path.replace('\\', "/"));
    if caps.embedded_context {
        ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(
                TextResourceContents::new(body, uri).mime_type("text/plain"),
            ),
        ))
    } else {
        // Without embedded context, the comment still has to reach the agent.
        ContentBlock::Text(TextContent::new(body))
    }
}

fn link(name: &str, mime: Option<String>, size: Option<i64>, uri: String) -> ResourceLink {
    let mut link = ResourceLink::new(name.to_owned(), uri);
    link.mime_type = mime;
    link.size = size;
    link
}

fn unsupported_note(uri: &str, reason: &str) -> ContentBlock {
    ContentBlock::Text(TextContent::new(format!(
        "[Attachment {uri} was not sent: {reason}.]"
    )))
}

/// Reduce a stored attachment part into a sendable input.
///
/// `read` is injected so this stays testable without touching the filesystem.
pub fn attachment_input(
    part: &Part,
    read_base64: &dyn Fn(&str) -> Option<String>,
) -> Option<AttachmentInput> {
    let payload = part.payload();
    let kind = payload.get("kind")?.as_str()?;
    let path = payload.get("path")?.as_str()?.to_owned();
    let uri = format!("file://{}", path.replace('\\', "/"));
    let name = payload
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("attachment")
        .to_owned();
    let mime = payload
        .get("mime")
        .and_then(|value| value.as_str())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let size = payload.get("size").and_then(|value| value.as_i64());
    match kind {
        "image" => Some(AttachmentInput::Image {
            mime,
            data: read_base64(&path)?,
            uri: Some(uri),
        }),
        "text" => Some(AttachmentInput::Text {
            name,
            mime,
            text: payload
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            uri,
        }),
        _ => Some(AttachmentInput::Link {
            name,
            mime: Some(mime),
            size,
            uri,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> AttachmentInput {
        AttachmentInput::Image {
            mime: "image/png".into(),
            data: "aGVsbG8=".into(),
            uri: Some("file:///tmp/a.png".into()),
        }
    }

    #[test]
    fn text_only_prompt_has_one_block() {
        let blocks = build_prompt("hello", &[], &[], PromptCapabilities::minimal());
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text(text) if text.text == "hello"));
    }

    #[test]
    fn empty_text_with_an_attachment_still_sends_the_attachment() {
        let blocks = build_prompt(
            "",
            &[image()],
            &[],
            PromptCapabilities {
                images: true,
                embedded_context: false,
            },
        );
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], ContentBlock::Image(_)));
    }

    #[test]
    fn an_image_is_never_sent_to_an_agent_without_image_support() {
        let blocks = build_prompt("look", &[image()], &[], PromptCapabilities::minimal());
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlock::Text(_)));
        match &blocks[1] {
            ContentBlock::Text(text) => {
                assert!(text.text.contains("not sent"), "{}", text.text);
                assert!(
                    text.text.contains("does not accept images"),
                    "{}",
                    text.text
                );
            }
            other => panic!("expected a note, got {other:?}"),
        }
    }

    #[test]
    fn extracted_text_is_embedded_when_supported_and_linked_otherwise() {
        let attachment = AttachmentInput::Text {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            text: "body".into(),
            uri: "file:///tmp/notes.txt".into(),
        };
        let embedded = build_prompt(
            "",
            std::slice::from_ref(&attachment),
            &[],
            PromptCapabilities {
                images: false,
                embedded_context: true,
            },
        );
        assert!(matches!(embedded[0], ContentBlock::Resource(_)));
        let linked = build_prompt("", &[attachment], &[], PromptCapabilities::minimal());
        assert!(matches!(linked[0], ContentBlock::ResourceLink(_)));
    }

    #[test]
    fn a_context_keeps_the_comment_and_the_line_range() {
        let context = MessageContext {
            path: "src/main.rs".into(),
            start_line: Some(3),
            end_line: Some(9),
            comment: "explain this".into(),
            preview: Some("let x = 1;".into()),
        };
        let blocks = build_prompt(
            "look",
            &[],
            &[context],
            PromptCapabilities {
                images: false,
                embedded_context: true,
            },
        );
        match &blocks[1] {
            ContentBlock::Resource(resource) => match &resource.resource {
                EmbeddedResourceResource::TextResourceContents(text) => {
                    assert!(
                        text.text.contains("src/main.rs (lines 3-9)"),
                        "{}",
                        text.text
                    );
                    assert!(text.text.contains("explain this"), "{}", text.text);
                    assert!(text.text.contains("let x = 1;"), "{}", text.text);
                }
                other => panic!("expected text contents, got {other:?}"),
            },
            other => panic!("expected a resource, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_attachment_kind_becomes_a_link_not_an_error() {
        let part = Part {
            id: "p".into(),
            message_id: "m".into(),
            idx: 0,
            kind: "attachment".into(),
            payload_json: serde_json::json!({
                "kind": "file",
                "name": "doc.pdf",
                "mime": "application/pdf",
                "size": 10,
                "path": "/tmp/doc.pdf",
            })
            .to_string(),
        };
        let input = attachment_input(&part, &|_| None).expect("a link is always possible");
        assert!(matches!(input, AttachmentInput::Link { .. }));
    }

    #[test]
    fn an_unreadable_image_is_skipped_rather_than_sent_empty() {
        let part = Part {
            id: "p".into(),
            message_id: "m".into(),
            idx: 0,
            kind: "attachment".into(),
            payload_json: serde_json::json!({
                "kind": "image",
                "name": "a.png",
                "mime": "image/png",
                "path": "/tmp/a.png",
            })
            .to_string(),
        };
        assert!(attachment_input(&part, &|_| None).is_none());
    }
}
