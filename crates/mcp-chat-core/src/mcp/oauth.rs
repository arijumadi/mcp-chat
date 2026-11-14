//! OAuth 2.0 for remote MCP servers.
//!
//! Implements the pieces Jarvis needs for the MCP authorization flow: protected
//! resource discovery (RFC 9728), authorization-server metadata (RFC 8414),
//! dynamic client registration (RFC 7591), authorization code + PKCE (RFC 7636),
//! and refresh tokens. Tokens are persisted by `McpManager` in the OS keychain.

use std::collections::HashMap;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use url::{Url, form_urlencoded};

use crate::{APP_NAME, APP_VERSION, CoreError, MCP_PROTOCOL_VERSION, Result, now_ms};

const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REFRESH_WINDOW_MS: i64 = 30_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStart {
    pub flow_id: String,
    pub auth_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStatus {
    /// `pending`, `success`, `error`, `cancelled`, or `not_found`.
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// Absolute expiry in milliseconds, with a refresh margin already applied.
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub auth_method: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthDiscovery {
    pub resource: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: String,
}

#[derive(Debug)]
pub struct CallbackRequest {
    pub code: String,
    pub stream: TcpStream,
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

/// Discover the OAuth protected-resource and authorization-server metadata for
/// a remote MCP endpoint. Discovery never sends credentials.
pub async fn discover(http: &reqwest::Client, server_url: &str) -> Result<OAuthDiscovery> {
    let server = validate_oauth_url(server_url, "MCP server URL")?;
    let challenge = probe_www_authenticate(http, &server).await;
    let resource_metadata = fetch_protected_resource(http, &server, challenge.as_deref()).await?;
    let resource = validate_oauth_url(&resource_metadata.resource, "OAuth resource")?;
    if !same_origin(&resource, &server) {
        return Err(CoreError::invalid(
            "The OAuth resource metadata is for a different server.",
        ));
    }

    let issuer_raw = resource_metadata
        .authorization_servers
        .first()
        .ok_or_else(|| CoreError::invalid("The server did not advertise an OAuth issuer."))?;
    let issuer = validate_oauth_url(issuer_raw, "OAuth issuer")?;
    let metadata = fetch_authorization_server(http, &issuer).await?;
    let metadata_issuer = validate_oauth_url(&metadata.issuer, "OAuth issuer")?;
    if normalize_url(&metadata_issuer) != normalize_url(&issuer) {
        return Err(CoreError::invalid(
            "The OAuth authorization server returned a mismatched issuer.",
        ));
    }
    if !metadata.code_challenge_methods_supported.is_empty()
        && !metadata
            .code_challenge_methods_supported
            .iter()
            .any(|method| method.eq_ignore_ascii_case("S256"))
    {
        return Err(CoreError::invalid(
            "This OAuth server does not support PKCE S256.",
        ));
    }

    let mut scopes = if resource_metadata.scopes_supported.is_empty() {
        metadata.scopes_supported.clone()
    } else {
        resource_metadata.scopes_supported.clone()
    };
    scopes.sort();
    scopes.dedup();

    Ok(OAuthDiscovery {
        resource: normalize_url(&resource),
        issuer: normalize_url(&issuer),
        authorization_endpoint: metadata.authorization_endpoint,
        token_endpoint: metadata.token_endpoint,
        registration_endpoint: metadata.registration_endpoint,
        scopes,
    })
}

/// Register a public desktop OAuth client with the authorization server.
pub async fn register_client(
    http: &reqwest::Client,
    discovery: &OAuthDiscovery,
    redirect_uri: &str,
) -> Result<RegisteredClient> {
    let endpoint = discovery.registration_endpoint.as_deref().ok_or_else(|| {
        CoreError::invalid(
            "This OAuth server does not support dynamic client registration. \
             Add a client ID through the server's own developer console and use a bearer token for now.",
        )
    })?;
    let endpoint = validate_oauth_url(endpoint, "OAuth registration endpoint")?;
    let response = http
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .json(&json!({
            "client_name": "Jarvis",
            "software_id": "com.mcpchat.app",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        let detail = oauth_error(response).await;
        return Err(CoreError::Message(format!(
            "OAuth client registration failed: {detail}"
        )));
    }
    let registered: RegistrationResponse = response.json().await?;
    if registered.client_id.trim().is_empty() {
        return Err(CoreError::invalid(
            "The OAuth server returned an empty client ID.",
        ));
    }
    let auth_method = registered
        .token_endpoint_auth_method
        .filter(|method| !method.trim().is_empty())
        .unwrap_or_else(|| {
            if registered.client_secret.is_some() {
                "client_secret_basic".into()
            } else {
                "none".into()
            }
        });
    Ok(RegisteredClient {
        client_id: registered.client_id,
        client_secret: registered.client_secret,
        token_endpoint_auth_method: auth_method,
    })
}

/// Start a loopback callback listener and return its redirect URI.
pub async fn callback_listener() -> Result<(TcpListener, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, format!("http://127.0.0.1:{port}{CALLBACK_PATH}")))
}

/// Wait for the browser callback. The returned stream is left open so the
/// caller can exchange the code and then show a success or error page.
pub async fn wait_for_callback(
    listener: TcpListener,
    expected_state: &str,
    cancel: CancellationToken,
) -> Result<CallbackRequest> {
    let deadline = tokio::time::Instant::now() + CALLBACK_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(CoreError::Message(
                "Timed out waiting for the browser sign-in.".into(),
            ));
        }
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            accepted = tokio::time::timeout(remaining, listener.accept()) => accepted,
        };
        let (mut stream, _) = match accepted {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {
                return Err(CoreError::Message(
                    "Timed out waiting for the browser sign-in.".into(),
                ));
            }
        };

        let request = match read_request(&mut stream).await {
            Ok(request) => request,
            Err(_) => {
                let _ = write_response(
                    &mut stream,
                    400,
                    "Bad request",
                    "Jarvis could not read the sign-in response.",
                )
                .await;
                continue;
            }
        };
        let Some(target) = request.request_line.split_whitespace().nth(1) else {
            let _ = write_response(
                &mut stream,
                400,
                "Bad request",
                "Jarvis could not read the sign-in response.",
            )
            .await;
            continue;
        };
        let Ok(url) = callback_url(target) else {
            let _ = write_response(
                &mut stream,
                400,
                "Bad request",
                "Jarvis could not read the sign-in response.",
            )
            .await;
            continue;
        };
        if url.path() != CALLBACK_PATH {
            let _ = write_response(
                &mut stream,
                404,
                "Not found",
                "This is not the Jarvis OAuth callback.",
            )
            .await;
            continue;
        }

        let params: HashMap<String, String> =
            form_urlencoded::parse(url.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        if let Some(error) = params.get("error") {
            let description = params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or(error);
            let _ = write_response(
                &mut stream,
                400,
                "Sign-in denied",
                "The provider did not authorize Jarvis. You can close this tab.",
            )
            .await;
            return Err(CoreError::Message(format!(
                "OAuth sign-in was denied: {}",
                clean_detail(description)
            )));
        }
        if params.get("state").map(String::as_str) != Some(expected_state) {
            let _ = write_response(
                &mut stream,
                400,
                "Sign-in expired",
                "The OAuth state did not match. Start the connection again from Jarvis.",
            )
            .await;
            return Err(CoreError::Message(
                "The OAuth state did not match. Start the connection again.".into(),
            ));
        }
        let code = match params.get("code") {
            Some(code) if !code.is_empty() => code.clone(),
            _ => {
                let _ = write_response(
                    &mut stream,
                    400,
                    "Missing code",
                    "The provider did not return an authorization code. Start again from Jarvis.",
                )
                .await;
                return Err(CoreError::Message(
                    "The OAuth provider did not return an authorization code.".into(),
                ));
            }
        };
        return Ok(CallbackRequest { code, stream });
    }
}

pub async fn respond_success(stream: &mut TcpStream) -> Result<()> {
    write_response(
        stream,
        200,
        "Connected",
        "Jarvis is connected. You can close this tab and return to the app.",
    )
    .await
}

pub async fn respond_error(stream: &mut TcpStream) -> Result<()> {
    write_response(
        stream,
        400,
        "Connection failed",
        "Jarvis could not finish the sign-in. Return to the app for details.",
    )
    .await
}

pub fn new_state() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn pkce_pair() -> (String, String) {
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let digest = Sha256::digest(verifier.as_bytes());
    (verifier, URL_SAFE_NO_PAD.encode(digest))
}

pub fn authorization_url(
    discovery: &OAuthDiscovery,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<String> {
    let mut url = validate_oauth_url(
        &discovery.authorization_endpoint,
        "OAuth authorization endpoint",
    )?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", state);
        query.append_pair("resource", &discovery.resource);
        if !discovery.scopes.is_empty() {
            query.append_pair("scope", &discovery.scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

pub async fn exchange_code(
    http: &reqwest::Client,
    discovery: &OAuthDiscovery,
    client: &RegisteredClient,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<OAuthToken> {
    let mut form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("code_verifier".to_string(), verifier.to_string()),
        ("resource".to_string(), discovery.resource.clone()),
    ];
    let token_endpoint = validate_oauth_url(&discovery.token_endpoint, "OAuth token endpoint")?;
    let request = token_request(http, token_endpoint, client, &mut form);
    let response = request.send().await?;
    parse_token_response(response, client, discovery).await
}

pub async fn refresh_token(http: &reqwest::Client, token: &OAuthToken) -> Result<OAuthToken> {
    let refresh_token = token.refresh_token.as_deref().ok_or_else(|| {
        CoreError::Message("The OAuth session expired. Reconnect the server.".into())
    })?;
    let endpoint = token.token_endpoint.as_deref().ok_or_else(|| {
        CoreError::Message(
            "The OAuth session is missing its token endpoint. Reconnect the server.".into(),
        )
    })?;
    let endpoint = validate_oauth_url(endpoint, "OAuth token endpoint")?;
    let client = RegisteredClient {
        client_id: token.client_id.clone().unwrap_or_default(),
        client_secret: token.client_secret.clone(),
        token_endpoint_auth_method: token.auth_method.clone().unwrap_or_else(|| "none".into()),
    };
    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if let Some(resource) = &token.resource {
        form.push(("resource".to_string(), resource.clone()));
    }
    let request = token_request(http, endpoint, &client, &mut form);
    let response = request.send().await?;
    if !response.status().is_success() {
        let detail = oauth_error(response).await;
        return Err(CoreError::Message(format!(
            "OAuth token refresh failed: {detail}"
        )));
    }
    let refreshed: TokenResponse = response.json().await?;
    if refreshed.access_token.trim().is_empty() {
        return Err(CoreError::invalid(
            "The OAuth server returned an empty access token.",
        ));
    }
    Ok(OAuthToken {
        access_token: refreshed.access_token,
        refresh_token: refreshed
            .refresh_token
            .or_else(|| token.refresh_token.clone()),
        token_type: refreshed.token_type.or_else(|| token.token_type.clone()),
        scope: refreshed.scope.or_else(|| token.scope.clone()),
        expires_at: refreshed.expires_in.map(expiry_at),
        issuer: token.issuer.clone(),
        client_id: token.client_id.clone(),
        client_secret: token.client_secret.clone(),
        token_endpoint: token.token_endpoint.clone(),
        resource: token.resource.clone(),
        auth_method: token.auth_method.clone(),
    })
}

pub fn needs_refresh(token: &OAuthToken) -> bool {
    token
        .expires_at
        .is_some_and(|expires_at| expires_at <= now_ms() + REFRESH_WINDOW_MS)
}

fn token_request(
    http: &reqwest::Client,
    endpoint: Url,
    client: &RegisteredClient,
    form: &mut Vec<(String, String)>,
) -> reqwest::RequestBuilder {
    let use_basic = client
        .token_endpoint_auth_method
        .eq_ignore_ascii_case("client_secret_basic")
        && client.client_secret.is_some();
    if !use_basic {
        form.push(("client_id".to_string(), client.client_id.clone()));
        if let Some(secret) = &client.client_secret {
            form.push(("client_secret".to_string(), secret.clone()));
        }
    }
    let request = http
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&*form);
    if use_basic {
        request.basic_auth(&client.client_id, client.client_secret.as_deref())
    } else {
        request
    }
}

async fn parse_token_response(
    response: reqwest::Response,
    client: &RegisteredClient,
    discovery: &OAuthDiscovery,
) -> Result<OAuthToken> {
    if !response.status().is_success() {
        let detail = oauth_error(response).await;
        return Err(CoreError::Message(format!(
            "OAuth token exchange failed: {detail}"
        )));
    }
    let token: TokenResponse = response.json().await?;
    if token.access_token.trim().is_empty() {
        return Err(CoreError::invalid(
            "The OAuth server returned an empty access token.",
        ));
    }
    Ok(OAuthToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        token_type: token.token_type,
        scope: token.scope,
        expires_at: token.expires_in.map(expiry_at),
        issuer: Some(discovery.issuer.clone()),
        client_id: Some(client.client_id.clone()),
        client_secret: client.client_secret.clone(),
        token_endpoint: Some(discovery.token_endpoint.clone()),
        resource: Some(discovery.resource.clone()),
        auth_method: Some(client.token_endpoint_auth_method.clone()),
    })
}

fn expiry_at(seconds: i64) -> i64 {
    now_ms() + seconds.saturating_mul(1000) - REFRESH_WINDOW_MS
}

async fn probe_www_authenticate(http: &reqwest::Client, server: &Url) -> Option<String> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": "jarvis-oauth-probe",
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": APP_NAME, "version": APP_VERSION}
        }
    });
    let response = http
        .post(server.clone())
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .ok()?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return response
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
    }
    let response = http
        .get(server.clone())
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .ok()?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return response
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
    }
    None
}

async fn fetch_protected_resource(
    http: &reqwest::Client,
    server: &Url,
    challenge: Option<&str>,
) -> Result<ProtectedResourceMetadata> {
    let mut candidates = Vec::new();
    if let Some(url) = challenge.and_then(resource_metadata_from_challenge) {
        let parsed = server
            .join(&url)
            .map_err(|_| CoreError::invalid("Invalid OAuth resource metadata URL"))?;
        if !same_origin(&parsed, server) {
            return Err(CoreError::invalid(
                "The OAuth resource metadata URL points to a different origin.",
            ));
        }
        candidates.push(parsed);
    }
    candidates.push(well_known_url(
        server,
        "oauth-protected-resource",
        server.path(),
    ));
    candidates.push(well_known_url(server, "oauth-protected-resource", "/"));
    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }

    for url in unique {
        if let Some(metadata) = get_json::<ProtectedResourceMetadata>(http, &url).await? {
            return Ok(metadata);
        }
    }
    Err(CoreError::Message(
        "This MCP server requires OAuth but did not advertise protected-resource metadata.".into(),
    ))
}

async fn fetch_authorization_server(
    http: &reqwest::Client,
    issuer: &Url,
) -> Result<AuthorizationServerMetadata> {
    let candidates = vec![
        well_known_url(issuer, "oauth-authorization-server", issuer.path()),
        well_known_url(issuer, "openid-configuration", issuer.path()),
        well_known_url(issuer, "oauth-authorization-server", "/"),
        well_known_url(issuer, "openid-configuration", "/"),
    ];
    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    for url in unique {
        if let Some(metadata) = get_json::<AuthorizationServerMetadata>(http, &url).await? {
            return Ok(metadata);
        }
    }
    Err(CoreError::Message(
        "The OAuth authorization server metadata could not be loaded.".into(),
    ))
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    url: &Url,
) -> Result<Option<T>> {
    let response = http
        .get(url.clone())
        .header(ACCEPT, "application/json")
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let bytes = response.bytes().await?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| CoreError::Message(format!("Malformed OAuth metadata: {error}")))
}

fn resource_metadata_from_challenge(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    let start = lower.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = header.get(start..)?.trim_start();
    let value = if let Some(rest) = rest.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        rest.split([',', ' ']).next()?
    };
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn well_known_url(base: &Url, suffix: &str, path: &str) -> Url {
    let mut url = base.clone();
    let path = path.trim_end_matches('/');
    url.set_path(&format!("/.well-known/{suffix}{path}"));
    url.set_query(None);
    url.set_fragment(None);
    url
}

fn validate_oauth_url(raw: &str, label: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|_| CoreError::invalid(format!("Invalid {label}")))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(CoreError::invalid(format!(
            "{label} must use https:// (localhost may use http://)"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CoreError::invalid(format!(
            "{label} must not contain credentials"
        )));
    }
    if url.fragment().is_some() {
        return Err(CoreError::invalid(format!(
            "{label} must not contain a fragment"
        )));
    }
    Ok(url)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn normalize_url(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_owned()
}

async fn oauth_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<Value>(&body) {
        if let Some(description) = value.get("error_description").and_then(Value::as_str) {
            return clean_detail(description);
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return clean_detail(error);
        }
    }
    format!("HTTP {status}")
}

fn clean_detail(value: &str) -> String {
    value.chars().take(300).collect::<String>()
}

struct HttpRequest {
    request_line: String,
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::with_capacity(4096);
    loop {
        let mut chunk = [0u8; 2048];
        let read = tokio::time::timeout(CONNECTION_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| CoreError::Message("Timed out reading the OAuth callback.".into()))??;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err(CoreError::Message(
                "OAuth callback request was too large.".into(),
            ));
        }
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    let request_line = text.lines().next().unwrap_or_default().to_owned();
    Ok(HttpRequest { request_line })
}

fn callback_url(target: &str) -> Result<Url> {
    if target.starts_with("http://") || target.starts_with("https://") {
        let url = Url::parse(target).map_err(|_| CoreError::invalid("Invalid OAuth callback"))?;
        if url.path() != CALLBACK_PATH {
            return Err(CoreError::invalid("Invalid OAuth callback path"));
        }
        return Ok(url);
    }
    Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| CoreError::invalid("Invalid OAuth callback"))
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    title: &str,
    body: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title} · Jarvis</title><body style=\"margin:0;background:#0c0d10;color:#e7e9ee;font:15px/1.55 system-ui,sans-serif;display:grid;min-height:100vh;place-items:center\"><main style=\"max-width:34rem;padding:2rem\"><h1 style=\"font-size:1.35rem;margin:0 0 .75rem\">{title}</h1><p style=\"margin:0;color:#a6abb8\">{body}</p></main></body>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = Sha256::digest(verifier.as_bytes());
        assert_eq!(
            URL_SAFE_NO_PAD.encode(digest),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn resource_metadata_is_parsed_from_a_bearer_challenge() {
        let header = r#"Bearer realm="mcp", resource_metadata="http://127.0.0.1:3773/.well-known/oauth-protected-resource/mcp""#;
        assert_eq!(
            resource_metadata_from_challenge(header).as_deref(),
            Some("http://127.0.0.1:3773/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[test]
    fn well_known_paths_keep_the_resource_path() {
        let base = Url::parse("https://example.com/mcp").unwrap();
        assert_eq!(
            well_known_url(&base, "oauth-protected-resource", base.path()).as_str(),
            "https://example.com/.well-known/oauth-protected-resource/mcp"
        );
        let issuer = Url::parse("https://auth.example.com/tenant").unwrap();
        assert_eq!(
            well_known_url(&issuer, "oauth-authorization-server", issuer.path()).as_str(),
            "https://auth.example.com/.well-known/oauth-authorization-server/tenant"
        );
    }

    #[test]
    fn callback_url_accepts_loopback_and_rejects_other_paths() {
        let callback = callback_url("/callback?code=abc&state=def").unwrap();
        assert_eq!(callback.path(), "/callback");
        assert_eq!(
            callback
                .query_pairs()
                .find(|(key, _)| key == "code")
                .unwrap()
                .1,
            "abc"
        );
        assert_eq!(callback_url("/favicon.ico").unwrap().path(), "/favicon.ico");
    }

    #[test]
    fn token_expiry_uses_the_refresh_window() {
        let mut token = OAuthToken {
            access_token: "token".into(),
            refresh_token: None,
            token_type: None,
            scope: None,
            expires_at: Some(now_ms() + REFRESH_WINDOW_MS + 1),
            issuer: None,
            client_id: None,
            client_secret: None,
            token_endpoint: None,
            resource: None,
            auth_method: None,
        };
        assert!(!needs_refresh(&token));
        token.expires_at = Some(now_ms() + REFRESH_WINDOW_MS - 1);
        assert!(needs_refresh(&token));
    }

    #[test]
    fn authorization_url_carries_pkce_state_and_resource() {
        let discovery = OAuthDiscovery {
            resource: "https://example.com/mcp".into(),
            issuer: "https://auth.example.com".into(),
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            registration_endpoint: None,
            scopes: vec!["mcp.read".into(), "mcp.write".into()],
        };
        let url = authorization_url(
            &discovery,
            "client-1",
            "http://127.0.0.1:49152/callback",
            "state-1",
            "challenge-1",
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let params: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("resource").map(String::as_str),
            Some("https://example.com/mcp")
        );
        assert_eq!(
            params.get("scope").map(String::as_str),
            Some("mcp.read mcp.write")
        );
    }
}
