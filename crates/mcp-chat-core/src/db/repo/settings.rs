use sqlx::SqlitePool;

use crate::Result;

pub async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(value,)| value))
}

pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO settings (key, value) VALUES (?1, ?2)
           ON CONFLICT(key) DO UPDATE SET value = excluded.value"#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn all(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn delete(pool: &SqlitePool, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_i64(pool: &SqlitePool, key: &str, default: i64) -> Result<i64> {
    Ok(get(pool, key)
        .await?
        .and_then(|value| value.parse().ok())
        .unwrap_or(default))
}

pub async fn get_bool(pool: &SqlitePool, key: &str, default: bool) -> Result<bool> {
    Ok(get(pool, key)
        .await?
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(default))
}

pub async fn delete_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM settings").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn settings_roundtrip() {
        let db = Db::open_memory().await.unwrap();
        set(&db.pool, "theme", "dark").await.unwrap();
        assert_eq!(
            get(&db.pool, "theme").await.unwrap().as_deref(),
            Some("dark")
        );
        set(&db.pool, "theme", "light").await.unwrap();
        assert_eq!(
            get(&db.pool, "theme").await.unwrap().as_deref(),
            Some("light")
        );
        assert_eq!(get_i64(&db.pool, "missing", 7).await.unwrap(), 7);
        assert!(get_bool(&db.pool, "missing", true).await.unwrap());
    }
}
