use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Types ──────────────────────────────────────────────────────────────────

/// A recommendation stored in the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: i64,
    pub agent_id: String,
    pub league_id: String,
    pub recommendation_type: String,
    pub players: String,
    pub reasoning: String,
    pub created_at: String,
    pub outcome: Option<String>,
    pub outcome_date: Option<String>,
}

// ─── Database ───────────────────────────────────────────────────────────────

/// SQLite-backed data layer for caching and the recommendation ledger.
///
/// The inner connection is wrapped in a `Mutex` so `Database` is `Send + Sync`
/// and can live inside an `Arc<AppState>` shared across async tasks.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (or create) the database at `path`, initializing the schema.
    ///
    /// Creates parent directories if they don't exist.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(path = %parent.display(), error = %e, "failed to create database parent directory");
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cached_responses (
                key TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                ttl_seconds INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS recommendation_ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                league_id TEXT NOT NULL,
                recommendation_type TEXT NOT NULL,
                players TEXT NOT NULL,
                reasoning TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                outcome TEXT,
                outcome_date TEXT
            );

",
        )?;
        Ok(())
    }

    // ─── Cache layer ────────────────────────────────────────────────────────

    /// Get a cached value by key. Returns `None` if missing or expired.
    pub fn get_cached(&self, key: &str) -> Option<Value> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let result: Result<(String, String, i64), _> = conn.query_row(
            "SELECT data, fetched_at, ttl_seconds FROM cached_responses WHERE key = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );

        let (data, fetched_at, ttl_seconds) = result.ok()?;

        let fetched = match chrono::DateTime::parse_from_rfc3339(&fetched_at) {
            Ok(dt) => dt,
            Err(e) => {
                tracing::warn!(
                    cache_key = key,
                    raw_value = fetched_at,
                    error = %e,
                    "failed to parse cached fetched_at timestamp"
                );
                return None;
            }
        };
        let age = Utc::now().signed_duration_since(fetched);
        if age.num_seconds() > ttl_seconds {
            return None;
        }

        match serde_json::from_str(&data) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    cache_key = key,
                    error = %e,
                    "failed to parse cached JSON data"
                );
                None
            }
        }
    }

    /// Insert or update a cached value with the given TTL in seconds.
    pub fn set_cached(&self, key: &str, value: &Value, ttl_seconds: i64) {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let data = serde_json::to_string(value).unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        if let Err(e) = conn.execute(
            "INSERT OR REPLACE INTO cached_responses (key, data, fetched_at, ttl_seconds) \
             VALUES (?1, ?2, ?3, ?4)",
            params![key, data, now, ttl_seconds],
        ) {
            tracing::warn!(cache_key = key, error = %e, "failed to write cache entry");
        }
    }

    // ─── Recommendation ledger ──────────────────────────────────────────────

    /// Log a new recommendation. Returns the inserted row ID.
    pub fn log_recommendation(
        &self,
        agent_id: &str,
        league_id: &str,
        recommendation_type: &str,
        players: &str,
        reasoning: &str,
    ) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO recommendation_ledger \
             (agent_id, league_id, recommendation_type, players, reasoning, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent_id,
                league_id,
                recommendation_type,
                players,
                reasoning,
                now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Query recommendations, optionally filtered by agent_id.
    pub fn get_recommendations(
        &self,
        league_id: &str,
        agent_id: Option<&str>,
    ) -> Vec<Recommendation> {
        let conn = self.conn.lock().expect("db mutex poisoned");

        let (sql, bind_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match agent_id {
            Some(aid) => (
                "SELECT id, agent_id, league_id, recommendation_type, players, reasoning, \
                 created_at, outcome, outcome_date \
                 FROM recommendation_ledger WHERE league_id = ?1 AND agent_id = ?2 \
                 ORDER BY created_at DESC"
                    .to_string(),
                vec![Box::new(league_id.to_string()), Box::new(aid.to_string())],
            ),
            None => (
                "SELECT id, agent_id, league_id, recommendation_type, players, reasoning, \
                 created_at, outcome, outcome_date \
                 FROM recommendation_ledger WHERE league_id = ?1 \
                 ORDER BY created_at DESC"
                    .to_string(),
                vec![Box::new(league_id.to_string())],
            ),
        };

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(league_id, error = %e, "failed to prepare recommendation query");
                return vec![];
            }
        };

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Recommendation {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                league_id: row.get(2)?,
                recommendation_type: row.get(3)?,
                players: row.get(4)?,
                reasoning: row.get(5)?,
                created_at: row.get(6)?,
                outcome: row.get(7)?,
                outcome_date: row.get(8)?,
            })
        });

        match rows {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                tracing::warn!(league_id, error = %e, "failed to query recommendations");
                vec![]
            }
        }
    }

    /// Record an outcome for a previously logged recommendation.
    pub fn record_outcome(&self, id: i64, outcome: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE recommendation_ledger SET outcome = ?1, outcome_date = ?2 WHERE id = ?3",
            params![outcome, now, id],
        )?;
        Ok(())
    }
}
