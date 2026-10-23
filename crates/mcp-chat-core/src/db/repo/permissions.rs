use sqlx::SqlitePool;

use crate::models::PermissionRule;
use crate::{Result, new_id, now_ms};

pub async fn create(
    pool: &SqlitePool,
    server_id: Option<&str>,
    action: &str,
    resource: &str,
    effect: &str,
) -> Result<PermissionRule> {
    let id = new_id();
    let row = sqlx::query_as::<_, PermissionRule>(
        r#"INSERT INTO permission_rules (id, server_id, action, resource, effect, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(server_id)
    .bind(action)
    .bind(resource)
    .bind(effect)
    .bind(now_ms())
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<PermissionRule>> {
    let rows = sqlx::query_as::<_, PermissionRule>(
        "SELECT * FROM permission_rules ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_for_server(pool: &SqlitePool, server_id: &str) -> Result<Vec<PermissionRule>> {
    let rows = sqlx::query_as::<_, PermissionRule>(
        "SELECT * FROM permission_rules WHERE server_id = ?1 OR server_id IS NULL ORDER BY created_at ASC",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM permission_rules WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM permission_rules")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::db::repo::servers;

    #[tokio::test]
    async fn permission_rules_roundtrip() {
        let db = Db::open_memory().await.unwrap();
        let server = servers::create(
            &db.pool,
            "Mock",
            "mock",
            "http://127.0.0.1:8787/mcp",
            "streamable_http",
            "none",
            "{}",
            "ask",
        )
        .await
        .unwrap();
        create(&db.pool, Some(&server.id), "mcp.mock.echo", "*", "allow")
            .await
            .unwrap();
        create(&db.pool, None, "mcp.*.delete", "*", "deny")
            .await
            .unwrap();
        assert_eq!(
            list_for_server(&db.pool, &server.id).await.unwrap().len(),
            2
        );
        delete_all(&db.pool).await.unwrap();
        assert!(list(&db.pool).await.unwrap().is_empty());
    }
}
