use sqlx::SqlitePool;

use crate::models::AcpAgent;
use crate::{CoreError, Result, new_id, now_ms};

/// A registerable external ACP agent.
///
/// `env_json` holds environment variables for the child process. The UI never
/// echoes its values back; treat it as secret material stored in SQLite.
#[derive(Debug, Clone)]
pub struct AcpAgentConfig {
    pub name: String,
    pub slug: String,
    pub command: String,
    pub args_json: String,
    pub env_json: String,
    pub permission_policy: String,
    pub enabled: bool,
}

impl AcpAgentConfig {
    pub fn new(
        name: impl Into<String>,
        slug: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            slug: slug.into(),
            command: command.into(),
            args_json: "[]".into(),
            env_json: "{}".into(),
            permission_policy: PermissionPolicy::Ask.as_str().into(),
            enabled: true,
        }
    }

    pub fn args(mut self, args: impl Into<String>) -> Self {
        self.args_json = args.into();
        self
    }

    pub fn env(mut self, env: impl Into<String>) -> Self {
        self.env_json = env.into();
        self
    }

    pub fn policy(mut self, policy: PermissionPolicy) -> Self {
        self.permission_policy = policy.as_str().into();
        self
    }
}

/// How an ACP permission request is answered.
///
/// ACP permission is advisory: an agent that never asks is free to act. These
/// policies only decide what Jarvis answers when asked, and `AutoReads` trusts
/// the agent's self-reported `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Forward every request to the user.
    Ask,
    /// Auto-allow read/search/think; ask for everything else.
    AutoReads,
}

impl PermissionPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AutoReads => "auto_reads",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "ask" => Ok(Self::Ask),
            "auto_reads" => Ok(Self::AutoReads),
            other => Err(CoreError::invalid(format!(
                "unknown ACP permission policy: {other}"
            ))),
        }
    }
}

/// The kind of action an ACP agent is asking permission for.
///
/// The agent reports this itself; it is not a security boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    Read,
    Search,
    Think,
    Edit,
    Delete,
    Move,
    Execute,
    Other,
}

impl PermissionKind {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("read") => Self::Read,
            Some("search") => Self::Search,
            Some("think") => Self::Think,
            Some("edit") => Self::Edit,
            Some("delete") => Self::Delete,
            Some("move") => Self::Move,
            Some("execute") => Self::Execute,
            _ => Self::Other,
        }
    }

    /// Actions that never change files or run commands.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Read | Self::Search | Self::Think)
    }
}

/// Resolve a policy and an action kind to a yes/no decision, before the user is
/// involved. `false` means "ask the user".
pub fn auto_decide(policy: PermissionPolicy, kind: PermissionKind) -> bool {
    match policy {
        PermissionPolicy::Ask => false,
        PermissionPolicy::AutoReads => kind.is_read_only(),
    }
}

/// Shape of a permission option as advertised by the agent. Only the fields the
/// decision needs are modelled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: String,
}

/// What to answer for a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Select the option with this id.
    Select(String),
    /// The agent offered no option Jarvis is willing to pick.
    Cancel,
}

/// Pick an option from what the agent offered.
///
/// `approve` is the decision already made (by policy or by the user).
///
/// - approve: prefer `allow_once`. Never pick `allow_always`, which would
///   widen a rule silently.
/// - reject: prefer `reject_once`, then `reject_always`, then cancel.
/// - If the agent only offers `allow_always`, cancel instead of silently
///   choosing it.
pub fn select_option(options: &[PermissionOption], approve: bool) -> PermissionOutcome {
    let find = |kind: &str| {
        options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| option.option_id.clone())
    };

    if approve {
        if let Some(id) = find("allow_once") {
            return PermissionOutcome::Select(id);
        }
        // An ACP `allow_always` must not become a persisted rule, so a
        // request that only offers it is cancelled rather than approved.
        if find("allow_always").is_some() {
            return PermissionOutcome::Cancel;
        }
        return PermissionOutcome::Cancel;
    }

    if let Some(id) = find("reject_once") {
        return PermissionOutcome::Select(id);
    }
    if let Some(id) = find("reject_always") {
        return PermissionOutcome::Select(id);
    }
    PermissionOutcome::Cancel
}

pub async fn create(pool: &SqlitePool, config: &AcpAgentConfig) -> Result<AcpAgent> {
    validate(config)?;
    let now = now_ms();
    let row = sqlx::query_as::<_, AcpAgent>(
        r#"INSERT INTO acp_agents
           (id, name, slug, command, args_json, env_json, permission_policy, enabled, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
           RETURNING *"#,
    )
    .bind(new_id())
    .bind(&config.name)
    .bind(&config.slug)
    .bind(&config.command)
    .bind(&config.args_json)
    .bind(&config.env_json)
    .bind(&config.permission_policy)
    .bind(config.enabled)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn update(pool: &SqlitePool, id: &str, config: &AcpAgentConfig) -> Result<AcpAgent> {
    validate(config)?;
    let row = sqlx::query_as::<_, AcpAgent>(
        r#"UPDATE acp_agents
           SET name = ?2, slug = ?3, command = ?4, args_json = ?5, env_json = ?6,
               permission_policy = ?7, enabled = ?8, updated_at = ?9
           WHERE id = ?1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&config.name)
    .bind(&config.slug)
    .bind(&config.command)
    .bind(&config.args_json)
    .bind(&config.env_json)
    .bind(&config.permission_policy)
    .bind(config.enabled)
    .bind(now_ms())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| CoreError::NotFound("acp agent".into()))?;
    Ok(row)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<AcpAgent> {
    sqlx::query_as::<_, AcpAgent>("SELECT * FROM acp_agents WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CoreError::NotFound("acp agent".into()))
}

pub async fn get_by_slug(pool: &SqlitePool, slug: &str) -> Result<Option<AcpAgent>> {
    Ok(
        sqlx::query_as::<_, AcpAgent>("SELECT * FROM acp_agents WHERE slug = ?1")
            .bind(slug)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<AcpAgent>> {
    Ok(
        sqlx::query_as::<_, AcpAgent>("SELECT * FROM acp_agents ORDER BY name COLLATE NOCASE ASC")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn list_enabled(pool: &SqlitePool) -> Result<Vec<AcpAgent>> {
    Ok(sqlx::query_as::<_, AcpAgent>(
        "SELECT * FROM acp_agents WHERE enabled = 1 ORDER BY name COLLATE NOCASE ASC",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    let affected = sqlx::query("DELETE FROM acp_agents WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(CoreError::NotFound("acp agent".into()));
    }
    Ok(())
}

pub async fn set_health(pool: &SqlitePool, id: &str, error: Option<&str>) -> Result<AcpAgent> {
    let row = sqlx::query_as::<_, AcpAgent>(
        r#"UPDATE acp_agents
           SET last_error = ?2, last_checked_at = ?3
           WHERE id = ?1
           RETURNING *"#,
    )
    .bind(id)
    .bind(error)
    .bind(now_ms())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| CoreError::NotFound("acp agent".into()))?;
    Ok(row)
}

fn validate(config: &AcpAgentConfig) -> Result<()> {
    if config.name.trim().is_empty() {
        return Err(CoreError::invalid("agent name is required"));
    }
    if !is_valid_slug(&config.slug) {
        return Err(CoreError::invalid(
            "agent slug must be lowercase letters, digits and dashes",
        ));
    }
    if config.command.trim().is_empty() {
        return Err(CoreError::invalid("agent command is required"));
    }
    PermissionPolicy::parse(&config.permission_policy)?;
    parse_object_array(&config.args_json, "args_json")?;
    parse_object(&config.env_json, "env_json")?;
    Ok(())
}

pub fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

fn parse_object_array(value: &str, field: &str) -> Result<Vec<serde_json::Value>> {
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|_| CoreError::invalid(format!("{field} must be a JSON array")))?;
    match parsed {
        serde_json::Value::Array(items) if items.iter().all(|item| item.is_string()) => Ok(items),
        _ => Err(CoreError::invalid(format!(
            "{field} must be a JSON array of strings"
        ))),
    }
}

fn parse_object(value: &str, field: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|_| CoreError::invalid(format!("{field} must be a JSON object")))?;
    match parsed {
        serde_json::Value::Object(map) if map.values().all(serde_json::Value::is_string) => Ok(map),
        _ => Err(CoreError::invalid(format!(
            "{field} must be a JSON object of string values"
        ))),
    }
}

/// Parse the stored args into a command-line vector.
pub fn agent_args(agent: &AcpAgent) -> Result<Vec<String>> {
    let values = parse_object_array(&agent.args_json, "args_json")?;
    Ok(values
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect())
}

/// Parse the stored env into pairs.
pub fn agent_env(agent: &AcpAgent) -> Result<Vec<(String, String)>> {
    let map = parse_object(&agent.env_json, "env_json")?;
    Ok(map
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_owned())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn sample(slug: &str) -> AcpAgentConfig {
        AcpAgentConfig::new("OpenCode", slug, "opencode")
            .args(r#"["acp"]"#)
            .env(r#"{"OPENCODE_TOKEN": "secret"}"#)
    }

    #[tokio::test]
    async fn create_list_update_and_delete_agents() {
        let db = Db::open_memory().await.unwrap();
        let created = create(&db.pool, &sample("opencode")).await.unwrap();
        assert_eq!(created.slug, "opencode");
        assert_eq!(created.permission_policy, "ask");
        assert!(created.enabled);

        let config = AcpAgentConfig {
            name: "OpenCode local".into(),
            ..sample("opencode").policy(PermissionPolicy::AutoReads)
        };
        let updated = update(&db.pool, &created.id, &config).await.unwrap();
        assert_eq!(updated.name, "OpenCode local");
        assert_eq!(updated.permission_policy, "auto_reads");

        assert_eq!(list(&db.pool).await.unwrap().len(), 1);
        assert_eq!(list_enabled(&db.pool).await.unwrap().len(), 1);

        delete(&db.pool, &created.id).await.unwrap();
        assert!(list(&db.pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabled_agents_are_excluded_from_the_enabled_list() {
        let db = Db::open_memory().await.unwrap();
        let mut config = sample("codex");
        config.enabled = false;
        create(&db.pool, &config).await.unwrap();
        assert!(list(&db.pool).await.unwrap().len() == 1);
        assert!(list_enabled(&db.pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_invalid_configs() {
        let db = Db::open_memory().await.unwrap();
        let mut bad_slug = sample("Bad Slug");
        assert!(create(&db.pool, &bad_slug).await.is_err());
        bad_slug.slug = "good".into();
        bad_slug.command = "   ".into();
        assert!(create(&db.pool, &bad_slug).await.is_err());
        let mut bad_args = sample("good2");
        bad_args.args_json = r#"["acp", 3]"#.into();
        assert!(create(&db.pool, &bad_args).await.is_err());
        let mut bad_env = sample("good3");
        bad_env.env_json = r#"{"A": 3}"#.into();
        assert!(create(&db.pool, &bad_env).await.is_err());
        let mut bad_policy = sample("good4");
        bad_policy.permission_policy = "inherit_auto".into();
        assert!(create(&db.pool, &bad_policy).await.is_err());
    }

    #[tokio::test]
    async fn slugs_are_unique() {
        let db = Db::open_memory().await.unwrap();
        create(&db.pool, &sample("opencode")).await.unwrap();
        assert!(create(&db.pool, &sample("opencode")).await.is_err());
    }

    #[tokio::test]
    async fn parses_args_and_env_for_launch() {
        let db = Db::open_memory().await.unwrap();
        let agent = create(&db.pool, &sample("gemini")).await.unwrap();
        assert_eq!(agent_args(&agent).unwrap(), vec!["acp".to_owned()]);
        assert_eq!(
            agent_env(&agent).unwrap(),
            vec![("OPENCODE_TOKEN".to_owned(), "secret".to_owned())]
        );
    }

    #[tokio::test]
    async fn health_markers_are_recorded() {
        let db = Db::open_memory().await.unwrap();
        let agent = create(&db.pool, &sample("opencode")).await.unwrap();
        let updated = set_health(&db.pool, &agent.id, Some("binary not found"))
            .await
            .unwrap();
        assert_eq!(updated.last_error.as_deref(), Some("binary not found"));
        assert!(updated.last_checked_at.is_some());
    }

    #[test]
    fn auto_reads_only_covers_read_only_actions() {
        assert!(!auto_decide(PermissionPolicy::Ask, PermissionKind::Read));
        assert!(auto_decide(
            PermissionPolicy::AutoReads,
            PermissionKind::Read
        ));
        assert!(auto_decide(
            PermissionPolicy::AutoReads,
            PermissionKind::Search
        ));
        assert!(!auto_decide(
            PermissionPolicy::AutoReads,
            PermissionKind::Edit
        ));
        assert!(!auto_decide(
            PermissionPolicy::AutoReads,
            PermissionKind::Execute
        ));
        assert!(!auto_decide(
            PermissionPolicy::AutoReads,
            PermissionKind::Delete
        ));
    }

    #[test]
    fn never_picks_allow_always() {
        let only_always = vec![PermissionOption {
            option_id: "a".into(),
            kind: "allow_always".into(),
        }];
        assert_eq!(select_option(&only_always, true), PermissionOutcome::Cancel);

        let both = vec![
            PermissionOption {
                option_id: "a".into(),
                kind: "allow_always".into(),
            },
            PermissionOption {
                option_id: "b".into(),
                kind: "allow_once".into(),
            },
        ];
        assert_eq!(
            select_option(&both, true),
            PermissionOutcome::Select("b".into())
        );
    }

    #[test]
    fn rejection_prefers_reject_once_then_cancels() {
        let once = vec![PermissionOption {
            option_id: "r".into(),
            kind: "reject_once".into(),
        }];
        assert_eq!(
            select_option(&once, false),
            PermissionOutcome::Select("r".into())
        );
        assert_eq!(select_option(&[], false), PermissionOutcome::Cancel);
    }
}
