use rusqlite::{Connection, Result, params, OptionalExtension};
use std::sync::{Arc, Mutex};
use std::path::Path;
use tracing::{info, error, info_span};

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
}

pub struct QueueStore {
    pub conn: Arc<Mutex<Connection>>,
}

impl QueueStore {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        
        // Critical: Enable WAL mode for high concurrency between Scanner (Writer) and Workers (Readers/Updaters)
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS queue (
                path TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'PENDING',
                mtime INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Create index for fast polling
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_queue_status ON queue(status)",
            [],
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn push(&self, path: &str, mtime: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Insert or Update if mtime has changed (file modified)
        conn.execute(
            "INSERT INTO queue (path, status, mtime) 
             VALUES (?1, 'PENDING', ?2)
             ON CONFLICT(path) DO UPDATE SET 
             status = 'PENDING',
             mtime = excluded.mtime,
             updated_at = CURRENT_TIMESTAMP
             WHERE queue.mtime != excluded.mtime",
            params![path, mtime],
        )?;
        Ok(())
    }

    pub fn pop(&self) -> Option<Task> {
        let _span = info_span!("queue_pop").entered();
        let conn = self.conn.lock().unwrap();
        
        // SQLite doesn't have a single UPDATE RETURNING before 3.35, 
        // and even then, doing it concurrently needs care. 
        // We use a transaction to SELECT then UPDATE.
        let tx = match conn.unchecked_transaction() {
            Ok(t) => t,
            Err(e) => {
                error!("Queue pop transaction failed: {}", e);
                return None;
            }
        };

        let row: Result<String> = tx.query_row(
            "SELECT path FROM queue WHERE status = 'PENDING' LIMIT 1",
            [],
            |row| row.get(0),
        );

        match row {
            Ok(path) => {
                if let Err(e) = tx.execute("UPDATE queue SET status = 'PROCESSING', updated_at = CURRENT_TIMESTAMP WHERE path = ?1", params![&path]) {
                    error!("Failed to mark task as PROCESSING: {}", e);
                    let _ = tx.rollback();
                    return None;
                }
                if let Err(e) = tx.commit() {
                    error!("Failed to commit task pop: {}", e);
                    return None;
                }
                Some(Task { path })
            }
            Err(_) => {
                let _ = tx.rollback();
                None
            }
        }
    }

    pub fn mark_done(&self, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE queue SET status = 'DONE', updated_at = CURRENT_TIMESTAMP WHERE path = ?1",
            params![path],
        )?;
        Ok(())
    }

    pub fn count_pending(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT count(*) FROM queue WHERE status = 'PENDING'",
            [],
            |row| row.get(0),
        )
    }
}
