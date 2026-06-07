use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::LeagueId;

/// Counter for probabilistic cache eviction (1 in 100 writes triggers cleanup).
static SET_CACHED_COUNTER: AtomicU64 = AtomicU64::new(0);

// ─── Types ──────────────────────────────────────────────────────────────────

/// A recommendation stored in the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: i64,
    pub agent_id: String,
    pub league_id: LeagueId,
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
/// The inner connection is wrapped in a `parking_lot::Mutex` so `Database` is
/// `Send + Sync` and can live inside an `Arc<AppState>` shared across async
/// tasks. All public sync methods have async counterparts (suffixed `_async`)
/// that run via `tokio::task::spawn_blocking` to avoid blocking the async
/// runtime.
///
/// `parking_lot::Mutex` does not poison on panic, so lock acquisition is
/// infallible and cancel-safe: dropping a `spawn_blocking` handle cannot
/// leave the mutex in a poisoned state.
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
        let conn = self.conn.lock();
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

    pub fn health_check(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.query_row("SELECT 1", [], |_| Ok(()))
    }

    // ─── Cache layer ────────────────────────────────────────────────────────

    /// Get a cached value by key. Returns `None` if missing or expired.
    pub fn get_cached(&self, key: &str) -> Option<Value> {
        let conn = self.conn.lock();
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
        // Guard against clock skew: if age is negative, treat entry as stale.
        if age.num_seconds() < 0 {
            return None;
        }
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
        let data = match serde_json::to_string(value) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(cache_key = key, error = %e, "failed to serialize cache value");
                return;
            }
        };
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        if let Err(e) = conn.execute(
            "INSERT OR REPLACE INTO cached_responses (key, data, fetched_at, ttl_seconds) \
             VALUES (?1, ?2, ?3, ?4)",
            params![key, data, now, ttl_seconds],
        ) {
            tracing::warn!(cache_key = key, error = %e, "failed to write cache entry");
        }

        // Probabilistic cache eviction: clean up expired entries on ~1% of writes.
        let count = SET_CACHED_COUNTER.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(100) {
            self.evict_expired_locked(&conn);
        }
    }

    /// Delete all expired cache entries. Expects the caller to already hold the lock.
    fn evict_expired_locked(&self, conn: &Connection) {
        let now = Utc::now().to_rfc3339();
        match conn.execute(
            "DELETE FROM cached_responses WHERE \
             datetime(fetched_at, '+' || ttl_seconds || ' seconds') < datetime(?1)",
            params![now],
        ) {
            Ok(n) if n > 0 => tracing::debug!(deleted = n, "evicted expired cache entries"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "failed to evict expired cache entries"),
        }
    }

    // ─── Recommendation ledger ──────────────────────────────────────────────

    /// Log a new recommendation. Returns the inserted row ID.
    pub fn log_recommendation(
        &self,
        agent_id: &str,
        league_id: &LeagueId,
        recommendation_type: &str,
        players: &str,
        reasoning: &str,
    ) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO recommendation_ledger \
             (agent_id, league_id, recommendation_type, players, reasoning, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent_id,
                league_id.as_str(),
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
        league_id: &LeagueId,
        agent_id: Option<&str>,
    ) -> Vec<Recommendation> {
        let conn = self.conn.lock();

        let (sql, bind_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match agent_id {
            Some(aid) => (
                "SELECT id, agent_id, league_id, recommendation_type, players, reasoning, \
                 created_at, outcome, outcome_date \
                 FROM recommendation_ledger WHERE league_id = ?1 AND agent_id = ?2 \
                 ORDER BY created_at DESC"
                    .to_string(),
                vec![
                    Box::new(league_id.as_str().to_owned()),
                    Box::new(aid.to_string()),
                ],
            ),
            None => (
                "SELECT id, agent_id, league_id, recommendation_type, players, reasoning, \
                 created_at, outcome, outcome_date \
                 FROM recommendation_ledger WHERE league_id = ?1 \
                 ORDER BY created_at DESC"
                    .to_string(),
                vec![Box::new(league_id.as_str().to_owned())],
            ),
        };

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%league_id, error = %e, "failed to prepare recommendation query");
                return vec![];
            }
        };

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(Recommendation {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                league_id: LeagueId::new(row.get::<_, String>(2)?),
                recommendation_type: row.get(3)?,
                players: row.get(4)?,
                reasoning: row.get(5)?,
                created_at: row.get(6)?,
                outcome: row.get(7)?,
                outcome_date: row.get(8)?,
            })
        });

        match rows {
            Ok(mapped) => mapped
                .filter_map(|r| match r {
                    Ok(rec) => Some(rec),
                    Err(e) => {
                        tracing::warn!(%league_id, error = %e, "failed to deserialize recommendation row");
                        None
                    }
                })
                .collect(),
            Err(e) => {
                tracing::warn!(%league_id, error = %e, "failed to query recommendations");
                vec![]
            }
        }
    }

    /// Record an outcome for a previously logged recommendation.
    ///
    /// Returns an error if the recommendation ID does not exist.
    pub fn record_outcome(&self, id: i64, outcome: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        let rows_affected = conn.execute(
            "UPDATE recommendation_ledger SET outcome = ?1, outcome_date = ?2 WHERE id = ?3",
            params![outcome, now, id],
        )?;
        if rows_affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }
}

// ─── Async wrappers ────────────────────────────────────────────────────────

/// Async wrappers that run sync DB operations on a blocking thread pool.
///
/// Each operation runs inside `spawn_blocking`, making it a Tokio task: if the
/// caller's future is cancelled (dropped), the DB operation still runs to
/// completion, preserving data integrity.
impl Database {
    pub async fn get_cached_async(self: &Arc<Self>, key: String) -> Option<Value> {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || db.get_cached(&key))
            .await
            .ok()
            .flatten()
    }

    pub async fn set_cached_async(self: &Arc<Self>, key: String, value: Value, ttl: i64) {
        let db = Arc::clone(self);
        let _ = tokio::task::spawn_blocking(move || db.set_cached(&key, &value, ttl)).await;
    }

    pub async fn log_recommendation_async(
        self: &Arc<Self>,
        agent_id: String,
        league_id: LeagueId,
        recommendation_type: String,
        players: String,
        reasoning: String,
    ) -> Result<i64, rusqlite::Error> {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            db.log_recommendation(
                &agent_id,
                &league_id,
                &recommendation_type,
                &players,
                &reasoning,
            )
        })
        .await
        .expect("spawn_blocking panicked")
    }

    pub async fn get_recommendations_async(
        self: &Arc<Self>,
        league_id: LeagueId,
        agent_id: Option<String>,
    ) -> Vec<Recommendation> {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || db.get_recommendations(&league_id, agent_id.as_deref()))
            .await
            .unwrap_or_default()
    }

    pub async fn record_outcome_async(
        self: &Arc<Self>,
        id: i64,
        outcome: String,
    ) -> Result<(), rusqlite::Error> {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || db.record_outcome(id, &outcome))
            .await
            .expect("spawn_blocking panicked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a fresh in-memory-like database in a temp directory.
    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("failed to open test db");
        (db, dir)
    }

    // ── Health check ───────────────────────────────────────────────────

    #[test]
    fn health_check_succeeds() {
        let (db, _dir) = test_db();
        db.health_check().expect("health check should succeed");
    }

    // ── Cache layer ────────────────────────────────────────────────────

    #[test]
    fn cache_miss_returns_none() {
        let (db, _dir) = test_db();
        assert!(db.get_cached("nonexistent").is_none());
    }

    #[test]
    fn cache_set_then_get() {
        let (db, _dir) = test_db();
        let value = serde_json::json!({"hello": "world"});
        db.set_cached("key1", &value, 3600);

        let cached = db.get_cached("key1").expect("should find cached value");
        assert_eq!(cached, value);
    }

    #[test]
    fn cache_overwrite() {
        let (db, _dir) = test_db();
        let v1 = serde_json::json!(1);
        let v2 = serde_json::json!(2);

        db.set_cached("key", &v1, 3600);
        db.set_cached("key", &v2, 3600);

        let cached = db.get_cached("key").expect("should find cached value");
        assert_eq!(cached, v2);
    }

    #[test]
    fn cache_expired_returns_none() {
        let (db, _dir) = test_db();
        let value = serde_json::json!("ephemeral");

        // Insert with 0 TTL — immediately expired.
        db.set_cached("expire_me", &value, 0);
        // The entry should be considered expired on the next read.
        // TTL=0 means age (>= 0 seconds) > 0 is false for immediate reads,
        // but the check is `age.num_seconds() > ttl_seconds` (strict >),
        // so age=0, ttl=0 => 0 > 0 is false — still valid.
        // Use TTL=-1 to guarantee expiry (negative TTL is nonsensical but
        // exercises the boundary).
        db.set_cached("expire_me_neg", &value, -1);
        assert!(db.get_cached("expire_me_neg").is_none());
    }

    #[test]
    fn evict_expired_removes_stale_entries() {
        let (db, _dir) = test_db();

        // Insert an entry with a far-past fetched_at and a short TTL so the
        // eviction SQL (which uses datetime arithmetic) sees it as expired.
        {
            let conn = db.conn.lock();
            let past = "2000-01-01T00:00:00+00:00";
            conn.execute(
                "INSERT OR REPLACE INTO cached_responses (key, data, fetched_at, ttl_seconds) \
                 VALUES (?1, ?2, ?3, ?4)",
                params!["stale_key", "\"stale\"", past, 1],
            )
            .unwrap();
        }

        // Verify our Rust-side check also considers it expired.
        assert!(db.get_cached("stale_key").is_none());

        // Manually trigger eviction.
        let conn = db.conn.lock();
        db.evict_expired_locked(&conn);
        drop(conn);

        // Verify the row was actually deleted from the table.
        let conn = db.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cached_responses WHERE key = 'stale_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "stale entry should have been evicted");
    }

    #[test]
    fn evict_does_not_remove_live_entries() {
        let (db, _dir) = test_db();
        let value = serde_json::json!("alive");

        db.set_cached("live_key", &value, 3600);

        let conn = db.conn.lock();
        db.evict_expired_locked(&conn);
        drop(conn);

        assert!(db.get_cached("live_key").is_some());
    }

    #[test]
    fn probabilistic_eviction_triggers_on_100th_write() {
        let (db, _dir) = test_db();

        // Reset the counter to just before a multiple of 100.
        SET_CACHED_COUNTER.store(99, Ordering::Relaxed);

        // Insert a stale entry.
        {
            let conn = db.conn.lock();
            let past = "2000-01-01T00:00:00+00:00";
            conn.execute(
                "INSERT OR REPLACE INTO cached_responses (key, data, fetched_at, ttl_seconds) \
                 VALUES (?1, ?2, ?3, ?4)",
                params!["stale_prob", "\"old\"", past, 1],
            )
            .unwrap();
        }

        // This write will be the 100th (counter was 99, fetch_add makes it 100
        // and returns 99 — wait, is_multiple_of checks the returned value).
        // Actually: fetch_add(1) returns old value. 99 is not a multiple of 100.
        // So we need counter at 99, then the *next* set_cached increments to 100
        // and returns 99. is_multiple_of(100) on 99 is false.
        // We need old value = 0, 100, 200... So store 0-1 = we need fetch_add to
        // return a multiple of 100. Store 99 => returns 99, not multiple. Store 100
        // => returns 100, is_multiple_of(100) = true.
        SET_CACHED_COUNTER.store(100, Ordering::Relaxed);

        // This set_cached call should trigger eviction (counter returns 100).
        db.set_cached("trigger", &serde_json::json!("trigger"), 3600);

        // The stale entry should have been cleaned up.
        let conn = db.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cached_responses WHERE key = 'stale_prob'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "probabilistic eviction should have cleaned stale entry"
        );
    }

    // ── Recommendation ledger ──────────────────────────────────────────

    #[test]
    fn log_and_query_recommendation() {
        let (db, _dir) = test_db();
        let league = LeagueId::new("test-league");

        let id = db
            .log_recommendation("ariadne", &league, "pickup", "[\"p1\"]", "looks good")
            .expect("should log recommendation");
        assert!(id > 0);

        let recs = db.get_recommendations(&league, None);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, id);
        assert_eq!(recs[0].agent_id, "ariadne");
        assert_eq!(recs[0].recommendation_type, "pickup");
        assert_eq!(recs[0].players, "[\"p1\"]");
        assert_eq!(recs[0].reasoning, "looks good");
        assert!(recs[0].outcome.is_none());
    }

    #[test]
    fn query_filters_by_agent() {
        let (db, _dir) = test_db();
        let league = LeagueId::new("league-1");

        db.log_recommendation("alice", &league, "start", "[\"p1\"]", "reason a")
            .unwrap();
        db.log_recommendation("bob", &league, "sit", "[\"p2\"]", "reason b")
            .unwrap();

        let alice_recs = db.get_recommendations(&league, Some("alice"));
        assert_eq!(alice_recs.len(), 1);
        assert_eq!(alice_recs[0].agent_id, "alice");

        let bob_recs = db.get_recommendations(&league, Some("bob"));
        assert_eq!(bob_recs.len(), 1);
        assert_eq!(bob_recs[0].agent_id, "bob");

        let all_recs = db.get_recommendations(&league, None);
        assert_eq!(all_recs.len(), 2);
    }

    #[test]
    fn query_filters_by_league() {
        let (db, _dir) = test_db();
        let league_a = LeagueId::new("league-a");
        let league_b = LeagueId::new("league-b");

        db.log_recommendation("agent", &league_a, "pickup", "[]", "r1")
            .unwrap();
        db.log_recommendation("agent", &league_b, "drop", "[]", "r2")
            .unwrap();

        assert_eq!(db.get_recommendations(&league_a, None).len(), 1);
        assert_eq!(db.get_recommendations(&league_b, None).len(), 1);
    }

    #[test]
    fn record_outcome_updates_recommendation() {
        let (db, _dir) = test_db();
        let league = LeagueId::new("test-league");

        let id = db
            .log_recommendation("agent", &league, "start", "[\"p1\"]", "solid matchup")
            .unwrap();

        db.record_outcome(id, "win").expect("should record outcome");

        let recs = db.get_recommendations(&league, None);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].outcome.as_deref(), Some("win"));
        assert!(recs[0].outcome_date.is_some());
    }

    #[test]
    fn record_outcome_nonexistent_id_fails() {
        let (db, _dir) = test_db();
        let result = db.record_outcome(99999, "loss");
        assert!(result.is_err());
    }

    #[test]
    fn multiple_recommendations_ordered_by_created_desc() {
        let (db, _dir) = test_db();
        let league = LeagueId::new("test-league");

        let id1 = db
            .log_recommendation("agent", &league, "pickup", "[\"p1\"]", "first")
            .unwrap();
        let id2 = db
            .log_recommendation("agent", &league, "drop", "[\"p2\"]", "second")
            .unwrap();

        let recs = db.get_recommendations(&league, None);
        assert_eq!(recs.len(), 2);
        // Most recent first.
        assert_eq!(recs[0].id, id2);
        assert_eq!(recs[1].id, id1);
    }

    // ── Schema initialization ──────────────────────────────────────────

    #[test]
    fn open_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let deep_path = dir.path().join("a").join("b").join("c").join("test.db");
        let db = Database::open(&deep_path).expect("should create parent dirs and open");
        db.health_check().unwrap();
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");

        let db1 = Database::open(&path).unwrap();
        db1.set_cached("key", &serde_json::json!("v"), 3600);
        drop(db1);

        let db2 = Database::open(&path).unwrap();
        let cached = db2.get_cached("key");
        assert_eq!(cached, Some(serde_json::json!("v")));
    }
}
