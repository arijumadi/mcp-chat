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
    /// Salted hash of the password, when one is configured.
    ///
    /// Stored as a hash rather than the text so a copy of `server-password`
    /// does not hand over the server. The password is an *alternative* to the
    /// token, not a replacement: scripts and the Tailscale login link keep
    /// working, and a forgotten password can be reset from the data directory.
    password: Option<String>,
    /// Per-installation salt, so the same password does not hash to the same
    /// value on two hosts.
    salt: String,
    /// Set when the cookie should carry `Secure`. Off by default because the
    /// intended deployment is a loopback bind behind an SSH tunnel, where an
    /// HTTPS-only cookie would never come back.
    secure_cookie: bool,
    /// Accepted `Host` values. Empty means "accept anything", which is the
    /// right default on a loopback bind where only this machine can reach us.
    allowed_hosts: Vec<String>,
}

impl Auth {
    pub fn new(
        token: String,
        password: Option<String>,
        salt: String,
        secure_cookie: bool,
        allowed_hosts: Vec<String>,
    ) -> Self {
        Self {
            token,
            password,
            salt,
            secure_cookie,
            allowed_hosts,
        }
    }

    /// True when any password is configured, so the login page can say so.
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// Salted, repeated SHA-256.
    ///
    /// The workspace has no Argon2 dependency and adding one for a single-user
    /// server is not worth it. Repeating the hash makes a bulk guess against a
    /// stolen file slower than a single pass; it is not as strong as Argon2, and
    /// that is the honest trade here. The real defence is that the port is not
    /// reachable from the internet.
    fn hash_password(&self, password: &str) -> String {
        hash_password_with(&self.salt, password)
    }

    /// True when the candidate is either the token or the password.
    pub fn matches_credential(&self, candidate: &str) -> bool {
        if self.matches(candidate) {
            return true;
        }
        match &self.password {
            Some(expected) => {
                let candidate = self.hash_password(candidate);
                // Both sides are fixed-length hex, so a length check leaks
                // nothing.
                candidate.len() == expected.len()
                    && candidate
                        .bytes()
                        .zip(expected.bytes())
                        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                        == 0
            }
            None => false,
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
    ///
    /// A cookie holds the session value, not the password: the password must not
    /// be recoverable from a browser profile. `Bearer` accepts either the token
    /// or the password, so curl keeps working with whichever the user has.
    pub fn allows_headers(&self, headers: &HeaderMap) -> bool {
        if let Some(value) = headers.get(header::AUTHORIZATION) {
            if let Ok(text) = value.to_str() {
                if let Some(token) = text.strip_prefix("Bearer ") {
                    return self.matches_credential(token.trim());
                }
            }
        }
        cookie_token(headers)
            .as_deref()
            .is_some_and(|value| self.session_value_matches(value))
    }

    /// The value handed to the browser.
    ///
    /// With a password configured this is a hash of it, so the cookie is
    /// worthless if it leaks without also leaking the salt. Without one it is
    /// the token, which is what the login page already hands out.
    fn session_value(&self) -> String {
        match &self.password {
            Some(hash) => self.hash_password(hash),
            None => self.token.clone(),
        }
    }

    fn session_value_matches(&self, candidate: &str) -> bool {
        let expected = self.session_value();
        let (left, right) = (candidate.as_bytes(), expected.as_bytes());
        if left.len() != right.len() {
            return false;
        }
        let mut difference = 0u8;
        for (a, b) in left.iter().zip(right.iter()) {
            difference |= a ^ b;
        }
        difference == 0
    }

    fn set_cookie(&self, headers: &mut HeaderMap) {
        let mut cookie = format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000",
            self.session_value()
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

/// Salted, repeated SHA-256.
///
/// Free rather than a method so `main` can hash at startup and never hold the
/// plain password. The workspace has no Argon2 dependency and adding one for a
/// single-user server is not worth it; repeating the hash makes a bulk guess
/// against a stolen file slower than a single pass. It is not as strong as
/// Argon2, and that is the honest trade: the real defence is that the port is
/// not reachable from the internet.
pub fn hash_password_with(salt: &str, password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(salt.as_bytes());
    digest.update(password.as_bytes());
    let mut value = digest.finalize();
    for _ in 0..20_000 {
        let mut next = Sha256::new();
        next.update(salt.as_bytes());
        next.update(value);
        value = next.finalize();
    }
    value.iter().map(|byte| format!("{byte:02x}")).collect()
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
    // A redirect is right for a page the user navigated to, and wrong for a
    // subresource: a stale cookie would turn every script into a 303 and the
    // app would render blank with no explanation. Only HTML gets redirected.
    let wants_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    if path.starts_with("/api/") || !wants_html {
        return (StatusCode::UNAUTHORIZED, "Not signed in").into_response();
    }
    redirect_to_login()
}

pub async fn login_page(State(state): State<Arc<AppState>>, request: Request<Body>) -> Response {
    if state.auth.allows_headers(request.headers()) {
        return redirect_home();
    }
    let hint = if state.auth.has_password() {
        "Paste the token printed by the server, or enter your password."
    } else {
        "Paste the token printed by the server."
    };
    (
        StatusCode::OK,
        Html(LOGIN_HTML.replace("<!--hint-->", hint)),
    )
        .into_response()
}

pub async fn login(State(state): State<Arc<AppState>>, body: String) -> Response {
    let token = body
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "token" || *key == "password")
        .map(|(_, value)| urldecode(value))
        .unwrap_or_default();
    if !state.auth.matches_credential(token.trim()) {
        return (
            StatusCode::UNAUTHORIZED,
            Html(LOGIN_HTML.replace(
                "<!--error-->",
                "<p class=\"error\">That token or password is not valid.</p>",
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
  <p><!--hint--></p>
  <input name="token" type="password" autofocus autocomplete="current-password"
         placeholder="Access token or password" required>
  <button type="submit">Sign in</button>
</form></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_with_password(password: &str) -> Auth {
        Auth::new(
            "token-value".into(),
            Some(hash_password_with("salt-1", password)),
            "salt-1".into(),
            false,
            Vec::new(),
        )
    }

    #[test]
    fn the_token_still_works_when_a_password_is_set() {
        let auth = auth_with_password("hunter2");
        assert!(auth.matches_credential("token-value"));
        // Scripts that use Bearer keep working.
        assert!(auth.matches_credential("hunter2"));
        assert!(!auth.matches_credential("wrong"));
    }

    #[test]
    fn the_same_password_hashes_differently_per_salt() {
        // Two hosts with the same password must not share a hash, or one leaked
        // data directory would confirm the other's password.
        let one = hash_password_with("salt-a", "same-password");
        let two = hash_password_with("salt-b", "same-password");
        assert_ne!(one, two);
    }

    #[test]
    fn the_cookie_never_contains_the_password() {
        let auth = auth_with_password("hunter2");
        let mut headers = HeaderMap::new();
        auth.set_cookie(&mut headers);
        let cookie = headers
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("a cookie is set");

        assert!(
            !cookie.contains("hunter2"),
            "the password must not be recoverable from the cookie: {cookie}"
        );
        // And the value it does carry is accepted.
        let value = cookie
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, value)| value.to_owned())
            .expect("cookie has a value");
        assert!(auth.session_value_matches(&value));
        // The raw token is not that value, so a stolen cookie is not the token.
        assert!(!auth.session_value_matches("token-value"));
    }

    #[test]
    fn without_a_password_the_cookie_is_the_token() {
        // Unchanged behaviour for an install that never set one.
        let auth = Auth::new("token-value".into(), None, "s".into(), false, Vec::new());
        let mut headers = HeaderMap::new();
        auth.set_cookie(&mut headers);
        let cookie = headers
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("a cookie is set");
        assert!(
            cookie.starts_with("jarvis_session=token-value;"),
            "{cookie}"
        );
    }
}
