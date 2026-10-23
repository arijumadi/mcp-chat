use sqlx::SqlitePool;

use crate::models::{Conversation, DEFAULT_MAX_STEPS, SearchHit};
use crate::{CoreError, Result, new_id, now_ms};

pub fn title_from_text(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "New chat".into();
    }
    let mut title: String = compact.chars().take(60).collect();
    if compact.chars().count() > 60 {
        title.push('…');
    }
    title
}

pub async fn create(
    pool: &SqlitePool,
    title: &str,
    provider_id: Option<&str>,
    model: Option<&str>,
) -> Result<Conversation> {
    let id = new_id();
    let now = now_ms();
    let row = sqlx::query_as::<_, Conversation>(
        r#"INSERT INTO conversations
           (id, title, created_at, updated_at, provider_id, model, archived, max_steps)
           VALUES (?1, ?2, ?3, ?3, ?4, ?5, 0, ?6)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(title)
    .bind(now)
    .bind(provider_id)
    .bind(model)
    .bind(DEFAULT_MAX_STEPS)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Conversation> {
    sqlx::query_as::<_, Conversation>("SELECT * FROM conversations WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CoreError::NotFound("conversation".into()))
}

pub async fn list(pool: &SqlitePool, limit: i64, offset: i64) -> Result<Vec<Conversation>> {
    let rows = sqlx::query_as::<_, Conversation>(
        r#"SELECT * FROM conversations
           WHERE archived = 0
           ORDER BY updated_at DESC
           LIMIT ?1 OFFSET ?2"#,
    )
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Conversation>> {
    let rows = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE archived = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn update_title(pool: &SqlitePool, id: &str, title: &str) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CoreError::invalid("title cannot be empty"));
    }
    let title: String = title.chars().take(200).collect();
    sqlx::query("UPDATE conversations SET title = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(title)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn touch(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE conversations SET updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_model(
    pool: &SqlitePool,
    id: &str,
    provider_id: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE conversations SET provider_id = ?2, model = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(provider_id)
    .bind(model)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_system_prompt(pool: &SqlitePool, id: &str, prompt: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE conversations SET system_prompt = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(prompt)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_archived(pool: &SqlitePool, id: &str, archived: bool) -> Result<()> {
    sqlx::query("UPDATE conversations SET archived = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(id)
        .bind(archived as i64)
        .bind(now_ms())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM conversations WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM conversations")
        .execute(pool)
        .await?;
    Ok(())
}

fn fts_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    tokens.join(" ")
}

fn like_pattern(input: &str) -> String {
    let escaped = input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let fts = fts_query(query);
    let like = like_pattern(query);
    let rows = sqlx::query_as::<_, (String, String, String, i64)>(
        r#"SELECT c.id, c.title,
                  COALESCE((
                    SELECT snippet(messages_fts, 0, '', '', '…', 12)
                    FROM messages_fts
                    WHERE messages_fts MATCH ?1 AND messages_fts.conversation_id = c.id
                    LIMIT 1
                  ), '') AS snippet,
                  c.updated_at
           FROM conversations c
           WHERE c.archived = 0
             AND (
               c.title LIKE ?2 ESCAPE '\'
               OR c.id IN (
                 SELECT conversation_id FROM messages_fts WHERE messages_fts MATCH ?1
               )
             )
           ORDER BY c.updated_at DESC
           LIMIT ?3"#,
    )
    .bind(&fts)
    .bind(&like)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(conversation_id, title, snippet, updated_at)| SearchHit {
            conversation_id,
            title,
            snippet,
            updated_at,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn create_list_and_search_conversations() {
        let db = Db::open_memory().await.unwrap();
        let a = create(&db.pool, "Alpha", None, None).await.unwrap();
        let b = create(&db.pool, "Beta", None, None).await.unwrap();
        let threads = list(&db.pool, 20, 0).await.unwrap();
        assert_eq!(threads.len(), 2);
        assert!(threads.iter().any(|c| c.id == a.id));
        assert!(threads.iter().any(|c| c.id == b.id));

        update_title(&db.pool, &a.id, "Renamed thread")
            .await
            .unwrap();
        assert_eq!(get(&db.pool, &a.id).await.unwrap().title, "Renamed thread");

        let hits = search(&db.pool, "renamed", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation_id, a.id);

        delete(&db.pool, &b.id).await.unwrap();
        assert_eq!(list(&db.pool, 20, 0).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn message_text_is_searchable_and_removed_with_the_conversation() {
        let db = Db::open_memory().await.unwrap();
        let conversation = create(&db.pool, "Thread", None, None).await.unwrap();
        let message =
            crate::db::repo::messages::create(&db.pool, &conversation.id, "user", "completed")
                .await
                .unwrap();
        crate::db::repo::messages::append_part(
            &db.pool,
            &message.id,
            "text",
            &crate::models::PartPayload::Text {
                text: "a needle in a haystack".into(),
            },
        )
        .await
        .unwrap();
        let hits = search(&db.pool, "needle", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation_id, conversation.id);

        delete(&db.pool, &conversation.id).await.unwrap();
        assert!(search(&db.pool, "needle", 10).await.unwrap().is_empty());
    }

    #[test]
    fn title_is_trimmed_to_sixty_chars() {
        let title = title_from_text(&"word ".repeat(40));
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 61);
    }
}
