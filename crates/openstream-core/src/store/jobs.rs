use rusqlite::{params, Connection};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum JobStoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct DownloadJobStore {
    conn: Connection,
}

impl DownloadJobStore {
    pub fn open_default() -> Result<Self, JobStoreError> {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("StreamCraft");
        std::fs::create_dir_all(&dir)?;
        Self::open(dir.join("jobs.sqlite"))
    }

    pub fn open(path: PathBuf) -> Result<Self, JobStoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                payload TEXT NOT NULL,
                state TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn upsert_json(&self, id: Uuid, state: &str, payload: &str) -> Result<(), JobStoreError> {
        self.conn.execute(
            "INSERT INTO jobs (id, payload, state, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET payload=excluded.payload, state=excluded.state, updated_at=excluded.updated_at",
            params![id.to_string(), payload, state],
        )?;
        Ok(())
    }

    pub fn list_json(&self) -> Result<Vec<(String, String, String)>, JobStoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, state, payload FROM jobs ORDER BY updated_at DESC")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
