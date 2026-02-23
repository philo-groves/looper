use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::model::{ExecutionResult, Percept, RecommendedAction};

/// One stored chat session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedChatSession {
    /// Stable chat identifier.
    pub id: String,
    /// Human-readable title derived from first message.
    pub title: String,
    /// Session creation timestamp (unix millis).
    pub created_at_unix_ms: i64,
    /// Last update timestamp (unix millis).
    pub updated_at_unix_ms: i64,
    /// Number of stored messages for this chat.
    pub message_count: usize,
}

/// One stored chat message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedChatMessage {
    /// Auto-incremented message id.
    pub id: i64,
    /// Chat session id.
    pub chat_id: String,
    /// Message role (`me` or `looper`).
    pub role: String,
    /// Message content.
    pub content: String,
    /// Message creation timestamp (unix millis).
    pub created_at_unix_ms: i64,
    /// Optional source iteration id for Looper responses.
    pub iteration_id: Option<i64>,
}

/// Persisted representation of one loop iteration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedIteration {
    /// Database id.
    pub id: i64,
    /// Unix timestamp in seconds.
    pub created_at_unix: i64,
    /// Percepts sensed in this iteration.
    pub sensed_percepts: Vec<Percept>,
    /// Surprising percepts in this iteration.
    pub surprising_percepts: Vec<Percept>,
    /// Planned actions.
    pub planned_actions: Vec<RecommendedAction>,
    /// Action execution results.
    pub action_results: Vec<ExecutionResult>,
}

/// SQLite-backed iteration store.
#[derive(Clone, Debug)]
pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    /// Creates and initializes a store at the given path.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let store = Self { path };
        store.initialize()?;
        Ok(store)
    }

    /// Inserts an iteration and returns its id.
    pub fn insert_iteration(&self, iteration: &PersistedIteration) -> Result<i64> {
        let conn = self.connection()?;
        let sensed_json = serde_json::to_string(&iteration.sensed_percepts)?;
        let surprising_json = serde_json::to_string(&iteration.surprising_percepts)?;
        let planned_json = serde_json::to_string(&iteration.planned_actions)?;
        let results_json = serde_json::to_string(&iteration.action_results)?;

        conn.execute(
            "INSERT INTO iterations (created_at_unix, sensed_percepts, surprising_percepts, planned_actions, action_results)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                iteration.created_at_unix,
                sensed_json,
                surprising_json,
                planned_json,
                results_json,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Fetches an iteration by id.
    pub fn get_iteration(&self, id: i64) -> Result<Option<PersistedIteration>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT id, created_at_unix, sensed_percepts, surprising_percepts, planned_actions, action_results
             FROM iterations WHERE id = ?1",
        )?;

        let mut rows = statement.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let sensed_raw: String = row.get(2)?;
        let surprising_raw: String = row.get(3)?;
        let planned_raw: String = row.get(4)?;
        let results_raw: String = row.get(5)?;

        Ok(Some(PersistedIteration {
            id: row.get(0)?,
            created_at_unix: row.get(1)?,
            sensed_percepts: serde_json::from_str(&sensed_raw)?,
            surprising_percepts: serde_json::from_str(&surprising_raw)?,
            planned_actions: serde_json::from_str(&planned_raw)?,
            action_results: serde_json::from_str(&results_raw)?,
        }))
    }

    /// Lists up to `limit` iterations with ids greater than `after_id`.
    pub fn list_iterations_after(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<PersistedIteration>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT id, created_at_unix, sensed_percepts, surprising_percepts, planned_actions, action_results
             FROM iterations
             WHERE (?1 IS NULL OR id > ?1)
             ORDER BY id ASC
             LIMIT ?2",
        )?;

        let rows = statement.query_map(params![after_id, limit as i64], |row| {
            let sensed_raw: String = row.get(2)?;
            let surprising_raw: String = row.get(3)?;
            let planned_raw: String = row.get(4)?;
            let results_raw: String = row.get(5)?;

            let iteration = PersistedIteration {
                id: row.get(0)?,
                created_at_unix: row.get(1)?,
                sensed_percepts: serde_json::from_str(&sensed_raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                surprising_percepts: serde_json::from_str(&surprising_raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                planned_actions: serde_json::from_str(&planned_raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                action_results: serde_json::from_str(&results_raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            };

            Ok(iteration)
        })?;

        let mut iterations = Vec::new();
        for row in rows {
            iterations.push(row?);
        }
        Ok(iterations)
    }

    /// Returns the latest stored iteration id, if any.
    pub fn latest_iteration_id(&self) -> Result<Option<i64>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT MAX(id) FROM iterations")?;
        let latest = statement.query_row([], |row| row.get(0))?;
        Ok(latest)
    }

    /// Returns up to `limit` previous windows of percept text.
    pub fn latest_percept_windows(&self, limit: usize) -> Result<Vec<Vec<String>>> {
        let conn = self.connection()?;
        let mut statement =
            conn.prepare("SELECT sensed_percepts FROM iterations ORDER BY id DESC LIMIT ?1")?;
        let rows = statement.query_map(params![limit as i64], |row| {
            let sensed_raw: String = row.get(0)?;
            Ok(sensed_raw)
        })?;

        let mut windows = Vec::new();
        for raw in rows {
            let sensed: Vec<Percept> = serde_json::from_str(&raw?)
                .map_err(|error| anyhow!("invalid stored percept payload: {error}"))?;
            windows.push(sensed.into_iter().map(|percept| percept.content).collect());
        }
        windows.reverse();
        Ok(windows)
    }

    /// Appends one chat message and ensures the chat session exists.
    pub fn insert_chat_message(
        &self,
        chat_id: &str,
        role: &str,
        content: &str,
        iteration_id: Option<i64>,
    ) -> Result<i64> {
        let trimmed_chat_id = chat_id.trim();
        if trimmed_chat_id.is_empty() {
            return Err(anyhow!("chat_id cannot be empty"));
        }

        let trimmed_content = content.trim();
        if trimmed_content.is_empty() {
            return Err(anyhow!("chat content cannot be empty"));
        }

        let now = Self::now_unix_ms();
        let conn = self.connection()?;
        let title = self.chat_title_from_first_message(trimmed_content);

        conn.execute(
            "INSERT INTO chat_sessions (id, title, created_at_unix_ms, updated_at_unix_ms)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET
               updated_at_unix_ms = excluded.updated_at_unix_ms,
               title = CASE
                 WHEN chat_sessions.title = 'New Chat' AND ?4 = 'me' THEN ?2
                 ELSE chat_sessions.title
               END",
            params![trimmed_chat_id, title, now, role],
        )?;

        conn.execute(
            "INSERT INTO chat_messages (chat_id, role, content, created_at_unix_ms, iteration_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![trimmed_chat_id, role, trimmed_content, now, iteration_id],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Lists most recently updated chat sessions.
    pub fn list_chat_sessions(&self, limit: usize) -> Result<Vec<PersistedChatSession>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT s.id, s.title, s.created_at_unix_ms, s.updated_at_unix_ms, COUNT(m.id)
             FROM chat_sessions s
             LEFT JOIN chat_messages m ON m.chat_id = s.id
             GROUP BY s.id
             HAVING COUNT(m.id) > 0
             ORDER BY s.updated_at_unix_ms DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(PersistedChatSession {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at_unix_ms: row.get(2)?,
                updated_at_unix_ms: row.get(3)?,
                message_count: row.get::<_, i64>(4)?.max(0) as usize,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Lists chat messages for one session after an optional id.
    pub fn list_chat_messages(
        &self,
        chat_id: &str,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<PersistedChatMessage>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT id, chat_id, role, content, created_at_unix_ms, iteration_id
             FROM chat_messages
             WHERE chat_id = ?1 AND (?2 IS NULL OR id > ?2)
             ORDER BY id ASC
             LIMIT ?3",
        )?;

        let rows = statement.query_map(params![chat_id, after_id, limit as i64], |row| {
            Ok(PersistedChatMessage {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at_unix_ms: row.get(4)?,
                iteration_id: row.get(5)?,
            })
        })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    /// Returns current unix timestamp in seconds.
    pub fn now_unix() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Returns current unix timestamp in milliseconds.
    pub fn now_unix_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.connection()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS iterations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at_unix INTEGER NOT NULL,
                sensed_percepts TEXT NOT NULL,
                surprising_percepts TEXT NOT NULL,
                planned_actions TEXT NOT NULL,
                action_results TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at_unix_ms INTEGER NOT NULL,
                updated_at_unix_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at_unix_ms INTEGER NOT NULL,
                iteration_id INTEGER,
                FOREIGN KEY(chat_id) REFERENCES chat_sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_chat_messages_chat_id_id
                ON chat_messages(chat_id, id);
            CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated
                ON chat_sessions(updated_at_unix_ms DESC);",
        )?;
        Ok(())
    }

    fn chat_title_from_first_message(&self, content: &str) -> String {
        let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
        let trimmed = compact.trim();
        if trimmed.is_empty() {
            return "New Chat".to_string();
        }
        trimmed.chars().take(48).collect::<String>()
    }

    fn connection(&self) -> Result<Connection> {
        Connection::open(Path::new(&self.path)).map_err(Into::into)
    }
}
