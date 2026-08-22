//! A JSON file holding a list of rows, with locked read-modify-write.
//!
//! Every registry in this tool is "a list of things with ids", so they all share
//! this one implementation. The interesting part is [`JsonTable::mutate`]: it
//! takes the lock, re-reads from disk, applies the change and writes atomically
//! — so a concurrent `pa tick` and `pa goal complete` cannot lose each other's
//! edit the way a naive load-edit-save would.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use umoja_domain::error::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::files::{read_json, write_json};
use crate::lock::FileLock;

#[derive(Debug)]
pub struct JsonTable<T> {
    path: PathBuf,
    marker: PhantomData<fn() -> T>,
}

impl<T> JsonTable<T>
where
    T: Serialize + DeserializeOwned + Clone,
{
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            marker: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rows(&self) -> Result<Vec<T>> {
        read_json(&self.path)
    }

    /// Locks, re-reads, applies, writes. The closure sees the state on disk
    /// right now, not a copy read some time earlier.
    pub fn mutate<F, R>(&self, apply: F) -> Result<R>
    where
        F: FnOnce(&mut Vec<T>) -> Result<R>,
    {
        let _lock = FileLock::acquire(&self.path)?;
        let mut rows: Vec<T> = read_json(&self.path)?;
        let outcome = apply(&mut rows)?;
        write_json(&self.path, &rows)?;
        Ok(outcome)
    }

    /// Replaces the row matching `is_match`, or appends when there is none.
    pub fn upsert<M>(&self, value: &T, is_match: M) -> Result<()>
    where
        M: Fn(&T) -> bool,
    {
        self.mutate(|rows| {
            match rows.iter_mut().find(|row| is_match(row)) {
                Some(slot) => *slot = value.clone(),
                None => rows.push(value.clone()),
            }
            Ok(())
        })
    }

    /// Removes matching rows and reports how many went.
    pub fn remove<M>(&self, is_match: M) -> Result<usize>
    where
        M: Fn(&T) -> bool,
    {
        self.mutate(|rows| {
            let before = rows.len();
            rows.retain(|row| !is_match(row));
            Ok(before - rows.len())
        })
    }

    pub fn find<M>(&self, is_match: M) -> Result<Option<T>>
    where
        M: Fn(&T) -> bool,
    {
        Ok(self.rows()?.into_iter().find(|row| is_match(row)))
    }

    /// Keeps the newest `keep` rows, oldest first in the file.
    ///
    /// Registries that only grow eventually make every command slower; dropping
    /// the oldest row is better than refusing the newest write.
    pub fn trim_to(&self, keep: usize) -> Result<usize> {
        self.mutate(|rows| {
            if rows.len() <= keep {
                return Ok(0);
            }
            let dropped = rows.len() - keep;
            rows.drain(..dropped);
            Ok(dropped)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct Row {
        id: String,
        value: u32,
    }

    fn table(name: &str) -> JsonTable<Row> {
        let dir = std::env::temp_dir().join(format!("pa-table-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        JsonTable::new(dir.join("rows.json"))
    }

    fn row(id: &str, value: u32) -> Row {
        Row {
            id: id.into(),
            value,
        }
    }

    #[test]
    fn upsert_appends_then_replaces() {
        let table = table("upsert");
        table.upsert(&row("a", 1), |r| r.id == "a").unwrap();
        table.upsert(&row("b", 2), |r| r.id == "b").unwrap();
        table.upsert(&row("a", 99), |r| r.id == "a").unwrap();

        let rows = table.rows().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].value, 99);
    }

    #[test]
    fn remove_reports_what_it_took() {
        let table = table("remove");
        table.upsert(&row("a", 1), |r| r.id == "a").unwrap();
        assert_eq!(table.remove(|r| r.id == "a").unwrap(), 1);
        assert_eq!(table.remove(|r| r.id == "a").unwrap(), 0);
    }

    #[test]
    fn mutate_sees_what_is_on_disk_not_a_stale_copy() {
        let table = table("fresh");
        table.upsert(&row("a", 1), |r| r.id == "a").unwrap();

        // Simulate another process writing between our read and our write.
        let other = JsonTable::<Row>::new(table.path().to_path_buf());
        other.upsert(&row("b", 2), |r| r.id == "b").unwrap();

        table
            .mutate(|rows| {
                rows.push(row("c", 3));
                Ok(())
            })
            .unwrap();

        let ids: Vec<String> = table.rows().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn trimming_drops_the_oldest_rows_first() {
        let table = table("trim");
        for index in 0..10 {
            table
                .upsert(&row(&index.to_string(), index), |r| {
                    r.id == index.to_string()
                })
                .unwrap();
        }
        assert_eq!(table.trim_to(4).unwrap(), 6);
        let ids: Vec<String> = table.rows().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["6", "7", "8", "9"]);
        assert_eq!(table.trim_to(10).unwrap(), 0);
    }
}
