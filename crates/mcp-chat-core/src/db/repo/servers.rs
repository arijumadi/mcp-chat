use sqlx::SqlitePool;

use crate::models::Server;
use crate::{Result, new_id, now_ms};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub slug: String,
    pub url: String,
    pub transport: String,
    pub command: String,
    pub args_json: String,
    pub env_json: String,
    pub cwd: String,
    pub auth_kind: String,
    pub headers_json: String,
    pub policy: String,
}

impl ServerConfig {
    /// A remote HTTP server. Local stdio fields keep their empty defaults.
    pub fn http(
        name: impl Into<String>,
        slug: impl Into<String>,
        url: impl Into<String>,
        transport: impl Into<String>,
        auth_kind: impl Into<String>,
        headers_json: impl Into<String>,
        policy: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slug: slug.into(),
            url: url.into(),
            transport: transport.into(),
            command: String::new(),
            args_json: "[]".into(),
            env_json: "{}".into(),
            cwd: String::new(),
            auth_kind: auth_kind.into(),
            headers_json: headers_json.into(),
            policy: policy.into(),
        }
    }
}

pub async fn create(pool: &SqlitePool, config: &ServerConfig) -> Result<Server> {
    let id = new_id();
    let row = sqlx::query_as::<_, Server>(
        r#"INSERT INTO servers
           (id, name, slug, url, transport, command, args_json, env_json, cwd,
            auth_kind, headers_json, enabled, policy, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(&config.name)
    .bind(&config.slug)
    .bind(&config.url)
    .bind(&config.transport)
    .bind(&config.command)
    .bind(&config.args_json)
    .bind(&config.env_json)
    .bind(&config.cwd)
    .bind(&config.auth_kind)
    .bind(&config.headers_json)
    .bind(&config.policy)
    .bind(now_ms())
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Server>> {
    let rows = sqlx::query_as::<_, Server>("SELECT * FROM servers ORDER BY created_at ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn list_enabled(pool: &SqlitePool) -> Result<Vec<Server>> {
    let rows = sqlx::query_as::<_, Server>(
        "SELECT * FROM servers WHERE enabled = 1 ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Server> {
    sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| crate::CoreError::NotFound("server".into()))
}

pub async fn get_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<Server>> {
    let row = sqlx::query_as::<_, Server>("SELECT * FROM servers WHERE slug = ?1")
        .bind(slug)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn update(
    pool: &SqlitePool,
    id: &str,
    config: &ServerConfig,
    enabled: bool,
) -> Result<Server> {
    let row = sqlx::query_as::<_, Server>(
        r#"UPDATE servers SET
             name = ?2, url = ?3, transport = ?4, command = ?5, args_json = ?6,
             env_json = ?7, cwd = ?8, auth_kind = ?9, headers_json = ?10,
             policy = ?11, enabled = ?12
           WHERE id = ?1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&config.name)
    .bind(&config.url)
    .bind(&config.transport)
    .bind(&config.command)
    .bind(&config.args_json)
    .bind(&config.env_json)
    .bind(&config.cwd)
    .bind(&config.auth_kind)
    .bind(&config.headers_json)
    .bind(&config.policy)
    .bind(enabled as i64)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::CoreError::NotFound("server".into()))?;
    Ok(row)
}

pub async fn set_enabled(pool: &SqlitePool, id: &str, enabled: bool) -> Result<()> {
    sqlx::query("UPDATE servers SET enabled = ?2 WHERE id = ?1")
        .bind(id)
        .bind(enabled as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM servers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub struct DiscoveryUpdate<'a> {
    pub protocol_version: Option<&'a str>,
    pub server_info_json: Option<&'a str>,
    pub capabilities_json: Option<&'a str>,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub oauth_issuer: Option<&'a str>,
    pub oauth_client_id: Option<&'a str>,
}

pub async fn record_discovery(
    pool: &SqlitePool,
    id: &str,
    update: DiscoveryUpdate<'_>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE servers SET
             protocol_version = COALESCE(?2, protocol_version),
             server_info_json = COALESCE(?3, server_info_json),
             capabilities_json = COALESCE(?4, capabilities_json),
             last_status = ?5,
             last_error = ?6,
             last_checked_at = ?7,
             oauth_issuer = COALESCE(?8, oauth_issuer),
             oauth_client_id = COALESCE(?9, oauth_client_id),
             negotiated_at = CASE WHEN ?2 IS NULL THEN negotiated_at ELSE ?7 END
           WHERE id = ?1"#,
    )
    .bind(id)
    .bind(update.protocol_version)
    .bind(update.server_info_json)
    .bind(update.capabilities_json)
    .bind(update.status)
    .bind(update.error)
    .bind(now_ms())
    .bind(update.oauth_issuer)
    .bind(update.oauth_client_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_error(pool: &SqlitePool, id: &str, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE servers SET last_status = 'error', last_error = ?2, last_checked_at = ?3 WHERE id = ?1",
    )
    .bind(id)
    .bind(error)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cache_tools(pool: &SqlitePool, id: &str, tools_json: &str) -> Result<()> {
    sqlx::query("UPDATE servers SET tools_json = ?2, tools_cached_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(tools_json)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cached_tools(pool: &SqlitePool, id: &str) -> Result<Option<(String, i64)>> {
    let row: Option<(Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT tools_json, tools_cached_at FROM servers WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((Some(json), Some(at))) => Some((json, at)),
        _ => None,
    })
}

pub async fn clear_tools(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE servers SET tools_json = NULL, tools_cached_at = NULL WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM servers").execute(pool).await?;
    Ok(())
}

pub async fn set_transport(pool: &SqlitePool, id: &str, transport: &str) -> Result<()> {
    sqlx::query("UPDATE servers SET transport = ?2 WHERE id = ?1")
        .bind(id)
        .bind(transport)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn server_crud_and_tool_cache() {
        let db = Db::open_memory().await.unwrap();
        let s = create(
            &db.pool,
            &ServerConfig::http(
                "Mock",
                "mock",
                "http://127.0.0.1:8787/mcp",
                "streamable_http",
                "none",
                "{}",
                "ask",
            ),
        )
        .await
        .unwrap();
        assert_eq!(list(&db.pool).await.unwrap().len(), 1);

        cache_tools(&db.pool, &s.id, r#"[{"name":"echo"}]"#)
            .await
            .unwrap();
        let (json, _) = cached_tools(&db.pool, &s.id).await.unwrap().unwrap();
        assert!(json.contains("echo"));

        clear_tools(&db.pool, &s.id).await.unwrap();
        assert!(cached_tools(&db.pool, &s.id).await.unwrap().is_none());

        record_discovery(
            &db.pool,
            &s.id,
            DiscoveryUpdate {
                protocol_version: Some("2026-07-28"),
                server_info_json: Some(r#"{"name":"mock"}"#),
                capabilities_json: Some(r#"{"tools":{}}"#),
                status: "connected",
                error: None,
                oauth_issuer: None,
                oauth_client_id: None,
            },
        )
        .await
        .unwrap();
        let loaded = get(&db.pool, &s.id).await.unwrap();
        assert_eq!(loaded.protocol_version.as_deref(), Some("2026-07-28"));
        assert_eq!(loaded.last_status.as_deref(), Some("connected"));
    }

    #[tokio::test]
    async fn stdio_server_fields_roundtrip() {
        let db = Db::open_memory().await.unwrap();
        let created = create(
            &db.pool,
            &ServerConfig {
                name: "Local".into(),
                slug: "local".into(),
                url: String::new(),
                transport: "stdio".into(),
                command: "npx".into(),
                args_json: r#"["-y","pkg"]"#.into(),
                env_json: r#"{"TOKEN":"value"}"#.into(),
                cwd: "/tmp".into(),
                auth_kind: "none".into(),
                headers_json: "{}".into(),
                policy: "ask".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.transport, "stdio");
        assert_eq!(created.command, "npx");
        assert_eq!(created.args_json, r#"["-y","pkg"]"#);
        assert_eq!(created.env_json, r#"{"TOKEN":"value"}"#);
        assert_eq!(created.cwd, "/tmp");

        let mut config = ServerConfig {
            name: "Local edited".into(),
            slug: "local".into(),
            url: String::new(),
            transport: "stdio".into(),
            command: "uvx".into(),
            args_json: r#"["server"]"#.into(),
            env_json: "{}".into(),
            cwd: String::new(),
            auth_kind: "none".into(),
            headers_json: "{}".into(),
            policy: "auto".into(),
        };
        let updated = update(&db.pool, &created.id, &config, true).await.unwrap();
        assert_eq!(updated.name, "Local edited");
        assert_eq!(updated.command, "uvx");
        assert_eq!(updated.policy, "auto");

        config.transport = "streamable_http".into();
        config.url = "https://example.com/mcp".into();
        config.command.clear();
        let updated = update(&db.pool, &created.id, &config, true).await.unwrap();
        assert_eq!(updated.transport, "streamable_http");
        assert_eq!(updated.url, "https://example.com/mcp");
        assert!(updated.command.is_empty());
    }
}
