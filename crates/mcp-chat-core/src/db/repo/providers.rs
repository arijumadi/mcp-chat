use sqlx::SqlitePool;

use crate::models::Provider;
use crate::{Result, new_id, now_ms};

#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &SqlitePool,
    name: &str,
    kind: &str,
    base_url: &str,
    model: &str,
    api_key_ref: Option<&str>,
    context_window: i64,
    max_output_tokens: i64,
) -> Result<Provider> {
    let id = new_id();
    let now = now_ms();
    let row = sqlx::query_as::<_, Provider>(
        r#"INSERT INTO providers
           (id, name, kind, base_url, model, api_key_ref, context_window, max_output_tokens, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(name)
    .bind(kind)
    .bind(base_url)
    .bind(model)
    .bind(api_key_ref)
    .bind(context_window)
    .bind(max_output_tokens)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Provider>> {
    let rows = sqlx::query_as::<_, Provider>("SELECT * FROM providers ORDER BY created_at ASC")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Provider> {
    sqlx::query_as::<_, Provider>("SELECT * FROM providers WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| crate::CoreError::NotFound("provider".into()))
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    kind: &str,
    base_url: &str,
    model: &str,
    context_window: i64,
    max_output_tokens: i64,
) -> Result<Provider> {
    let row = sqlx::query_as::<_, Provider>(
        r#"UPDATE providers SET
             name = ?2, kind = ?3, base_url = ?4, model = ?5,
             context_window = ?6, max_output_tokens = ?7, updated_at = ?8
           WHERE id = ?1
           RETURNING *"#,
    )
    .bind(id)
    .bind(name)
    .bind(kind)
    .bind(base_url)
    .bind(model)
    .bind(context_window)
    .bind(max_output_tokens)
    .bind(now_ms())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::CoreError::NotFound("provider".into()))?;
    Ok(row)
}

pub async fn set_api_key_ref(pool: &SqlitePool, id: &str, key_ref: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE providers SET api_key_ref = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(key_ref)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM providers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM providers").execute(pool).await?;
    Ok(())
}

pub async fn clear_conversation_refs(pool: &SqlitePool, provider_id: &str) -> Result<()> {
    sqlx::query("UPDATE conversations SET provider_id = NULL WHERE provider_id = ?1")
        .bind(provider_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn provider_crud() {
        let db = Db::open_memory().await.unwrap();
        let p = create(
            &db.pool,
            "Local",
            "openai_compat",
            "http://127.0.0.1:1234/v1",
            "test-model",
            None,
            128000,
            8192,
        )
        .await
        .unwrap();
        assert_eq!(list(&db.pool).await.unwrap().len(), 1);
        let updated = update(
            &db.pool,
            &p.id,
            "Local 2",
            "anthropic",
            "https://api.anthropic.com",
            "claude",
            200000,
            16000,
        )
        .await
        .unwrap();
        assert_eq!(updated.name, "Local 2");
        assert_eq!(updated.kind, "anthropic");
        delete(&db.pool, &p.id).await.unwrap();
        assert!(list(&db.pool).await.unwrap().is_empty());
    }
}
