use std::collections::HashMap;

use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::models::{Message, MessagePage, MessageWithParts, Part, PartPayload};
use crate::{CoreError, Result, new_id, now_ms};

pub async fn create(
    pool: &SqlitePool,
    conversation_id: &str,
    role: &str,
    status: &str,
) -> Result<Message> {
    let id = new_id();
    let now = now_ms();
    let row = sqlx::query_as::<_, Message>(
        r#"INSERT INTO messages (id, conversation_id, role, created_at, status)
           VALUES (?1, ?2, ?3, ?4, ?5)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(conversation_id)
    .bind(role)
    .bind(now)
    .bind(status)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Message> {
    sqlx::query_as::<_, Message>("SELECT * FROM messages WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CoreError::NotFound("message".into()))
}

pub async fn append_part(
    pool: &SqlitePool,
    message_id: &str,
    kind: &str,
    payload: &PartPayload,
) -> Result<Part> {
    let mut tx = pool.begin().await?;
    let next: (i64,) =
        sqlx::query_as("SELECT COALESCE(MAX(idx), -1) + 1 FROM parts WHERE message_id = ?1")
            .bind(message_id)
            .fetch_one(&mut *tx)
            .await?;
    let id = new_id();
    let json = serde_json::to_string(payload)?;
    let part = sqlx::query_as::<_, Part>(
        r#"INSERT INTO parts (id, message_id, idx, kind, payload_json)
           VALUES (?1, ?2, ?3, ?4, ?5)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(message_id)
    .bind(next.0)
    .bind(kind)
    .bind(&json)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(part)
}

pub async fn append_raw_part(
    pool: &SqlitePool,
    message_id: &str,
    kind: &str,
    payload: &Value,
) -> Result<Part> {
    let mut tx = pool.begin().await?;
    let next: (i64,) =
        sqlx::query_as("SELECT COALESCE(MAX(idx), -1) + 1 FROM parts WHERE message_id = ?1")
            .bind(message_id)
            .fetch_one(&mut *tx)
            .await?;
    let id = new_id();
    let json = serde_json::to_string(payload)?;
    let part = sqlx::query_as::<_, Part>(
        r#"INSERT INTO parts (id, message_id, idx, kind, payload_json)
           VALUES (?1, ?2, ?3, ?4, ?5)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(message_id)
    .bind(next.0)
    .bind(kind)
    .bind(&json)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(part)
}

pub async fn update_part_payload(pool: &SqlitePool, part_id: &str, payload: &Value) -> Result<()> {
    let json = serde_json::to_string(payload)?;
    sqlx::query("UPDATE parts SET payload_json = ?2 WHERE id = ?1")
        .bind(part_id)
        .bind(json)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_part_text(pool: &SqlitePool, part_id: &str, text: &str) -> Result<()> {
    sqlx::query(
        "UPDATE parts SET payload_json = json_set(payload_json, '$.text', ?2) WHERE id = ?1",
    )
    .bind(part_id)
    .bind(text)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_status(
    pool: &SqlitePool,
    message_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE messages SET status = ?2, error = ?3 WHERE id = ?1")
        .bind(message_id)
        .bind(status)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn parts_for_message(pool: &SqlitePool, message_id: &str) -> Result<Vec<Part>> {
    let parts =
        sqlx::query_as::<_, Part>("SELECT * FROM parts WHERE message_id = ?1 ORDER BY idx ASC")
            .bind(message_id)
            .fetch_all(pool)
            .await?;
    Ok(parts)
}

pub async fn get_part(pool: &SqlitePool, part_id: &str) -> Result<Part> {
    sqlx::query_as::<_, Part>("SELECT * FROM parts WHERE id = ?1")
        .bind(part_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| CoreError::NotFound("part".into()))
}

pub async fn list_for_conversation(
    pool: &SqlitePool,
    conversation_id: &str,
) -> Result<Vec<MessageWithParts>> {
    let messages = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC, id ASC",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;

    let parts = sqlx::query_as::<_, Part>(
        r#"SELECT p.* FROM parts p
           JOIN messages m ON m.id = p.message_id
           WHERE m.conversation_id = ?1
           ORDER BY m.created_at ASC, m.id ASC, p.idx ASC"#,
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;

    let mut by_message: HashMap<String, Vec<Part>> = HashMap::new();
    for part in parts {
        by_message
            .entry(part.message_id.clone())
            .or_default()
            .push(part);
    }

    Ok(messages
        .into_iter()
        .map(|message| {
            let parts = by_message.remove(&message.id).unwrap_or_default();
            MessageWithParts { message, parts }
        })
        .collect())
}

pub async fn list_page_for_conversation(
    pool: &SqlitePool,
    conversation_id: &str,
    before: Option<&str>,
    limit: usize,
) -> Result<MessagePage> {
    let limit = limit.clamp(1, 500);
    let cursor = match before {
        Some(id) => {
            let message = get(pool, id).await?;
            if message.conversation_id != conversation_id {
                return Err(CoreError::invalid(
                    "message does not belong to conversation",
                ));
            }
            Some(message)
        }
        None => None,
    };

    let mut messages = if let Some(cursor) = &cursor {
        sqlx::query_as::<_, Message>(
            r#"SELECT * FROM messages
               WHERE conversation_id = ?1
                 AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))
               ORDER BY created_at DESC, id DESC
               LIMIT ?4"#,
        )
        .bind(conversation_id)
        .bind(cursor.created_at)
        .bind(&cursor.id)
        .bind((limit + 1) as i64)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Message>(
            r#"SELECT * FROM messages
               WHERE conversation_id = ?1
               ORDER BY created_at DESC, id DESC
               LIMIT ?2"#,
        )
        .bind(conversation_id)
        .bind((limit + 1) as i64)
        .fetch_all(pool)
        .await?
    };

    let has_more = messages.len() > limit;
    messages.truncate(limit);
    messages.reverse();
    if messages.is_empty() {
        return Ok(MessagePage {
            messages: Vec::new(),
            has_more: false,
        });
    }

    let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
    let mut query = QueryBuilder::<Sqlite>::new("SELECT * FROM parts WHERE message_id IN (");
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") ORDER BY idx ASC");
    let parts = query.build_query_as::<Part>().fetch_all(pool).await?;

    let mut by_message: HashMap<String, Vec<Part>> = HashMap::new();
    for part in parts {
        by_message
            .entry(part.message_id.clone())
            .or_default()
            .push(part);
    }

    Ok(MessagePage {
        messages: messages
            .into_iter()
            .map(|message| {
                let parts = by_message.remove(&message.id).unwrap_or_default();
                MessageWithParts { message, parts }
            })
            .collect(),
        has_more,
    })
}

pub async fn delete_message(pool: &SqlitePool, message_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM messages WHERE id = ?1")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a message and every message that follows it in the conversation.
/// Used by Regenerate.
pub async fn delete_from(pool: &SqlitePool, conversation_id: &str, message_id: &str) -> Result<()> {
    let message = get(pool, message_id).await?;
    if message.conversation_id != conversation_id {
        return Err(CoreError::invalid(
            "message does not belong to conversation",
        ));
    }
    sqlx::query(
        r#"DELETE FROM messages
           WHERE conversation_id = ?1
             AND (created_at > ?2 OR (created_at = ?2 AND id >= ?3))"#,
    )
    .bind(conversation_id)
    .bind(message.created_at)
    .bind(message.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove every message strictly after the given message. Used by Regenerate
/// while keeping the last user message.
pub async fn delete_after(
    pool: &SqlitePool,
    conversation_id: &str,
    message_id: &str,
) -> Result<()> {
    let message = get(pool, message_id).await?;
    if message.conversation_id != conversation_id {
        return Err(CoreError::invalid(
            "message does not belong to conversation",
        ));
    }
    sqlx::query(
        r#"DELETE FROM messages
           WHERE conversation_id = ?1
             AND (created_at > ?2 OR (created_at = ?2 AND id > ?3))"#,
    )
    .bind(conversation_id)
    .bind(message.created_at)
    .bind(message.id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn last_message(pool: &SqlitePool, conversation_id: &str) -> Result<Option<Message>> {
    let row = sqlx::query_as::<_, Message>(
        r#"SELECT * FROM messages WHERE conversation_id = ?1
           ORDER BY created_at DESC, id DESC LIMIT 1"#,
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// A dangling tool_use without a tool_result makes most providers reject the
/// whole history. Run this once at startup, before any request.
pub async fn sweep_interrupted(pool: &SqlitePool) -> Result<usize> {
    let now = now_ms();
    let parts = sqlx::query(
        r#"UPDATE parts
           SET payload_json = json_set(
             payload_json,
             '$.state', 'aborted',
             '$.endedAt', ?1,
             '$.error', 'App closed before the tool finished'
           )
           WHERE kind = 'tool_call'
             AND json_extract(payload_json, '$.state') IN ('streaming', 'running')"#,
    )
    .bind(now)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"UPDATE messages
           SET status = 'interrupted',
               error = COALESCE(error, 'Interrupted when the app closed')
           WHERE status IN ('streaming', 'running')"#,
    )
    .execute(pool)
    .await?;

    Ok(parts.rows_affected() as usize)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::Db;
    use crate::db::repo::conversations;

    #[tokio::test]
    async fn persists_messages_and_ordered_parts() {
        let db = Db::open_memory().await.unwrap();
        let conv = conversations::create(&db.pool, "Test", None, None)
            .await
            .unwrap();
        let msg = create(&db.pool, &conv.id, "assistant", "streaming")
            .await
            .unwrap();
        let p1 = append_part(
            &db.pool,
            &msg.id,
            "text",
            &PartPayload::Text {
                text: "hello".into(),
            },
        )
        .await
        .unwrap();
        let p2 = append_part(
            &db.pool,
            &msg.id,
            "tool_call",
            &PartPayload::ToolCall {
                call_id: "call_1".into(),
                name: "echo".into(),
                server: "mock".into(),
                args: json!({"text": "hi"}),
                state: "running".into(),
                started_at: Some(now_ms()),
                ended_at: None,
                executed: false,
                approval: None,
                error: None,
                result_preview: None,
                full_result_ref: None,
                truncated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(p1.idx, 0);
        assert_eq!(p2.idx, 1);

        set_status(&db.pool, &msg.id, "completed", None)
            .await
            .unwrap();
        let history = list_for_conversation(&db.pool, &conv.id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].parts.len(), 2);
        assert_eq!(history[0].message.status, "completed");
    }

    #[tokio::test]
    async fn sweep_marks_running_tools_aborted() {
        let db = Db::open_memory().await.unwrap();
        let conv = conversations::create(&db.pool, "Test", None, None)
            .await
            .unwrap();
        let msg = create(&db.pool, &conv.id, "assistant", "streaming")
            .await
            .unwrap();
        append_part(
            &db.pool,
            &msg.id,
            "tool_call",
            &PartPayload::ToolCall {
                call_id: "call_1".into(),
                name: "echo".into(),
                server: "mock".into(),
                args: json!({}),
                state: "running".into(),
                started_at: Some(now_ms()),
                ended_at: None,
                executed: false,
                approval: None,
                error: None,
                result_preview: None,
                full_result_ref: None,
                truncated: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(sweep_interrupted(&db.pool).await.unwrap(), 1);
        let parts = parts_for_message(&db.pool, &msg.id).await.unwrap();
        let payload = parts[0].payload();
        assert_eq!(payload["state"], "aborted");
        assert_eq!(get(&db.pool, &msg.id).await.unwrap().status, "interrupted");
    }

    #[tokio::test]
    async fn pages_messages_with_a_stable_cursor_and_parts() {
        let db = Db::open_memory().await.unwrap();
        let conv = conversations::create(&db.pool, "Paged", None, None)
            .await
            .unwrap();
        for index in 0..5 {
            let message = create(&db.pool, &conv.id, "assistant", "completed")
                .await
                .unwrap();
            append_part(
                &db.pool,
                &message.id,
                "text",
                &PartPayload::Text {
                    text: format!("message {index}"),
                },
            )
            .await
            .unwrap();
        }

        let first = list_page_for_conversation(&db.pool, &conv.id, None, 2)
            .await
            .unwrap();
        assert_eq!(first.messages.len(), 2);
        assert!(first.has_more);
        assert!(
            first
                .messages
                .iter()
                .all(|message| !message.parts.is_empty())
        );

        let cursor = first.messages[0].message.id.clone();
        let second = list_page_for_conversation(&db.pool, &conv.id, Some(&cursor), 2)
            .await
            .unwrap();
        assert_eq!(second.messages.len(), 2);
        assert!(second.has_more);
        assert!(first.messages.iter().all(|message| {
            second
                .messages
                .iter()
                .all(|item| item.message.id != message.message.id)
        }));

        let cursor = second.messages[0].message.id.clone();
        let third = list_page_for_conversation(&db.pool, &conv.id, Some(&cursor), 2)
            .await
            .unwrap();
        assert_eq!(third.messages.len(), 1);
        assert!(!third.has_more);
        let total = first.messages.len() + second.messages.len() + third.messages.len();
        assert_eq!(total, 5);
    }
}
