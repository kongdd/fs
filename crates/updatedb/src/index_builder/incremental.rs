//! In-memory directory snapshot for updates; no filenames or postings are loaded.
use std::{collections::HashMap, ffi::OsString};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::os_string;

pub(super) struct StoredDirectory {
    pub id: i64,
    pub stamp: Vec<u8>,
    pub children: Vec<OsString>,
}

#[derive(Default)]
pub(super) struct DirectorySnapshot {
    pub directories: HashMap<Vec<u8>, StoredDirectory>,
}

impl DirectorySnapshot {
    pub fn load(connection: &Connection, mut scope: &[u8]) -> Result<Self> {
        while let Some(parent) = scope.strip_suffix(b"/") {
            scope = parent;
        }
        let mut lower = scope.to_vec();
        lower.push(b'/');
        let mut upper = lower.clone();
        *upper.last_mut().unwrap() = b'0';
        // BLOB ranges are component-safe, including non-UTF-8 names. Empty scope
        // is the root; avoid an OR/range lookup for this common full-update case.
        let condition = if scope.is_empty() {
            ""
        } else {
            " WHERE d.path=?1 OR (d.path>=?2 AND d.path<?3)"
        };
        let mut snapshot = Self::default();
        let mut select = connection.prepare(&format!(
            "SELECT d.id,d.path,d.stamp FROM directories d{condition}"
        ))?;
        let mut rows = if scope.is_empty() {
            select.query([])?
        } else {
            select.query(params![scope, &lower, &upper])?
        };
        while let Some(row) = rows.next()? {
            snapshot.directories.insert(
                row.get(1)?,
                StoredDirectory {
                    id: row.get(0)?,
                    stamp: row.get(2)?,
                    children: Vec::new(),
                },
            );
        }
        // A single scan replaces one children query per reused directory. Keep
        // children with no directory row too: they may have been unreadable at
        // initialization and still need to be checked on the next update.
        let mut select = connection.prepare(&format!(
            "SELECT d.path,c.name FROM children c JOIN directories d ON d.id=c.directory{condition}
             ORDER BY c.directory,c.name"
        ))?;
        let mut rows = if scope.is_empty() {
            select.query([])?
        } else {
            select.query(params![scope, &lower, &upper])?
        };
        while let Some(row) = rows.next()? {
            let path = row.get_ref(0)?.as_blob()?;
            if let Some(directory) = snapshot.directories.get_mut(path) {
                directory.children.push(os_string(row.get(1)?)?);
            }
        }
        Ok(snapshot)
    }
}

// Optional, backward-compatible metadata. Invalidation triggers also protect
// this count when an older fs binary or another SQLite writer updates blocks.
const COUNT_KEY: &str = "entry_count";

pub(super) fn cached_entry_count(connection: &Connection) -> Result<Option<u64>> {
    let value: Option<Vec<u8>> = connection
        .query_row(
            "SELECT value FROM meta WHERE key=? AND
             (SELECT COUNT(*) FROM sqlite_schema WHERE type='trigger' AND name IN
              ('fs_count_insert','fs_count_delete','fs_count_update'))=3",
            [COUNT_KEY],
            |row| row.get(0),
        )
        .optional()?;
    value
        .map(|bytes| {
            let bytes: [u8; 8] = bytes
                .try_into()
                .ok()
                .context("corrupt cached entry count")?;
            Ok(u64::from_le_bytes(bytes))
        })
        .transpose()
}

pub(super) fn count_entries(connection: &Connection) -> Result<u64> {
    let count: i64 = connection.query_row("SELECT COALESCE(SUM(n),0) FROM blocks", [], |row| {
        row.get(0)
    })?;
    u64::try_from(count).context("negative entry count")
}

pub(super) fn store_entry_count(connection: &Connection, count: u64) -> Result<()> {
    // Installed only on an actual update/initialization. A no-op update of an
    // older DB remains read-only, even when its count is not cached yet.
    connection.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS fs_count_insert AFTER INSERT ON blocks BEGIN
             DELETE FROM meta WHERE key='entry_count'; END;
         CREATE TRIGGER IF NOT EXISTS fs_count_delete AFTER DELETE ON blocks BEGIN
             DELETE FROM meta WHERE key='entry_count'; END;
         CREATE TRIGGER IF NOT EXISTS fs_count_update AFTER UPDATE OF n ON blocks BEGIN
             DELETE FROM meta WHERE key='entry_count'; END;",
    )?;
    connection.execute(
        "INSERT INTO meta(key,value) VALUES (?,?)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![COUNT_KEY, count.to_le_bytes().as_slice()],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/updatedb/incremental.rs"]
mod tests;
