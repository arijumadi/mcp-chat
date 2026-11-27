//! Single-user authentication.
//!
//! The desktop app has no authentication because the only way to reach it is to
//! already be at the keyboard. A listening socket has no such protection: this
//! process can run `bash` and write files, so an unauthenticated port is a
//! remote shell.
//!
//! One token, compared in constant time, carried by an HttpOnly cookie so that
//! `EventSource` and `<img>` work without custom headers. `Authorization:
//! Bearer` is accepted as well, for curl.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

use crate::state::AppState;

const COOKIE: &str = "jarvis_session";

pub struct Auth {
    token: String,
    /// Set when the cookie should carry `Secure`. Off by default because the
    /// intended deployment is a loopback bind behind an SSH tunnel, where an
    /// HTTPS-only cookie would never come back.
    secure_cookie: bool,
    /// Accepted `Host` values. Empty means "accept anything", which is the
    /// right default on a loopback bind where only this machine can reach us.
    allowed_hosts: Vec<String>,
}

impl Auth {
    pub fn new(token: String, secure_cookie: bool, allowed_hosts: Vec<String>) -> Self {
        Self {
            token,
            secure_cookie,
            allowed_hosts,
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn matches(&self, candidate: &str) -> bool {
        // Constant-time comparison. The token is short and an attacker cannot
        // observe server time precisely through a browser, but this costs
        // nothing and removes the question.
        let left = self.token.as_bytes();
        let right = candidate.as_bytes();
        if left.len() != right.len() {
            return false;
        }
        let mut difference = 0u8;
        for (a, b) in left.iter().zip(right.iter()) {
            difference |= a ^ b;
        }
        difference == 0
    }

    /// True when the request carries a valid credential.
    pub fn allows_headers(&self, headers: &HeaderMap) -> bool {
        if let Some(value) = headers.get(header::AUTHORIZATION) {
            if let Ok(text) = value.to_str() {
                if let Some(token) = text.strip_prefix("Bearer ") {
                    return self.matches(token.trim());
                }
            }
        }
        cookie_token(headers)
            .as_deref()
            .is_some_and(|token| self.matches(token))
    }

    fn set_cookie(&self, headers: &mut HeaderMap) {
        let mut cookie = format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000",
            self.token
        );
        if self.secure_cookie {
            cookie.push_str("; Secure");
        }
        if let Ok(value) = header::HeaderValue::from_str(&cookie) {
            headers.append(header::SET_COOKIE, value);
        }
    }

    /// `Host` checking is a cheap defense against DNS rebinding: a page on
    /// `evil.example` that resolves to 127.0.0.1 still sends its own Host.
    ///
    /// More than one name is normal: a Tailscale host answers to its IP, its
    /// MagicDNS name, and sometimes a short hostname.
    pub fn host_allowed(&self, headers: &HeaderMap) -> bool {
        if self.allowed_hosts.is_empty() {
            return true;
        }
        let Some(host) = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let host = host.split(':').next().unwrap_or(host);
        self.allowed_hosts
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed))
    }
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{COOKIE}=")) {
            return Some(value.to_owned());
        }
    }
    None
}

/// Reject anything that does not carry the token.
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !state.auth.host_allowed(request.headers()) {
        return (StatusCode::FORBIDDEN, "Unknown host").into_response();
    }
    if state.auth.allows_headers(request.headers()) {
        return next.run(request).await;
    }
    let path = request.uri().path();
    if path.starts_with("/login/") {
        return next.run(request).await;
    }
    if path.starts_with("/api/") {
        return (StatusCode::UNAUTHORIZED, "Not signed in").into_response();
    }
    redirect_to_login()
}

pub async fn login_page(State(state): State<Arc<AppState>>, request: Request<Body>) -> Response {
    if state.auth.allows_headers(request.headers()) {
        return redirect_home();
    }
    (StatusCode::OK, Html(LOGIN_HTML)).into_response()
}

pub async fn login(State(state): State<Arc<AppState>>, body: String) -> Response {
    let token = body
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "token")
        .map(|(_, value)| urldecode(value))
        .unwrap_or_default();
    if !state.auth.matches(token.trim()) {
        return (
            StatusCode::UNAUTHORIZED,
            Html(LOGIN_HTML.replace(
                "<!--error-->",
                "<p class=\"error\">That token is not valid.</p>",
            )),
        )
            .into_response();
    }
    let mut response = redirect_home();
    state.auth.set_cookie(response.headers_mut());
    response
}

fn redirect_to_login() -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, "/login/")]).into_response()
}

fn redirect_home() -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response()
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Print the URL a browser should open, once, at startup.
pub fn announce(address: SocketAddr, token: &str, loopback: bool) {
    if loopback {
        tracing::info!("Jarvis is listening on http://{address}");
        tracing::info!("Open http://{address}/login/ and paste this token:");
    } else {
        tracing::warn!("Jarvis is listening on http://{address} (not loopback)");
        tracing::warn!("Anyone who can reach this port and has the token can run commands here.");
        tracing::info!("Open http://{address}/login/ and paste this token:");
    }
    tracing::info!("{token}");
}

const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Jarvis</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #0b0c0f; color: #e7e9ee;
         font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif; }
  form { width: min(92vw, 340px); display: grid; gap: 12px; }
  h1 { margin: 0 0 4px; font-size: 20px; font-weight: 600; letter-spacing: -0.01em; }
  p { margin: 0; color: #9aa0ac; font-size: 13px; }
  p.error { color: #ff8b8b; }
  input { width: 100%; box-sizing: border-box; padding: 10px 12px; border-radius: 8px;
          border: 1px solid #2a2d35; background: #14161b; color: inherit; font: inherit; }
  input:focus { outline: 2px solid #3b82f6; outline-offset: 1px; border-color: transparent; }
  button { padding: 10px 12px; border-radius: 8px; border: 0; background: #3b82f6;
           color: #fff; font: inherit; font-weight: 600; cursor: pointer; }
  button:hover { background: #2f6fd8; }
</style></head>
<body><form method="post" action="/login/">
  <!--error-->
  <h1>Jarvis</h1>
  <p>Paste the token printed by the server.</p>
  <input name="token" type="password" autofocus autocomplete="current-password"
         placeholder="Access token" required>
  <button type="submit">Sign in</button>
</form></body></html>"#;
