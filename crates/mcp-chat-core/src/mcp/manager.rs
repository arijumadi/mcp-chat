use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::db::repo::servers;
use crate::mcp::client::{
    ClientConfig, McpClient, McpConnection, ProgressFn, args_from_json, env_from_json,
    headers_from_json,
};
use crate::mcp::oauth::{self, OAuthStart, OAuthStatus, OAuthToken};
use crate::mcp::types::{ServerCapabilities, ServerInfo, ToolInfo, TransportKind};
use crate::models::Server;
use crate::secrets::{
    SecretStore, oauth_account, oauth_token_account, server_api_key_account, server_bearer_account,
};
use crate::{CoreError, MCP_LEGACY_VERSION, Result, now_ms};

pub struct McpManager {
    db: Db,
    secrets: Arc<dyn SecretStore>,
    connections: RwLock<HashMap<String, Arc<McpConnection>>>,
    ttl: RwLock<HashMap<String, i64>>,
    oauth_flows: RwLock<HashMap<String, OAuthFlow>>,
}

struct OAuthFlow {
    status: OAuthStatus,
    cancel: CancellationToken,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReport {
    pub protocol_version: String,
    pub transport: String,
    pub server_info: Option<ServerInfo>,
    pub capabilities: Option<ServerCapabilities>,
    pub instructions: Option<String>,
    pub tools: Vec<ToolInfo>,
}

impl McpManager {
    pub fn new(db: Db, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            db,
            secrets,
            connections: RwLock::new(HashMap::new()),
            ttl: RwLock::new(HashMap::new()),
            oauth_flows: RwLock::new(HashMap::new()),
        }
    }

    pub async fn test(
        &self,
        name: &str,
        slug: &str,
        url: &str,
        headers: HashMap<String, String>,
        bearer: Option<String>,
        transport: Option<TransportKind>,
    ) -> Result<ConnectionReport> {
        self.test_config(ClientConfig {
            server_id: "test".into(),
            server_name: name.into(),
            server_slug: slug.into(),
            url: url.into(),
            transport,
            headers,
            bearer,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn test_stdio(
        &self,
        name: &str,
        slug: &str,
        command: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        cwd: Option<String>,
    ) -> Result<ConnectionReport> {
        self.test_config(ClientConfig {
            server_id: "test".into(),
            server_name: name.into(),
            server_slug: slug.into(),
            url: String::new(),
            transport: Some(TransportKind::Stdio),
            headers: HashMap::new(),
            bearer: None,
            command: command.into(),
            args,
            env,
            cwd,
        })
        .await
    }

    /// Test a saved server, including the current OAuth session.
    pub async fn test_saved(&self, server: &Server) -> Result<ConnectionReport> {
        let config = self.client_config(server).await?;
        self.test_config(config).await
    }

    async fn test_config(&self, config: ClientConfig) -> Result<ConnectionReport> {
        let connection = McpClient::connect(config, CancellationToken::new()).await?;
        let tools = connection
            .client
            .list_tools(&CancellationToken::new())
            .await?;
        Ok(ConnectionReport {
            protocol_version: connection.negotiated.protocol_version,
            transport: connection.negotiated.transport.as_str().into(),
            server_info: connection.negotiated.server_info,
            capabilities: connection.negotiated.capabilities,
            instructions: connection.negotiated.instructions,
            tools: tools.tools,
        })
    }

    pub async fn connect_server(&self, server: &Server) -> Result<Arc<McpConnection>> {
        if server.auth_kind == "oauth" {
            let token = self.oauth_token(server).await?.ok_or_else(|| {
                CoreError::Message("Connect this server with OAuth first.".into())
            })?;
            if oauth::needs_refresh(&token) {
                self.refresh_oauth_session(&server.id, &token).await?;
                self.connections.write().await.remove(&server.id);
            }
        }
        if let Some(connection) = self.connections.read().await.get(&server.id).cloned() {
            if !connection.client.is_closed() {
                return Ok(connection);
            }
            self.connections.write().await.remove(&server.id);
        }
        let mut config = self.client_config(server).await?;
        let cached_legacy = server.transport != "stdio"
            && server.protocol_version.as_deref() == Some(MCP_LEGACY_VERSION);
        if cached_legacy && config.transport.is_none() {
            config.transport = Some(TransportKind::LegacyHttp);
        }
        let mut retry_legacy = cached_legacy;
        let mut retry_oauth = server.auth_kind == "oauth";
        let connection = loop {
            match McpClient::connect(config.clone(), CancellationToken::new()).await {
                Ok(connection) => break connection,
                Err(error) if retry_legacy => {
                    tracing::debug!(server = %server.slug, error = %error.user_message(), "cached legacy negotiation failed; probing again");
                    retry_legacy = false;
                    config.transport = None;
                }
                Err(error) if retry_oauth && is_auth_error(&error) => {
                    retry_oauth = false;
                    self.force_refresh_oauth(server).await?;
                    config = self.client_config(server).await?;
                }
                Err(error) => return Err(error),
            }
        };
        let info_json = connection
            .negotiated
            .server_info
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let caps_json = connection
            .negotiated
            .capabilities
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let _ = servers::set_transport(
            &self.db.pool,
            &server.id,
            connection.negotiated.transport.as_str(),
        )
        .await;
        servers::record_discovery(
            &self.db.pool,
            &server.id,
            servers::DiscoveryUpdate {
                protocol_version: Some(&connection.negotiated.protocol_version),
                server_info_json: info_json.as_deref(),
                capabilities_json: caps_json.as_deref(),
                status: "connected",
                error: None,
                oauth_issuer: None,
                oauth_client_id: None,
            },
        )
        .await?;
        let connection = Arc::new(connection);
        self.connections
            .write()
            .await
            .insert(server.id.clone(), connection.clone());
        Ok(connection)
    }

    async fn client_config(&self, server: &Server) -> Result<ClientConfig> {
        if server.transport == "stdio" {
            return Ok(ClientConfig {
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                server_slug: server.slug.clone(),
                url: String::new(),
                transport: Some(TransportKind::Stdio),
                headers: HashMap::new(),
                bearer: None,
                command: server.command.clone(),
                args: args_from_json(&server.args_json)?,
                env: env_from_json(&server.env_json)?,
                cwd: Some(server.cwd.clone()).filter(|cwd| !cwd.trim().is_empty()),
            });
        }
        let mut headers = headers_from_json(&server.headers_json)?;
        let bearer = match server.auth_kind.as_str() {
            "none" => None,
            "bearer" => {
                let account = server_bearer_account(&server.id);
                self.secrets
                    .get(&account)
                    .await?
                    .ok_or_else(|| {
                        CoreError::Message(
                            "This server needs a bearer token. Add it on the server card.".into(),
                        )
                    })?
                    .into()
            }
            "api_key" => {
                let account = server_api_key_account(&server.id);
                let token = self.secrets.get(&account).await?.ok_or_else(|| {
                    CoreError::Message(
                        "This server needs an X-API-Key. Add it on the server card.".into(),
                    )
                })?;
                headers.insert("X-API-Key".into(), token);
                None
            }
            "oauth" => Some(self.oauth_access_token(server).await?),
            other => {
                return Err(CoreError::Message(format!(
                    "Unknown server auth kind {other:?}"
                )));
            }
        };
        let transport = TransportKind::parse(&server.transport);
        Ok(ClientConfig {
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            server_slug: server.slug.clone(),
            url: server.url.clone(),
            transport,
            headers,
            bearer,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        })
    }

    pub async fn has_oauth_token(&self, server: &Server) -> bool {
        if let Some(raw) = self
            .secrets
            .get(&oauth_token_account(&server.id))
            .await
            .ok()
            .flatten()
        {
            let trimmed = raw.trim();
            if trimmed.starts_with('{') {
                let Ok(token) = serde_json::from_str::<OAuthToken>(trimmed) else {
                    return false;
                };
                return !(oauth::needs_refresh(&token) && token.refresh_token.is_none());
            }
            return true;
        }
        if let Some(issuer) = server.oauth_issuer.as_deref() {
            return self
                .secrets
                .get(&oauth_account(&server.id, issuer, "access_token"))
                .await
                .ok()
                .flatten()
                .is_some();
        }
        false
    }

    /// Start an interactive OAuth flow. The caller opens `auth_url` in the
    /// browser; the returned flow can be polled with `oauth_status`.
    pub async fn start_oauth(self: &Arc<Self>, server: &Server) -> Result<OAuthStart> {
        if server.transport == "stdio" {
            return Err(CoreError::invalid(
                "OAuth is only available for remote MCP servers.",
            ));
        }
        if server.auth_kind != "oauth" {
            return Err(CoreError::invalid(
                "Set this server's auth mode to OAuth before connecting.",
            ));
        }
        let http = crate::llm::provider::http_client()?;
        let discovery = oauth::discover(&http, &server.url).await?;
        let (listener, redirect_uri) = oauth::callback_listener().await?;
        let client = oauth::register_client(&http, &discovery, &redirect_uri).await?;
        let (verifier, challenge) = oauth::pkce_pair();
        let state = oauth::new_state();
        let auth_url = oauth::authorization_url(
            &discovery,
            &client.client_id,
            &redirect_uri,
            &state,
            &challenge,
        )?;
        let flow_id = crate::new_id();
        let cancel = CancellationToken::new();
        let created_at = now_ms();
        {
            let mut flows = self.oauth_flows.write().await;
            flows.retain(|_, flow| {
                flow.status.status == "pending" || flow.created_at >= created_at - 3_600_000
            });
            flows.insert(
                flow_id.clone(),
                OAuthFlow {
                    status: OAuthStatus {
                        status: "pending".into(),
                        error: None,
                    },
                    cancel: cancel.clone(),
                    created_at,
                },
            );
        }

        let manager = Arc::clone(self);
        let server_id = server.id.clone();
        let task_flow_id = flow_id.clone();
        let task_redirect_uri = redirect_uri.clone();
        tokio::spawn(async move {
            let callback = oauth::wait_for_callback(listener, &state, cancel).await;
            let (status, error) = match callback {
                Ok(mut callback) => {
                    let exchange = oauth::exchange_code(
                        &http,
                        &discovery,
                        &client,
                        &callback.code,
                        &task_redirect_uri,
                        &verifier,
                    )
                    .await;
                    match exchange {
                        Ok(token) => match manager.save_oauth_session(&server_id, &token).await {
                            Ok(()) => {
                                let _ = oauth::respond_success(&mut callback.stream).await;
                                ("success".to_owned(), None)
                            }
                            Err(error) => {
                                let _ = oauth::respond_error(&mut callback.stream).await;
                                ("error".to_owned(), Some(error.user_message()))
                            }
                        },
                        Err(error) => {
                            let _ = oauth::respond_error(&mut callback.stream).await;
                            ("error".to_owned(), Some(error.user_message()))
                        }
                    }
                }
                Err(CoreError::Cancelled) => ("cancelled".to_owned(), None),
                Err(error) => ("error".to_owned(), Some(error.user_message())),
            };
            manager
                .set_oauth_status(&task_flow_id, &status, error.clone())
                .await;
            if let Some(error) = error {
                let _ = servers::record_error(&manager.db.pool, &server_id, &error).await;
            }
        });

        Ok(OAuthStart {
            flow_id,
            auth_url,
            redirect_uri,
        })
    }

    pub async fn oauth_status(&self, flow_id: &str) -> OAuthStatus {
        self.oauth_flows
            .read()
            .await
            .get(flow_id)
            .map(|flow| flow.status.clone())
            .unwrap_or(OAuthStatus {
                status: "not_found".into(),
                error: None,
            })
    }

    pub async fn cancel_oauth(&self, flow_id: &str) {
        let mut flows = self.oauth_flows.write().await;
        if let Some(flow) = flows.get_mut(flow_id) {
            flow.cancel.cancel();
            flow.status = OAuthStatus {
                status: "cancelled".into(),
                error: None,
            };
        }
    }

    async fn set_oauth_status(&self, flow_id: &str, status: &str, error: Option<String>) {
        let mut flows = self.oauth_flows.write().await;
        if let Some(flow) = flows.get_mut(flow_id) {
            flow.status = OAuthStatus {
                status: status.to_owned(),
                error,
            };
        }
    }

    pub async fn oauth_token(&self, server: &Server) -> Result<Option<OAuthToken>> {
        if let Some(raw) = self.secrets.get(&oauth_token_account(&server.id)).await? {
            let trimmed = raw.trim();
            if trimmed.starts_with('{') {
                return serde_json::from_str(trimmed).map(Some).map_err(|error| {
                    CoreError::Message(format!(
                        "The saved OAuth session could not be read: {error}"
                    ))
                });
            }
            return Ok(Some(OAuthToken {
                access_token: raw,
                refresh_token: None,
                token_type: None,
                scope: None,
                expires_at: None,
                issuer: server.oauth_issuer.clone(),
                client_id: server.oauth_client_id.clone(),
                client_secret: None,
                token_endpoint: None,
                resource: Some(server.url.clone()),
                auth_method: None,
            }));
        }
        if let Some(issuer) = server.oauth_issuer.as_deref() {
            if let Some(access_token) = self
                .secrets
                .get(&oauth_account(&server.id, issuer, "access_token"))
                .await?
            {
                return Ok(Some(OAuthToken {
                    access_token,
                    refresh_token: None,
                    token_type: None,
                    scope: None,
                    expires_at: None,
                    issuer: Some(issuer.to_owned()),
                    client_id: server.oauth_client_id.clone(),
                    client_secret: None,
                    token_endpoint: None,
                    resource: Some(server.url.clone()),
                    auth_method: None,
                }));
            }
        }
        Ok(None)
    }

    async fn oauth_access_token(&self, server: &Server) -> Result<String> {
        let token = self
            .oauth_token(server)
            .await?
            .ok_or_else(|| CoreError::Message("Connect this server with OAuth first.".into()))?;
        if oauth::needs_refresh(&token) {
            return Ok(self
                .refresh_oauth_session(&server.id, &token)
                .await?
                .access_token);
        }
        Ok(token.access_token)
    }

    pub async fn force_refresh_oauth(&self, server: &Server) -> Result<OAuthToken> {
        let token = self
            .oauth_token(server)
            .await?
            .ok_or_else(|| CoreError::Message("Connect this server with OAuth first.".into()))?;
        if token.refresh_token.is_none() {
            return Err(CoreError::Message(
                "The OAuth session expired. Reconnect the server.".into(),
            ));
        }
        self.refresh_oauth_session(&server.id, &token).await
    }

    async fn refresh_oauth_session(
        &self,
        server_id: &str,
        token: &OAuthToken,
    ) -> Result<OAuthToken> {
        let http = crate::llm::provider::http_client()?;
        let refreshed = oauth::refresh_token(&http, token).await?;
        let json = serde_json::to_string(&refreshed)?;
        self.secrets
            .set(&oauth_token_account(server_id), &json)
            .await?;
        Ok(refreshed)
    }

    async fn save_oauth_session(&self, server_id: &str, token: &OAuthToken) -> Result<()> {
        let issuer = token
            .issuer
            .as_deref()
            .ok_or_else(|| CoreError::invalid("The OAuth token is missing its issuer."))?;
        let client_id = token
            .client_id
            .as_deref()
            .ok_or_else(|| CoreError::invalid("The OAuth token is missing its client ID."))?;
        let json = serde_json::to_string(token)?;
        self.secrets
            .set(&oauth_token_account(server_id), &json)
            .await?;
        let _ = self
            .secrets
            .delete(&oauth_account(server_id, issuer, "access_token"))
            .await;
        servers::set_oauth(&self.db.pool, server_id, issuer, client_id).await?;
        servers::record_discovery(
            &self.db.pool,
            server_id,
            servers::DiscoveryUpdate {
                protocol_version: None,
                server_info_json: None,
                capabilities_json: None,
                status: "connected",
                error: None,
                oauth_issuer: Some(issuer),
                oauth_client_id: Some(client_id),
            },
        )
        .await?;
        self.connections.write().await.remove(server_id);
        Ok(())
    }

    pub async fn disconnect_oauth(&self, server: &Server) -> Result<()> {
        self.secrets
            .delete(&oauth_token_account(&server.id))
            .await?;
        if let Some(issuer) = server.oauth_issuer.as_deref() {
            for suffix in ["access_token", "refresh_token", "client_secret"] {
                let _ = self
                    .secrets
                    .delete(&oauth_account(&server.id, issuer, suffix))
                    .await;
            }
        }
        servers::clear_oauth(&self.db.pool, &server.id).await?;
        self.invalidate(&server.id).await;
        Ok(())
    }

    pub async fn refresh_tools(&self, server_id: &str) -> Result<Vec<ToolInfo>> {
        let server = servers::get(&self.db.pool, server_id).await?;
        let connection = self.connect_server(&server).await?;
        let result = connection
            .client
            .list_tools(&CancellationToken::new())
            .await?;
        let json = serde_json::to_string(&result.tools)?;
        servers::cache_tools(&self.db.pool, server_id, &json).await?;
        if let Some(ttl) = result.ttl_ms {
            self.ttl
                .write()
                .await
                .insert(server_id.to_owned(), ttl.clamp(0, 600_000));
        }
        Ok(result.tools)
    }

    pub async fn report(&self, server_id: &str) -> Result<ConnectionReport> {
        let tools = self.refresh_tools(server_id).await?;
        let server = servers::get(&self.db.pool, server_id).await?;
        Ok(ConnectionReport {
            protocol_version: server
                .protocol_version
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            transport: server.transport.clone(),
            server_info: server
                .server_info_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            capabilities: server
                .capabilities_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            instructions: None,
            tools,
        })
    }

    pub async fn tools_for_enabled_servers(&self) -> Result<Vec<(Server, Vec<ToolInfo>)>> {
        let servers = servers::list_enabled(&self.db.pool).await?;
        self.tools_for_server_list(servers).await
    }

    pub async fn tools_for_ids(&self, ids: &[String]) -> Result<Vec<(Server, Vec<ToolInfo>)>> {
        let servers = servers::list_enabled(&self.db.pool).await?;
        let servers = servers
            .into_iter()
            .filter(|server| ids.iter().any(|id| id == &server.id))
            .collect();
        self.tools_for_server_list(servers).await
    }

    pub async fn tools_for_slugs(&self, slugs: &[String]) -> Result<Vec<(Server, Vec<ToolInfo>)>> {
        let servers = servers::list_enabled(&self.db.pool).await?;
        let servers = servers
            .into_iter()
            .filter(|server| slugs.iter().any(|slug| slug == &server.slug))
            .collect();
        self.tools_for_server_list(servers).await
    }

    pub async fn resolve_tool(&self, slug: &str, remote_name: &str) -> Result<(Server, ToolInfo)> {
        let server = servers::get_by_slug(&self.db.pool, slug)
            .await?
            .ok_or_else(|| CoreError::Message(format!("No MCP server with slug {slug:?}.")))?;
        if !server.enabled {
            return Err(CoreError::Message(format!(
                "MCP server {slug:?} is disabled."
            )));
        }
        if server.policy == "deny" {
            return Err(CoreError::Message(format!(
                "MCP server {slug:?} is denied by policy."
            )));
        }
        let groups = self.tools_for_server_list(vec![server.clone()]).await?;
        let tools = groups
            .into_iter()
            .next()
            .map(|(_, tools)| tools)
            .unwrap_or_default();
        let tool = tools
            .into_iter()
            .find(|tool| tool.name == remote_name)
            .ok_or_else(|| {
                CoreError::Message(format!(
                    "No tool named {remote_name:?} on MCP server {slug:?}."
                ))
            })?;
        Ok((server, tool))
    }

    async fn tools_for_server_list(
        &self,
        servers: Vec<Server>,
    ) -> Result<Vec<(Server, Vec<ToolInfo>)>> {
        let mut out = Vec::new();
        for server in servers {
            if server.policy == "deny" {
                continue;
            }
            if server.auth_kind == "oauth" && !self.has_oauth_token(&server).await {
                continue;
            }
            let cached = servers::cached_tools(&self.db.pool, &server.id).await?;
            let fresh = match &cached {
                Some((json, at)) => {
                    let ttl = self
                        .ttl
                        .read()
                        .await
                        .get(&server.id)
                        .copied()
                        .unwrap_or(600_000);
                    if now_ms() - at < ttl {
                        serde_json::from_str::<Vec<ToolInfo>>(json)
                            .ok()
                            .map(|tools| (tools, true))
                    } else {
                        serde_json::from_str::<Vec<ToolInfo>>(json)
                            .ok()
                            .map(|tools| (tools, false))
                    }
                }
                None => None,
            };
            if let Some((tools, true)) = fresh {
                out.push((server, tools));
                continue;
            }
            match self.refresh_tools(&server.id).await {
                Ok(tools) => out.push((server, tools)),
                Err(error) => {
                    let message = error.user_message();
                    let _ = servers::record_error(&self.db.pool, &server.id, &message).await;
                    if let Some((tools, false)) = fresh {
                        out.push((server, tools));
                    }
                }
            }
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn call_tool(
        &self,
        server_id: &str,
        remote_name: &str,
        arguments: &serde_json::Value,
        tool: Option<&ToolInfo>,
        retry: Option<crate::mcp::MrtrRetry>,
        progress: Option<ProgressFn>,
        cancel: &CancellationToken,
    ) -> Result<crate::mcp::CallToolResult> {
        let server = servers::get(&self.db.pool, server_id).await?;
        let mut retried = false;
        loop {
            let connection = self.connect_server(&server).await?;
            let result = connection
                .client
                .call_tool(
                    remote_name,
                    arguments,
                    tool,
                    retry.clone(),
                    progress.clone(),
                    cancel,
                )
                .await;
            match result {
                Err(error) if !retried && server.auth_kind == "oauth" && is_auth_error(&error) => {
                    retried = true;
                    self.invalidate(server_id).await;
                    self.force_refresh_oauth(&server).await?;
                }
                Err(error) => {
                    if is_auth_error(&error) {
                        self.invalidate(server_id).await;
                    }
                    return Err(error);
                }
                Ok(result) => return Ok(result),
            }
        }
    }

    pub async fn invalidate(&self, server_id: &str) {
        self.connections.write().await.remove(server_id);
        self.ttl.write().await.remove(server_id);
        let _ = servers::clear_tools(&self.db.pool, server_id).await;
    }

    pub async fn refresh_all(&self) {
        let servers = self.db.pool.clone();
        let Ok(list) = servers::list_enabled(&servers).await else {
            return;
        };
        for server in list {
            if server.auth_kind == "oauth" && !self.has_oauth_token(&server).await {
                continue;
            }
            if let Err(error) = self.refresh_tools(&server.id).await {
                tracing::warn!(server = %server.slug, error = %error.user_message(), "MCP refresh failed");
                let _ =
                    servers::record_error(&self.db.pool, &server.id, &error.user_message()).await;
            }
        }
    }

    pub async fn cached_tools(&self, server_id: &str) -> Result<Vec<ToolInfo>> {
        Ok(servers::cached_tools(&self.db.pool, server_id)
            .await?
            .and_then(|(json, _)| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }
}

fn is_auth_error(error: &CoreError) -> bool {
    match error {
        CoreError::Unauthorized => true,
        CoreError::Message(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("rejected the credentials")
                || message.contains("requires authentication")
                || message.contains("authentication failed")
                || message.contains("token missing")
        }
        _ => false,
    }
}
