use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncReadExt;

use crate::{CoreError, Result, new_id};

pub const MAX_ATTACHMENTS: usize = 6;
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 100_000;
pub const MAX_TOTAL_TEXT_CHARS: usize = 200_000;
const MAX_TEXT_BYTES: u64 = 520_000;
const META_FILE: &str = ".meta.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    /// `image`, `text`, or `file`.
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.kind == "image"
    }
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid(message)
}

fn safe_component(value: &str) -> Result<&str> {
    let valid = !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(value)
    } else {
        Err(invalid("Invalid attachment id."))
    }
}

fn safe_name(source: &Path) -> String {
    let raw = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    let name: String = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "file".into()
    } else {
        trimmed.chars().take(100).collect()
    };
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    let reserved = matches!(
        stem.as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    );
    if name == META_FILE || name == "meta" || reserved {
        format!("file-{name}")
    } else {
        name
    }
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(crate) fn image_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn file_mime(path: &Path) -> &'static str {
    match ext(path).as_str() {
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "exe" => "application/vnd.microsoft.portable-executable",
        "dll" => "application/vnd.microsoft.portable-executable",
        _ => "application/octet-stream",
    }
}

fn text_mime(path: &Path) -> &'static str {
    match ext(path).as_str() {
        "json" | "jsonc" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "js" | "mjs" | "cjs" => "text/javascript",
        _ => "text/plain",
    }
}

fn is_text_extension(path: &Path) -> bool {
    matches!(
        ext(path).as_str(),
        "txt"
            | "text"
            | "md"
            | "markdown"
            | "mdx"
            | "rst"
            | "adoc"
            | "log"
            | "csv"
            | "tsv"
            | "json"
            | "jsonc"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "properties"
            | "env"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "js"
            | "mjs"
            | "cjs"
            | "jsx"
            | "ts"
            | "tsx"
            | "mts"
            | "cts"
            | "vue"
            | "svelte"
            | "astro"
            | "py"
            | "rb"
            | "go"
            | "rs"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "cs"
            | "php"
            | "swift"
            | "lua"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hs"
            | "ml"
            | "scala"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "bat"
            | "cmd"
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
            | "tex"
            | "dockerfile"
            | "makefile"
            | "gradle"
            | "gitignore"
    )
}

fn looks_binary(head: &[u8]) -> bool {
    head.contains(&0)
}

async fn read_head(path: &Path) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)
        .await
        .map_err(|_| invalid(format!("Cannot open {}", path.display())))?;
    let mut head = vec![0_u8; 512];
    let read = file.read(&mut head).await?;
    head.truncate(read);
    Ok(head)
}

async fn extract_text(path: &Path, size: u64) -> Result<(Option<String>, bool)> {
    let file = fs::File::open(path).await?;
    let mut buf = Vec::new();
    file.take(MAX_TEXT_BYTES).read_to_end(&mut buf).await?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    let mut truncated = size > buf.len() as u64;
    if text.chars().count() > MAX_TEXT_CHARS {
        text = text.chars().take(MAX_TEXT_CHARS).collect();
        truncated = true;
    }
    Ok((Some(text), truncated))
}

async fn extract_pdf_text(path: &Path) -> Option<(String, bool)> {
    let bytes = fs::read(path).await.ok()?;
    let extracted = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text_from_mem(&bytes)
            }))
            .ok()
            .and_then(|result| result.ok())
        }),
    )
    .await
    .ok()?
    .ok()
    .flatten()?;
    let mut text = extracted.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    let truncated = text.chars().count() > MAX_TEXT_CHARS;
    if truncated {
        text = text.chars().take(MAX_TEXT_CHARS).collect();
    }
    Some((text, truncated))
}

fn check_text_budget(total: &mut usize, text: &Option<String>) -> Result<()> {
    let length = text
        .as_ref()
        .map(|value| value.chars().count())
        .unwrap_or(0);
    if total.saturating_add(length) > MAX_TOTAL_TEXT_CHARS {
        return Err(invalid(
            "Text attachments are too large to include at once.",
        ));
    }
    *total += length;
    Ok(())
}

/// Copy user-selected files into app-managed pending storage. Metadata is kept
/// next to the copy so the webview never has to send file contents through IPC.
pub async fn stage(root: &Path, paths: &[String]) -> Result<Vec<Attachment>> {
    if paths.is_empty() {
        return Err(invalid("No files selected."));
    }
    if paths.len() > MAX_ATTACHMENTS {
        return Err(invalid(format!(
            "Attach at most {MAX_ATTACHMENTS} files at once."
        )));
    }
    let pending = root.join("pending");
    fs::create_dir_all(&pending).await?;
    let mut staged = Vec::new();
    let mut total_size = 0_u64;
    let mut total_text = 0_usize;
    for raw in paths {
        let source = fs::canonicalize(raw)
            .await
            .map_err(|_| invalid(format!("File not found: {raw}")))?;
        let metadata = fs::metadata(&source)
            .await
            .map_err(|_| invalid(format!("Cannot read {}", source.display())))?;
        if !metadata.is_file() {
            return Err(invalid(format!("Not a file: {}", source.display())));
        }
        let size = metadata.len();
        if size > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "{} is larger than 25 MB.",
                safe_name(&source)
            )));
        }
        total_size = total_size.saturating_add(size);
        if total_size > MAX_TOTAL_BYTES {
            return Err(invalid("Attachments total more than 25 MB."));
        }
        let head = read_head(&source).await?;
        let image = image_mime(&head);
        if image.is_some() && size > MAX_IMAGE_BYTES {
            return Err(invalid(format!(
                "{} is larger than 5 MB.",
                safe_name(&source)
            )));
        }
        let text_kind = image.is_none() && (is_text_extension(&source) || !looks_binary(&head));
        let is_pdf = image.is_none() && ext(&source) == "pdf";
        let (mime, kind, text, truncated) = if let Some(mime) = image {
            (mime.to_owned(), "image", None, false)
        } else if is_pdf {
            match extract_pdf_text(&source).await {
                Some((text, truncated)) => {
                    let text = Some(text);
                    check_text_budget(&mut total_text, &text)?;
                    (file_mime(&source).to_owned(), "text", text, truncated)
                }
                None => (file_mime(&source).to_owned(), "file", None, false),
            }
        } else if text_kind {
            let (text, truncated) = extract_text(&source, size).await?;
            check_text_budget(&mut total_text, &text)?;
            (text_mime(&source).to_owned(), "text", text, truncated)
        } else {
            (file_mime(&source).to_owned(), "file", None, false)
        };
        let id = new_id();
        let dir = pending.join(&id);
        fs::create_dir_all(&dir).await?;
        let name = safe_name(&source);
        let path = dir.join(&name);
        if let Err(error) = fs::copy(&source, &path).await {
            let _ = fs::remove_dir_all(&dir).await;
            return Err(error.into());
        }
        let attachment = Attachment {
            id,
            name,
            mime,
            size: size.min(i64::MAX as u64) as i64,
            kind: kind.to_owned(),
            path: path.to_string_lossy().into_owned(),
            text,
            truncated,
        };
        if let Err(error) = write_meta(&dir, &attachment).await {
            let _ = fs::remove_dir_all(&dir).await;
            return Err(error);
        }
        staged.push(attachment);
    }
    Ok(staged)
}

/// Stage a clipboard image (data URL or raw base64) without a filesystem path.
/// Copy an image read by the agent into persistent attachment storage. Unlike
/// a composer pending attachment, this copy is not swept after 24 hours
/// because the tool result may be replayed from conversation history later.
pub async fn store_read_image(root: &Path, source: &Path) -> Result<Attachment> {
    let mut staged = stage(root, &[source.to_string_lossy().into_owned()]).await?;
    let mut attachment = staged.remove(0);
    let pending_dir = root.join("pending").join(&attachment.id);
    let images = root.join("read-images");
    fs::create_dir_all(&images).await?;
    let destination = images.join(&attachment.id);
    if let Err(error) = fs::rename(&pending_dir, &destination).await {
        let _ = fs::remove_dir_all(&pending_dir).await;
        return Err(error.into());
    }
    let _ = fs::remove_file(destination.join(META_FILE)).await;
    attachment.path = destination
        .join(&attachment.name)
        .to_string_lossy()
        .into_owned();
    Ok(attachment)
}

pub async fn stage_image(root: &Path, data: &str, name: Option<&str>) -> Result<Attachment> {
    let payload = data
        .split_once(',')
        .filter(|(prefix, _)| prefix.starts_with("data:"))
        .map(|(_, payload)| payload)
        .unwrap_or(data);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|_| invalid("Clipboard image is not valid base64."))?;
    stage_image_bytes(root, &bytes, name).await
}

pub async fn stage_image_bytes(
    root: &Path,
    bytes: &[u8],
    name: Option<&str>,
) -> Result<Attachment> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(invalid("Clipboard image is larger than 5 MB."));
    }
    let mime = image_mime(bytes).ok_or_else(|| {
        invalid("Clipboard data is not a supported PNG, JPEG, GIF or WebP image.")
    })?;
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    };
    let requested = name
        .and_then(|value| Path::new(value).file_stem().and_then(|stem| stem.to_str()))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("clipboard");
    let name = safe_name(Path::new(&format!("{requested}.{extension}")));
    let id = new_id();
    let dir = root.join("pending").join(&id);
    fs::create_dir_all(&dir).await?;
    let path = dir.join(&name);
    if let Err(error) = fs::write(&path, bytes).await {
        let _ = fs::remove_dir_all(&dir).await;
        return Err(error.into());
    }
    let attachment = Attachment {
        id,
        name,
        mime: mime.to_owned(),
        size: bytes.len() as i64,
        kind: "image".into(),
        path: path.to_string_lossy().into_owned(),
        text: None,
        truncated: false,
    };
    if let Err(error) = write_meta(&dir, &attachment).await {
        let _ = fs::remove_dir_all(&dir).await;
        return Err(error);
    }
    Ok(attachment)
}

async fn write_meta(dir: &Path, attachment: &Attachment) -> Result<()> {
    let json = serde_json::to_vec(attachment)?;
    fs::write(dir.join(META_FILE), json).await?;
    Ok(())
}

async fn read_meta(dir: &Path) -> Result<Attachment> {
    let raw = fs::read(dir.join(META_FILE)).await?;
    let attachment: Attachment = serde_json::from_slice(&raw)?;
    Ok(attachment)
}

pub async fn discard_pending(root: &Path, ids: &[String]) {
    for id in ids {
        let Ok(id) = safe_component(id) else {
            continue;
        };
        let _ = fs::remove_dir_all(root.join("pending").join(id)).await;
    }
}

pub async fn attach_to_message(
    root: &Path,
    conversation_id: &str,
    message_id: &str,
    ids: &[String],
) -> Result<Vec<Attachment>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    if ids.len() > MAX_ATTACHMENTS {
        return Err(invalid(format!(
            "Attach at most {MAX_ATTACHMENTS} files at once."
        )));
    }
    let conversation_id = safe_component(conversation_id)?;
    let message_id = safe_component(message_id)?;
    let pending = root.join("pending");
    let dest = root.join(conversation_id).join(message_id);
    fs::create_dir_all(&dest).await?;
    let mut unique = std::collections::HashSet::new();
    for id in ids {
        if !unique.insert(id.as_str()) {
            return Err(invalid("The same attachment was added twice."));
        }
    }

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut attachments = Vec::new();
    let mut total_size = 0_u64;
    let mut total_text = 0_usize;
    for id in ids {
        let id = safe_component(id)?;
        let source_dir = pending.join(id);
        let metadata = match read_meta(&source_dir).await {
            Ok(value) => value,
            Err(error) => {
                for (from, to) in moved.iter().rev() {
                    let _ = fs::rename(&to, &from).await;
                }
                return Err(error);
            }
        };
        let size = metadata.size.max(0) as u64;
        total_size = total_size.saturating_add(size);
        let text_length = metadata
            .text
            .as_ref()
            .map(|value| value.chars().count())
            .unwrap_or(0);
        total_text = total_text.saturating_add(text_length);
        let valid = total_size <= MAX_TOTAL_BYTES
            && total_text <= MAX_TOTAL_TEXT_CHARS
            && (metadata.kind != "image" || size <= MAX_IMAGE_BYTES);
        if !valid {
            for (from, to) in moved.iter().rev() {
                let _ = fs::rename(&to, &from).await;
            }
            return Err(invalid("Attachments are too large for one message."));
        }
        let destination = dest.join(id);
        if let Err(error) = fs::rename(&source_dir, &destination).await {
            for (from, to) in moved.iter().rev() {
                let _ = fs::rename(&to, &from).await;
            }
            return Err(error.into());
        }
        let final_path = destination.join(&metadata.name);
        let mut attachment = metadata;
        attachment.path = final_path.to_string_lossy().into_owned();
        let _ = fs::remove_file(destination.join(META_FILE)).await;
        moved.push((source_dir, destination));
        attachments.push(attachment);
    }
    Ok(attachments)
}

pub async fn delete_message_dir(root: &Path, conversation_id: &str, message_id: &str) {
    if let (Ok(conversation_id), Ok(message_id)) =
        (safe_component(conversation_id), safe_component(message_id))
    {
        let _ = fs::remove_dir_all(root.join(conversation_id).join(message_id)).await;
    }
}

pub async fn delete_conversation(root: &Path, conversation_id: &str) {
    if let Ok(conversation_id) = safe_component(conversation_id) {
        let _ = fs::remove_dir_all(root.join(conversation_id)).await;
    }
}

/// Pending copies are only useful while the composer is open. Remove abandoned
/// selections so an accidental file pick does not leave data forever.
pub async fn sweep_pending(root: &Path) {
    let pending = root.join("pending");
    let Ok(mut entries) = fs::read_dir(&pending).await else {
        return;
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if modified < cutoff {
            let _ = fs::remove_dir_all(path).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stages_text_files_and_moves_them_into_a_message() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("notes.txt");
        fs::write(&source, "hello attachment").await.unwrap();
        let staged = stage(
            &root.path().join("attachments"),
            &[source.to_string_lossy().into_owned()],
        )
        .await
        .unwrap();
        assert_eq!(staged[0].kind, "text");
        assert_eq!(staged[0].text.as_deref(), Some("hello attachment"));
        let attached = attach_to_message(
            &root.path().join("attachments"),
            "conversation",
            "message",
            &[staged[0].id.clone()],
        )
        .await
        .unwrap();
        assert_eq!(attached[0].name, "notes.txt");
        assert!(Path::new(&attached[0].path).is_file());
        assert!(
            !root
                .path()
                .join("attachments")
                .join("pending")
                .join(&staged[0].id)
                .exists()
        );
    }

    #[tokio::test]
    async fn extracts_text_from_pdf_attachments() {
        let root = tempfile::tempdir().unwrap();
        let pdf = root.path().join("hello.pdf");
        fs::write(&pdf, include_bytes!("../tests/fixtures/hello.pdf"))
            .await
            .unwrap();
        let staged = stage(
            &root.path().join("attachments"),
            &[pdf.to_string_lossy().into()],
        )
        .await
        .unwrap();
        assert_eq!(staged[0].kind, "text");
        assert_eq!(staged[0].mime, "application/pdf");
        assert!(
            staged[0]
                .text
                .as_deref()
                .unwrap_or("")
                .contains("Hello attachment PDF")
        );
    }

    #[tokio::test]
    async fn stages_clipboard_images_from_base64() {
        let root = tempfile::tempdir().unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0_u8; 32]);
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let attachment = stage_image(
            &root.path().join("attachments"),
            &format!("data:image/png;base64,{encoded}"),
            Some("pasted.png"),
        )
        .await
        .unwrap();
        assert_eq!(attachment.kind, "image");
        assert_eq!(attachment.mime, "image/png");
        assert_eq!(attachment.name, "pasted.png");
        assert!(Path::new(&attachment.path).is_file());
    }

    #[tokio::test]
    async fn recognizes_png_magic_and_rejects_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        let image = root.path().join("pixel.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0_u8; 32]);
        fs::write(&image, png).await.unwrap();
        let staged = stage(
            &root.path().join("attachments"),
            &[image.to_string_lossy().into()],
        )
        .await
        .unwrap();
        assert_eq!(staged[0].kind, "image");
        assert_eq!(staged[0].mime, "image/png");

        let too_big = root.path().join("big.txt");
        let handle = fs::File::create(&too_big).await.unwrap();
        handle.set_len(MAX_FILE_BYTES + 1).await.unwrap();
        let error = stage(
            &root.path().join("attachments2"),
            &[too_big.to_string_lossy().into()],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("larger than 25 MB"));
    }
}
