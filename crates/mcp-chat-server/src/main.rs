//! Headless Jarvis.
//!
//! The same engine and the same web UI as the desktop app, reachable from
//! another computer. Everything — processes, workspace, database, secrets —
//! stays on this host; the browser is only a view.
//!
//! This binary deliberately does not depend on Tauri. `mcp-chat-core` has no
//! GUI coupling, so the only new pieces are an HTTP transport, a file-backed
//! secret store, and cookie authentication.

mod api;
mod assets;
mod auth;
mod events;
mod secrets;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

use mcp_chat_core::agent::ChatEngine;
use mcp_chat_core::db::Db;
use mcp_chat_core::db::repo::settings;
use mcp_chat_core::mcp::McpManager;
use mcp_chat_core::secrets::SecretStore;

use crate::auth::Auth;
use crate::state::AppState;

/// Attachments are base64 in the request body, and a 5 MB image becomes about
/// 6.7 MB of JSON. The default 2 MB limit would reject it.
const BODY_LIMIT: usize = 64 * 1024 * 1024;

#[tokio::main]
async fn main() {
    init_logging();
    if let Err(error) = run().await {
        // Before the port is bound there is no UI to show this in, so stderr is
        // the only honest place.
        eprintln!("jarvis-server: {error}");
        std::process::exit(1);
    }
}

struct Config {
    data_dir: PathBuf,
    workspace: Option<PathBuf>,
    static_dir: PathBuf,
    /// Every address to listen on. A host that is reachable over both a LAN and
    /// Tailscale needs both, and one socket cannot cover them.
    binds: Vec<SocketAddr>,
    token: String,
    /// Optional password, hashed. See `Auth::hash_password`.
    password: Option<String>,
    /// Per-installation salt for the password hash.
    salt: String,
    secure_cookie: bool,
    allowed_hosts: Vec<String>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let data_dir = match env_path("JARVIS_DATA_DIR") {
            Some(path) => path,
            None => default_data_dir()?,
        };
        let workspace = env_path("JARVIS_WORKSPACE");
        let static_dir = match env_path("JARVIS_STATIC_DIR") {
            Some(path) => path,
            None => default_static_dir()?,
        };
        // Comma separated. `0.0.0.0:8787` covers every interface, but it also
        // exposes Docker bridges and tunnels the user did not ask for; naming
        // each address is explicit.
        let binds = std::env::var("JARVIS_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".into())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value.parse::<SocketAddr>().map_err(|error| {
                    format!("JARVIS_BIND has an invalid address {value:?}: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if binds.is_empty() {
            return Err("JARVIS_BIND is empty".into());
        }
        let secure_cookie = std::env::var("JARVIS_SECURE_COOKIE")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        // Comma separated. A Tailscale host legitimately answers to several
        // names (its address and its MagicDNS name), and pinning one makes the
        // other return 403 for no security gain.
        let allowed_hosts = std::env::var("JARVIS_ALLOWED_HOST")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let token = load_or_create_token(&data_dir)?;
        let salt = load_or_create_salt(&data_dir)?;
        let password = match std::env::var("JARVIS_PASSWORD") {
            Ok(value) if !value.trim().is_empty() => Some(value),
            _ => std::fs::read_to_string(data_dir.join("server-password"))
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        };
        let password = password.map(|plain| crate::auth::hash_password_with(&salt, &plain));
        Ok(Self {
            data_dir,
            workspace,
            static_dir,
            binds,
            token,
            password,
            salt,
            secure_cookie,
            allowed_hosts,
        })
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_data_dir() -> Result<PathBuf, String> {
    if let Some(xdg) = env_path("XDG_DATA_HOME") {
        return Ok(xdg.join("jarvis"));
    }
    if let Some(home) = env_path("HOME") {
        return Ok(home.join(".local").join("share").join("jarvis"));
    }
    Err("Set JARVIS_DATA_DIR: neither XDG_DATA_HOME nor HOME is set.".into())
}

fn default_static_dir() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("dist");
            if candidate.join("index.html").is_file() {
                return Ok(candidate);
            }
        }
    }
    let local = PathBuf::from("dist");
    if local.join("index.html").is_file() {
        return Ok(local);
    }
    Err(
        "The web UI bundle was not found. Set JARVIS_STATIC_DIR to the Vite \
         `dist/` directory."
            .into(),
    )
}

/// A stable token is important: regenerating it on every restart would sign the
/// browser out each time the service is redeployed.
fn load_or_create_token(data_dir: &std::path::Path) -> Result<String, String> {
    if let Ok(token) = std::env::var("JARVIS_TOKEN") {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let path = data_dir.join("server-token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_owned();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
    std::fs::write(&path, &token)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// A stable per-installation salt, created on first use.
///
/// Kept next to the token in the data directory, which is already the thing an
/// attacker would need. Its purpose is not secrecy but uniqueness: two hosts with
/// the same password must not produce the same hash.
fn load_or_create_salt(data_dir: &std::path::Path) -> Result<String, String> {
    let path = data_dir.join("server-salt");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_owned();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let salt = uuid::Uuid::new_v4().simple().to_string();
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
    std::fs::write(&path, &salt)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(salt)
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    let state = bootstrap(&config).await?;

    let router = Router::new()
        .route("/login/", get(auth::login_page).post(auth::login))
        .route("/api/events", get(events::events))
        .route("/api/invoke", post(api::invoke))
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .with_state(state.clone());

    // One listener per address. A host on both a LAN and a Tailscale tailnet
    // needs two: 0.0.0.0 would also cover Docker bridges and tunnels the user
    // never asked to expose.
    //
    // An address that is not up yet (a DHCP lease that has not landed, a
    // Tailscale daemon still starting) must not take the whole server down, but
    // it must be loud: the configured address simply will not answer.
    let mut listeners = Vec::with_capacity(config.binds.len());
    for bind in &config.binds {
        match tokio::net::TcpListener::bind(bind).await {
            Ok(listener) => {
                let address = listener.local_addr().unwrap_or(*bind);
                listeners.push((listener, address));
            }
            Err(error) => {
                tracing::error!(
                    "cannot listen on {bind}: {error}. That address will not answer; \
                     the others still will."
                );
            }
        }
    }
    if listeners.is_empty() {
        return Err("no address could be bound; check JARVIS_BIND".into());
    }

    for (_, address) in &listeners {
        auth::announce(*address, state.auth.token(), address.ip().is_loopback());
    }
    tracing::info!(data_dir = %config.data_dir.display(), "data directory");
    tracing::info!(static_dir = %config.static_dir.display(), "web UI");
    if let Ok(workspace) = mcp_chat_core::agent::tools::workspace_root(&state.db.pool).await {
        tracing::info!(workspace = %workspace.display(), "workspace");
    }

    // One signal for every listener: dropping a receiver ends that server.
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let mut servers = Vec::with_capacity(listeners.len());
    for (listener, address) in listeners {
        let router = router.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        servers.push(tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                    tracing::info!("shutting down {address}");
                })
                .await;
            result.map_err(|error| format!("server on {address} stopped: {error}"))
        }));
    }

    // Serve until Ctrl-C, then let each listener finish and stop the agents.
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(());

    for server in servers {
        match server.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) => return Err(format!("a listener task failed: {error}")),
        }
    }

    // Child processes (ACP agents, stdio MCP servers) must not outlive us.
    state.engine.acp.close_all().await;
    Ok(())
}

async fn bootstrap(config: &Config) -> Result<Arc<AppState>, String> {
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|error| format!("cannot create {}: {error}", config.data_dir.display()))?;
    let output_dir = config.data_dir.join("tool-output");
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| format!("cannot create {}: {error}", output_dir.display()))?;
    let attachments_dir = config.data_dir.join("attachments");
    std::fs::create_dir_all(&attachments_dir)
        .map_err(|error| format!("cannot create {}: {error}", attachments_dir.display()))?;

    if let Some(report) = mcp_chat_core::builtin_skills::install_global_skills() {
        if report.changed() {
            tracing::info!(
                installed = report.installed,
                updated = report.updated,
                "bundled skills"
            );
        }
        if report.failed > 0 {
            tracing::warn!(
                failed = report.failed,
                "some bundled skills could not be installed"
            );
        }
    }

    let db_path = config.data_dir.join("mcp-chat.db");
    let db = Db::open(&db_path).await.map_err(|error| {
        format!(
            "cannot open the database at {}: {}",
            db_path.display(),
            error.user_message()
        )
    })?;
    mcp_chat_core::db::repo::messages::sweep_interrupted(&db.pool)
        .await
        .map_err(|error| {
            format!(
                "cannot clean up interrupted messages: {}",
                error.user_message()
            )
        })?;
    mcp_chat_core::attachments::sweep_pending(&attachments_dir).await;

    // Workspace: an explicit environment value wins, otherwise keep whatever is
    // stored, otherwise give the server its own directory so it never writes
    // into wherever the process happened to be started.
    if let Some(workspace) = &config.workspace {
        let path =
            mcp_chat_core::agent::tools::resolve_workspace_path(&workspace.display().to_string())
                .map_err(|error| error.user_message())?;
        settings::set(&db.pool, "workspace.root", &path.display().to_string())
            .await
            .map_err(|error| error.user_message())?;
    } else if settings::get(&db.pool, "workspace.root")
        .await
        .ok()
        .flatten()
        .is_none()
    {
        let path = config.data_dir.join("workspace");
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        settings::set(&db.pool, "workspace.root", &path.display().to_string())
            .await
            .map_err(|error| error.user_message())?;
    }

    let secrets_path = config.data_dir.join("secrets.json");
    secrets::prepare(&secrets_path)
        .map_err(|error| format!("cannot prepare {}: {error}", secrets_path.display()))?;
    let secrets: Arc<dyn SecretStore> = Arc::new(secrets::FileSecretStore::new(secrets_path));

    let manager = Arc::new(McpManager::new(db.clone(), secrets.clone()));
    let engine = ChatEngine::new(
        db.clone(),
        secrets.clone(),
        manager.clone(),
        output_dir.clone(),
        attachments_dir,
    );

    let state = Arc::new(AppState {
        engine: engine.clone(),
        db,
        secrets,
        mcp: manager.clone(),
        data_dir: config.data_dir.clone(),
        output_dir,
        static_dir: config.static_dir.clone(),
        auth: Auth::new(
            config.token.clone(),
            config.password.clone(),
            config.salt.clone(),
            config.secure_cookie,
            config.allowed_hosts.clone(),
        ),
    });

    tokio::spawn(async move {
        manager.refresh_all().await;
    });

    Ok(state)
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn,hyper=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mcp_chat_core::agent::ChatEngine;
    use mcp_chat_core::db::Db;
    use mcp_chat_core::mcp::McpManager;
    use mcp_chat_core::secrets::MemoryStore;

    use crate::api;
    use crate::state::AppState;

    async fn harness() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_memory().await.unwrap();
        let secrets: Arc<dyn mcp_chat_core::secrets::SecretStore> =
            Arc::new(MemoryStore::default());
        let mcp = Arc::new(McpManager::new(db.clone(), secrets.clone()));
        let engine = ChatEngine::new(
            db.clone(),
            secrets.clone(),
            mcp.clone(),
            dir.path().join("tool-output"),
            dir.path().join("attachments"),
        );
        std::fs::create_dir_all(&engine.attachments_dir).unwrap();
        let state = Arc::new(AppState {
            engine,
            db,
            secrets,
            mcp,
            data_dir: dir.path().to_path_buf(),
            output_dir: dir.path().join("tool-output"),
            static_dir: dir.path().to_path_buf(),
            auth: crate::auth::Auth::new("t".into(), None, "s".into(), false, Vec::new()),
        });
        (state, dir)
    }

    /// A one-pixel PNG, so the type detection path is the real one.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    /// Reported: picking an attachment and pressing send failed with
    /// "invalid type: null, expected a string". The upload returned a list
    /// while the caller expected a single attachment, so every field was
    /// undefined and `attachmentIds` became `[null]`.
    #[tokio::test]
    async fn an_upload_returns_one_attachment_not_a_list() {
        let (state, _dir) = harness().await;
        let value = api::dispatch(
            &state,
            "stage_attachment_upload",
            serde_json::json!({
                "data": format!("data:image/png;base64,{PNG}"),
                "name": "pixel.png",
            }),
        )
        .await
        .expect("upload");

        let object = value
            .as_object()
            .expect("the frontend reads `staged.id`, so this must be one object");
        assert_eq!(
            object.get("id").and_then(|id| id.as_str()).map(str::len),
            Some(36),
            "a missing id becomes null in `attachmentIds`"
        );
        assert_eq!(
            object.get("kind").and_then(|kind| kind.as_str()),
            Some("image")
        );
    }

    /// The id from an upload must be accepted by `send_message`, which is the
    /// step that actually failed in the report.
    #[tokio::test]
    async fn an_uploaded_id_can_be_sent() {
        let (state, _dir) = harness().await;
        let conversation = api::dispatch(
            &state,
            "create_conversation",
            serde_json::json!({ "input": {} }),
        )
        .await
        .expect("conversation");
        let conversation_id = conversation["id"].as_str().unwrap().to_owned();

        let staged = api::dispatch(
            &state,
            "stage_attachment_upload",
            serde_json::json!({
                "data": format!("data:image/png;base64,{PNG}"),
                "name": "pixel.png",
            }),
        )
        .await
        .expect("upload");
        let id = staged["id"].as_str().expect("id").to_owned();

        api::dispatch(
            &state,
            "send_message",
            serde_json::json!({
                "conversationId": conversation_id,
                "text": "here is a picture",
                "attachmentIds": [id],
            }),
        )
        .await
        .expect("the attachment id must be accepted by send_message");
    }

    #[tokio::test]
    async fn list_agent_sessions_reports_support_separately_from_emptiness() {
        let (state, _dir) = harness().await;

        // An agent that does not exist must fail loudly, not return a page that
        // looks like "you have no sessions".
        let missing = api::dispatch(
            &state,
            "list_agent_sessions",
            serde_json::json!({ "agentId": "does-not-exist" }),
        )
        .await;
        assert!(missing.is_err(), "an unknown agent id must be an error");
    }
}
