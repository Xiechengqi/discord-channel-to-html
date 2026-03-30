use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::db::MessageStore;
use crate::errors::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct ChannelInfo {
    pub channel_id: String,
    pub name: String,
    pub channel_type: String,
    pub channel_url: String,
    pub monitored: bool,
}

/// Manages the server-level DB (channel list + monitoring state)
/// and per-channel MessageStore instances.
pub struct ServerStore {
    conn: Mutex<Connection>,
    data_dir: PathBuf,
    stores: Mutex<HashMap<String, Arc<MessageStore>>>,
}

impl ServerStore {
    pub fn new(data_dir: &str) -> AppResult<Self> {
        let data_dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| AppError::DatabaseError(format!("create data dir: {e}")))?;

        let db_path = data_dir.join("server.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS channels (
                 channel_id   TEXT PRIMARY KEY,
                 name         TEXT NOT NULL,
                 channel_type TEXT NOT NULL DEFAULT 'Text',
                 channel_url  TEXT NOT NULL,
                 monitored    INTEGER NOT NULL DEFAULT 0,
                 created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
             );"
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(Self {
            conn: Mutex::new(conn),
            data_dir,
            stores: Mutex::new(HashMap::new()),
        })
    }

    /// Upsert channel list from Discord DOM scrape.
    /// Updates name/type/url if the channel already exists; does not touch `monitored`.
    pub fn upsert_channels(&self, channels: &[ChannelInfo]) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut stmt = conn.prepare_cached(
            "INSERT INTO channels (channel_id, name, channel_type, channel_url, monitored)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(channel_id) DO UPDATE SET
                 name = excluded.name,
                 channel_type = excluded.channel_type,
                 channel_url = excluded.channel_url,
                 updated_at = datetime('now')"
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;

        for ch in channels {
            stmt.execute(params![ch.channel_id, ch.name, ch.channel_type, ch.channel_url])
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        }
        Ok(())
    }

    /// Get all channels.
    pub fn get_all_channels(&self) -> AppResult<Vec<ChannelInfo>> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT channel_id, name, channel_type, channel_url, monitored FROM channels ORDER BY name"
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let rows = stmt.query_map([], |row| {
            Ok(ChannelInfo {
                channel_id: row.get(0)?,
                name: row.get(1)?,
                channel_type: row.get(2)?,
                channel_url: row.get(3)?,
                monitored: row.get::<_, i32>(4)? != 0,
            })
        }).map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let mut channels = Vec::new();
        for row in rows {
            channels.push(row.map_err(|e| AppError::DatabaseError(e.to_string()))?);
        }
        Ok(channels)
    }

    /// Get only monitored channels.
    pub fn get_monitored_channels(&self) -> AppResult<Vec<ChannelInfo>> {
        let all = self.get_all_channels()?;
        Ok(all.into_iter().filter(|c| c.monitored).collect())
    }

    /// Set monitored=1 for the given channel IDs, monitored=0 for all others.
    pub fn set_monitored(&self, channel_ids: &[String]) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        conn.execute("UPDATE channels SET monitored = 0, updated_at = datetime('now')", [])
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        let mut stmt = conn.prepare_cached(
            "UPDATE channels SET monitored = 1, updated_at = datetime('now') WHERE channel_id = ?1"
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;
        for id in channel_ids {
            stmt.execute(params![id])
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        }
        Ok(())
    }

    /// Add a single channel to monitoring.
    pub fn add_monitored(&self, channel_id: &str) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        conn.execute(
            "UPDATE channels SET monitored = 1, updated_at = datetime('now') WHERE channel_id = ?1",
            params![channel_id],
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    /// Remove a single channel from monitoring (keeps data).
    pub fn remove_monitored(&self, channel_id: &str) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        conn.execute(
            "UPDATE channels SET monitored = 0, updated_at = datetime('now') WHERE channel_id = ?1",
            params![channel_id],
        ).map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    /// Get or create a per-channel MessageStore.
    pub fn get_message_store(&self, channel_id: &str) -> AppResult<Arc<MessageStore>> {
        let mut stores = self.stores.lock().map_err(|e| AppError::DatabaseError(e.to_string()))?;
        if let Some(store) = stores.get(channel_id) {
            return Ok(store.clone());
        }
        let db_path = self.channel_db_path(channel_id);
        let store = Arc::new(MessageStore::new(db_path.to_str().unwrap_or("messages.db"))?);
        stores.insert(channel_id.to_string(), store.clone());
        Ok(store)
    }

    /// Get the per-channel message count (0 if DB doesn't exist yet).
    pub fn channel_message_count(&self, channel_id: &str) -> u64 {
        if let Ok(store) = self.get_message_store(channel_id) {
            store.count().unwrap_or(0)
        } else {
            0
        }
    }

    /// Delete a channel's message DB (for resync).
    pub fn clear_channel_data(&self, channel_id: &str) -> AppResult<()> {
        // Remove from cache
        if let Ok(mut stores) = self.stores.lock() {
            stores.remove(channel_id);
        }
        let path = self.channel_db_path(channel_id);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| AppError::DatabaseError(format!("remove channel db: {e}")))?;
            // Also remove WAL/SHM files
            let _ = std::fs::remove_file(path.with_extension("db-wal"));
            let _ = std::fs::remove_file(path.with_extension("db-shm"));
        }
        Ok(())
    }

    fn channel_db_path(&self, channel_id: &str) -> PathBuf {
        self.data_dir.join(format!("{}.db", channel_id))
    }
}
