use rusqlite::{Connection, Result, params, OptionalExtension};
use std::sync::{Arc, Mutex};
use std::path::Path;
use tracing::{info, error, info_span};

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
    pub trace_id: String,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
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
        
        // Drop table to ensure new schema is applied (ephemeral queue anyway)
        let _ = conn.execute("DROP TABLE IF EXISTS queue", []);

        conn.execute(
            "CREATE TABLE queue (
                path TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'PENDING',
                mtime INTEGER NOT NULL DEFAULT 0,
                trace_id TEXT NOT NULL,
                t0 INTEGER NOT NULL,
                t1 INTEGER NOT NULL,
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

    pub fn push(&self, path: &str, mtime: i64, trace_id: &str, t0: i64, t1: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Insert or Update if mtime has changed (file modified)
        conn.execute(
            "INSERT INTO queue (path, status, mtime, trace_id, t0, t1) 
             VALUES (?1, 'PENDING', ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET 
             status = 'PENDING',
             mtime = excluded.mtime,
             trace_id = excluded.trace_id,
             t0 = excluded.t0,
             t1 = excluded.t1,
             updated_at = CURRENT_TIMESTAMP
             WHERE queue.mtime != excluded.mtime",
            params![path, mtime, trace_id, t0, t1],
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

        let row: Result<(String, String, i64, i64)> = tx.query_row(
            "SELECT path, trace_id, t0, t1 FROM queue WHERE status = 'PENDING' LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        );

        match row {
            Ok((path, trace_id, t0, t1)) => {
                if let Err(e) = tx.execute("UPDATE queue SET status = 'PROCESSING', updated_at = CURRENT_TIMESTAMP WHERE path = ?1", params![&path]) {
                    error!("Failed to mark task as PROCESSING: {}", e);
                    let _ = tx.rollback();
                    return None;
                }
                if let Err(e) = tx.commit() {
                    error!("Failed to commit task pop: {}", e);
                    return None;
                }
                let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
                Some(Task { path, trace_id, t0, t1, t2 })
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
}
