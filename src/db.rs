use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::errors::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct StoredMessage {
    pub id: i64,
    pub author: String,
    pub timestamp: String,
    pub content: String,
    pub scraped_at: String,
}

#[derive(Debug, Clone)]
pub struct ScrapedMessage {
    pub author: String,
    pub timestamp: String,
    pub content: String,
    pub dedup_hash: String,
}

impl ScrapedMessage {
    /// Create a new scraped message with dedup hash.
    ///
    /// Dedup strategy:
    /// - If Discord's message ID is available (extracted from DOM `id="message-content-{id}"`),
    ///   use it directly — it's globally unique and stable.
    /// - Otherwise, fall back to SHA-256(author + timestamp + full content).
    ///   We hash the FULL content (not truncated) to avoid collisions when two users
    ///   post messages with the same prefix at the same timestamp.
    pub fn new(author: String, timestamp: String, content: String, msg_id: String) -> Self {
        let dedup_hash = if !msg_id.is_empty() {
            // Discord message IDs are snowflakes — globally unique
            format!("discord:{}", msg_id)
        } else {
            let hash_input = format!("{}|{}|{}", author, timestamp, content);
            let mut hasher = Sha256::new();
            hasher.update(hash_input.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        Self {
            author,
            timestamp,
            content,
            dedup_hash,
        }
    }
}

pub struct MessageStore {
    conn: Arc<Mutex<Connection>>,
}

impl MessageStore {
    pub fn new(path: &str) -> AppResult<Self> {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;

             CREATE TABLE IF NOT EXISTS messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 dedup_hash TEXT NOT NULL UNIQUE,
                 author TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 content TEXT NOT NULL,
                 scraped_at TEXT NOT NULL DEFAULT (datetime('now'))
             );

             CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);",
        )
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn insert_batch(&self, msgs: &[ScrapedMessage]) -> AppResult<usize> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut count = 0;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO messages (dedup_hash, author, timestamp, content)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

            for msg in msgs {
                let rows = stmt
                    .execute(params![msg.dedup_hash, msg.author, msg.timestamp, msg.content])
                    .map_err(|e| AppError::DatabaseError(e.to_string()))?;
                count += rows;
            }
        }

        tx.commit().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(count)
    }

    /// Fetch messages older than `before_id` (exclusive), newest-first, up to `limit`.
    /// Returns in chronological order (oldest first).
    pub fn get_before_id(&self, before_id: i64, limit: u32) -> AppResult<Vec<StoredMessage>> {
        let limit = limit.min(200);
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, author, timestamp, content, scraped_at FROM messages \
                 WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![before_id, limit], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    author: row.get(1)?,
                    timestamp: row.get(2)?,
                    content: row.get(3)?,
                    scraped_at: row.get(4)?,
                })
            })
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| AppError::DatabaseError(e.to_string()))?);
        }
        messages.reverse(); // chronological order
        Ok(messages)
    }

    pub fn get_messages(
        &self,
        before: Option<&str>,
        after: Option<&str>,
        limit: u32,
    ) -> AppResult<Vec<StoredMessage>> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let limit = limit.min(500);

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (before, after) {
                (Some(b), Some(a)) => (
                    "SELECT id, author, timestamp, content, scraped_at FROM messages \
                     WHERE timestamp < ?1 AND timestamp > ?2 \
                     ORDER BY id DESC LIMIT ?3"
                        .to_string(),
                    vec![
                        Box::new(b.to_string()),
                        Box::new(a.to_string()),
                        Box::new(limit),
                    ],
                ),
                (Some(b), None) => (
                    "SELECT id, author, timestamp, content, scraped_at FROM messages \
                     WHERE timestamp < ?1 \
                     ORDER BY id DESC LIMIT ?2"
                        .to_string(),
                    vec![Box::new(b.to_string()), Box::new(limit)],
                ),
                (None, Some(a)) => (
                    "SELECT id, author, timestamp, content, scraped_at FROM messages \
                     WHERE timestamp > ?1 \
                     ORDER BY id DESC LIMIT ?2"
                        .to_string(),
                    vec![Box::new(a.to_string()), Box::new(limit)],
                ),
                (None, None) => (
                    "SELECT id, author, timestamp, content, scraped_at FROM messages \
                     ORDER BY id DESC LIMIT ?1"
                        .to_string(),
                    vec![Box::new(limit)],
                ),
            };

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare_cached(&sql)
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    author: row.get(1)?,
                    timestamp: row.get(2)?,
                    content: row.get(3)?,
                    scraped_at: row.get(4)?,
                })
            })
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| AppError::DatabaseError(e.to_string()))?);
        }
        messages.reverse(); // chronological order
        Ok(messages)
    }

    pub fn get_latest(&self, n: u32) -> AppResult<Vec<StoredMessage>> {
        let n = n.min(500);
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, author, timestamp, content, scraped_at FROM messages \
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![n], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    author: row.get(1)?,
                    timestamp: row.get(2)?,
                    content: row.get(3)?,
                    scraped_at: row.get(4)?,
                })
            })
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| AppError::DatabaseError(e.to_string()))?);
        }
        messages.reverse(); // chronological order
        Ok(messages)
    }

    /// Return the Discord snowflake ID of the most recently inserted message,
    /// or None if the DB is empty or the latest entry uses a SHA-256 hash.
    pub fn get_latest_discord_id(&self) -> AppResult<Option<String>> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let result = conn.query_row(
            "SELECT dedup_hash FROM messages ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(hash) => Ok(hash.strip_prefix("discord:").map(str::to_string)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::DatabaseError(e.to_string())),
        }
    }

    pub fn count(&self) -> AppResult<u64> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let count: u64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(count)
    }

    /// Delete all messages and reset the auto-increment counter.
    pub fn clear(&self) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        conn.execute_batch(
            "DELETE FROM messages;
             DELETE FROM sqlite_sequence WHERE name = 'messages';",
        )
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}
