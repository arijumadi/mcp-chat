use sqlx::SqlitePool;

use crate::Result;

pub async fn selected(pool: &SqlitePool, conversation_id: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT server_id FROM conversation_servers WHERE conversation_id = ?1")
            .bind(conversation_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn set(pool: &SqlitePool, conversation_id: &str, server_ids: &[String]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM conversation_servers WHERE conversation_id = ?1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    for server_id in server_ids {
        sqlx::query(
            "INSERT OR IGNORE INTO conversation_servers (conversation_id, server_id) VALUES (?1, ?2)",
        )
        .bind(conversation_id)
        .bind(server_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE conversations SET servers_configured = 1 WHERE id = ?1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn reset_to_all(pool: &SqlitePool, conversation_id: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM conversation_servers WHERE conversation_id = ?1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE conversations SET servers_configured = 0 WHERE id = ?1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::db::repo::{conversations, servers};

    #[tokio::test]
    async fn selection_defaults_to_all_and_can_be_empty_or_explicit() {
        let db = Db::open_memory().await.unwrap();
        let conversation = conversations::create(&db.pool, "Chat", None, None)
            .await
            .unwrap();
        let a = servers::create(
            &db.pool,
            &servers::ServerConfig::http(
                "A",
                "a",
                "https://a.example/mcp",
                "streamable_http",
                "none",
                "{}",
                "ask",
            ),
        )
        .await
        .unwrap();
        let b = servers::create(
            &db.pool,
            &servers::ServerConfig::http(
                "B",
                "b",
                "https://b.example/mcp",
                "streamable_http",
                "none",
                "{}",
                "ask",
            ),
        )
        .await
        .unwrap();

        let all = conversations::selected_server_ids(&db.pool, &conversation.id)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        set(&db.pool, &conversation.id, std::slice::from_ref(&b.id))
            .await
            .unwrap();
        let selected = conversations::selected_server_ids(&db.pool, &conversation.id)
            .await
            .unwrap();
        assert_eq!(selected, vec![b.id.clone()]);

        set(&db.pool, &conversation.id, &[]).await.unwrap();
        assert!(
            conversations::selected_server_ids(&db.pool, &conversation.id)
                .await
                .unwrap()
                .is_empty()
        );

        reset_to_all(&db.pool, &conversation.id).await.unwrap();
        let all = conversations::selected_server_ids(&db.pool, &conversation.id)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a.id));
    }
}
