//! SQLite database connection wrapper and initialization.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use umoja_domain::error::{DomainError, Result};

use super::schema::initialize_schema;

#[derive(Clone)]
pub struct SqliteDb {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for SqliteDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteDb").field("path", &self.path).finish()
    }
}

impl SqliteDb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_buf = path.as_ref().to_path_buf();
        if let Some(parent) = path_buf.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::adapter("sqlite create dir", e))?;
        }

        let conn = Connection::open(&path_buf)
            .map_err(|e| DomainError::adapter("sqlite open", e))?;

        initialize_schema(&conn).map_err(|e| DomainError::adapter("sqlite init schema", e))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path_buf),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| DomainError::adapter("sqlite in_memory open", e))?;

        initialize_schema(&conn).map_err(|e| DomainError::adapter("sqlite init schema", e))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
        })
    }

    pub fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> std::result::Result<T, rusqlite::Error>,
    {
        let mut lock = self
            .conn
            .lock()
            .map_err(|_| DomainError::adapter("sqlite lock", "lock poisoned"))?;
        f(&mut lock).map_err(|e| DomainError::adapter("sqlite query", e))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}
