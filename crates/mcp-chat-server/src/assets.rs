//! Static files for the web UI.
//!
//! The same `dist/` bundle the desktop app loads is served here. Unknown paths
//! without a file extension fall back to `index.html`, so a deep link still
//! boots the app.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use crate::state::AppState;

pub async fn serve(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let root = &state.static_dir;
    let requested = uri.path().trim_start_matches('/');

    if requested.is_empty() {
        return index(root);
    }

    // Reject traversal before touching the filesystem. `canonicalize` below
    // also checks, but refusing a `..` outright keeps the intent obvious.
    let candidate = Path::new(requested);
    if candidate
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let path = root.join(candidate);
    match resolve(root, &path).await {
        Some(file) => file_response(&file).await,
        None => {
            // A path with no extension is an app route, not a missing asset.
            if candidate.extension().is_none() {
                index(root)
            } else {
                (StatusCode::NOT_FOUND, "Not found").into_response()
            }
        }
    }
}

async fn resolve(root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = tokio::fs::canonicalize(path).await.ok()?;
    let root = tokio::fs::canonicalize(root).await.ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    if tokio::fs::metadata(&canonical).await.ok()?.is_file() {
        Some(canonical)
    } else {
        None
    }
}

fn index(root: &Path) -> Response {
    let path = root.join("index.html");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/html; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            );
            response
        }
        Err(error) => {
            tracing::error!(path = %path.display(), %error, "static bundle is missing");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "The web UI bundle is missing. Point JARVIS_STATIC_DIR at the Vite `dist/` \
                     directory (looked in {}).",
                    path.display()
                ),
            )
                .into_response()
        }
    }
}

async fn file_response(path: &Path) -> Response {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let mime = content_type(path);
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(mime));
    // Vite writes content-hashed names under `assets/`, so those can be cached
    // forever; everything else stays revalidated.
    let cache = if path.to_string_lossy().contains("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache),
    );
    response
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "map" => "application/json",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
