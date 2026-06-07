//! Local application state persistence (`state.db`).
//!
//! Stores query history and recently-opened files in a private SQLite
//! database, kept separate from the user's own databases. All access is async
//! via `sqlx`. The store is designed to degrade gracefully: callers treat an
//! open failure as "history disabled" rather than as a fatal error.

use std::path::Path;

use sextant_core::SextantError;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// A recorded query-history entry. Listings return newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub connection: String,
    pub sql: String,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub timestamp: String,
}

/// A recently-opened `.sql` file. Listings return newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub connection: String,
    pub path: String,
    pub last_opened: String,
}

/// A reusable, named SQL snippet. Snippets are global (not per-connection).
#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub body: String,
}

/// Per-connection cap for the recent-files ring buffer.
const RECENT_FILES_RING: i64 = 20;

/// Handle to the local `state.db`. Cloning is cheap — the pool is `Arc` inside.
#[derive(Clone)]
pub struct StateStore {
    pool: SqlitePool,
}

impl StateStore {
    /// Open (creating if needed) the state database at `path`, applying the
    /// schema. The parent directory is created `0700` and the file `0600`.
    pub async fn open(path: &Path) -> Result<Self, SextantError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            set_mode(parent, 0o700);
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(db_err)?;

        set_mode(path, 0o600);

        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<(), SextantError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS query_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL DEFAULT (datetime('now')),
                connection TEXT NOT NULL,
                sql TEXT NOT NULL,
                duration_ms INTEGER,
                error_msg TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS recent_files (
                connection TEXT NOT NULL,
                path TEXT NOT NULL,
                last_opened TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (connection, path)
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS snippets (
                name TEXT PRIMARY KEY,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        Ok(())
    }

    /// Save a named snippet, overwriting any existing snippet with the same name.
    pub async fn save_snippet(&self, name: &str, body: &str) -> Result<(), SextantError> {
        sqlx::query(
            "INSERT INTO snippets (name, body, created_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(name) DO UPDATE SET body = excluded.body, created_at = datetime('now')",
        )
        .bind(name)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// All snippets, ordered by name.
    pub async fn snippets(&self) -> Result<Vec<Snippet>, SextantError> {
        let rows =
            sqlx::query_as::<_, (String, String)>("SELECT name, body FROM snippets ORDER BY name")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;

        Ok(rows
            .into_iter()
            .map(|(name, body)| Snippet { name, body })
            .collect())
    }

    /// Append a query to the history.
    pub async fn record_query(
        &self,
        connection: &str,
        sql: &str,
        duration_ms: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), SextantError> {
        sqlx::query(
            "INSERT INTO query_history (connection, sql, duration_ms, error_msg)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(connection)
        .bind(sql)
        .bind(duration_ms)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// The most recent history entries, newest first (capped at `limit`).
    pub async fn recent_queries(&self, limit: i64) -> Result<Vec<HistoryEntry>, SextantError> {
        let rows = sqlx::query_as::<_, (String, String, Option<i64>, Option<String>, String)>(
            "SELECT connection, sql, duration_ms, error_msg, timestamp
             FROM query_history ORDER BY id DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        Ok(rows
            .into_iter()
            .map(
                |(connection, sql, duration_ms, error, timestamp)| HistoryEntry {
                    connection,
                    sql,
                    duration_ms,
                    error,
                    timestamp,
                },
            )
            .collect())
    }

    /// Record a file as recently opened for a connection. Re-recording an
    /// existing path refreshes its position; the list is pruned to the
    /// [`RECENT_FILES_RING`] most-recent files per connection.
    pub async fn record_file(&self, connection: &str, path: &str) -> Result<(), SextantError> {
        sqlx::query(
            "INSERT INTO recent_files (connection, path, last_opened)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(connection, path) DO UPDATE SET last_opened = datetime('now')",
        )
        .bind(connection)
        .bind(path)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        // Prune older files beyond the ring. `rowid` breaks ties when several
        // files share the same coarse (second-resolution) timestamp.
        sqlx::query(
            "DELETE FROM recent_files
             WHERE connection = ?1 AND rowid NOT IN (
                 SELECT rowid FROM recent_files WHERE connection = ?1
                 ORDER BY last_opened DESC, rowid DESC LIMIT ?2
             )",
        )
        .bind(connection)
        .bind(RECENT_FILES_RING)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// The recent files for a connection, newest first.
    pub async fn recent_files(&self, connection: &str) -> Result<Vec<FileEntry>, SextantError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT connection, path, last_opened FROM recent_files
             WHERE connection = ?1 ORDER BY last_opened DESC, rowid DESC LIMIT ?2",
        )
        .bind(connection)
        .bind(RECENT_FILES_RING)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        Ok(rows
            .into_iter()
            .map(|(connection, path, last_opened)| FileEntry {
                connection,
                path,
                last_opened,
            })
            .collect())
    }
}

fn db_err(e: sqlx::Error) -> SextantError {
    SextantError::Database(e.to_string())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_store() -> (StateStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = StateStore::open(&path).await.unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn query_history_records_and_lists_newest_first() {
        let (store, _dir) = temp_store().await;

        store
            .record_query("pg", "SELECT 1", Some(5), None)
            .await
            .unwrap();
        store
            .record_query("pg", "SELECT 2", Some(7), None)
            .await
            .unwrap();
        store
            .record_query("pg", "SELECT bad", None, Some("syntax error"))
            .await
            .unwrap();

        let history = store.recent_queries(10).await.unwrap();
        assert_eq!(history.len(), 3);
        // Newest first.
        assert_eq!(history[0].sql, "SELECT bad");
        assert_eq!(history[0].error.as_deref(), Some("syntax error"));
        assert_eq!(history[0].duration_ms, None);
        assert_eq!(history[2].sql, "SELECT 1");
        assert_eq!(history[2].duration_ms, Some(5));
    }

    #[tokio::test]
    async fn recent_queries_respects_limit() {
        let (store, _dir) = temp_store().await;
        for i in 0..5 {
            store
                .record_query("pg", &format!("SELECT {i}"), Some(1), None)
                .await
                .unwrap();
        }
        let history = store.recent_queries(2).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].sql, "SELECT 4");
        assert_eq!(history[1].sql, "SELECT 3");
    }

    #[tokio::test]
    async fn recent_files_deduplicates_on_path() {
        let (store, _dir) = temp_store().await;
        store.record_file("pg", "/q/a.sql").await.unwrap();
        store.record_file("pg", "/q/b.sql").await.unwrap();
        store.record_file("pg", "/q/a.sql").await.unwrap();

        let files = store.recent_files("pg").await.unwrap();
        // The same path is not duplicated by re-recording.
        assert_eq!(files.len(), 2);
        assert_eq!(
            files.iter().filter(|f| f.path == "/q/a.sql").count(),
            1,
            "re-recording must not duplicate the path"
        );
    }

    #[tokio::test]
    async fn snippets_save_list_and_overwrite() {
        let (store, _dir) = temp_store().await;
        store
            .save_snippet("count", "SELECT count(*) FROM ?")
            .await
            .unwrap();
        store
            .save_snippet("recent", "SELECT * FROM t ORDER BY id DESC LIMIT 10")
            .await
            .unwrap();

        let list = store.snippets().await.unwrap();
        assert_eq!(list.len(), 2);
        // Ordered by name.
        assert_eq!(list[0].name, "count");
        assert_eq!(list[1].name, "recent");

        // Re-saving the same name overwrites the body, not appends.
        store.save_snippet("count", "SELECT 1").await.unwrap();
        let list = store.snippets().await.unwrap();
        assert_eq!(list.len(), 2);
        let count = list.iter().find(|s| s.name == "count").unwrap();
        assert_eq!(count.body, "SELECT 1");
    }

    #[tokio::test]
    async fn recent_files_are_scoped_per_connection() {
        let (store, _dir) = temp_store().await;
        store.record_file("pg", "/q/a.sql").await.unwrap();
        store.record_file("mysql", "/q/b.sql").await.unwrap();

        let pg = store.recent_files("pg").await.unwrap();
        assert_eq!(pg.len(), 1);
        assert_eq!(pg[0].path, "/q/a.sql");
    }

    #[tokio::test]
    async fn recent_files_ring_is_bounded() {
        let (store, _dir) = temp_store().await;
        for i in 0..(RECENT_FILES_RING + 5) {
            store
                .record_file("pg", &format!("/q/file{i}.sql"))
                .await
                .unwrap();
        }

        // The public listing is capped, and so is the underlying table (prune).
        let files = store.recent_files("pg").await.unwrap();
        assert_eq!(files.len() as i64, RECENT_FILES_RING);

        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM recent_files WHERE connection = 'pg'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(count, RECENT_FILES_RING, "prune must bound the table");
    }
}
