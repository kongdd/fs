//! Directory snapshots and filter pruning; postings are streamed only when pruning.
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::{os_string, pack, unpack};

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
    /// Apply added directory-name exclusions to the stored tree, without
    /// visiting excluded subtrees. Rewrite each affected global posting once,
    /// not once per removed directory. Only surviving parents need a readdir
    /// to remove excluded child markers from their own filename blocks.
    pub fn prune_ignored(
        &mut self,
        connection: &Connection,
        names: &HashSet<OsString>,
    ) -> Result<HashSet<i64>> {
        let names_bytes: HashSet<_> = names.iter().map(|name| name.as_encoded_bytes()).collect();
        let mut removed = Vec::new();
        let mut parents = HashSet::new();
        self.directories.retain(|path, directory| {
            let ignored = !path.is_empty()
                && path
                    .split(|&byte| byte == b'/')
                    .any(|part| names_bytes.contains(part));
            if ignored {
                removed.push(directory.id);
            } else if directory.children.iter().any(|name| names.contains(name)) {
                parents.insert(directory.id);
            }
            !ignored
        });
        if removed.is_empty() {
            return Ok(parents);
        }
        connection
            .execute_batch("CREATE TEMP TABLE ignored_directories(id INTEGER PRIMARY KEY);")?;
        {
            let mut insert =
                connection.prepare("INSERT INTO temp.ignored_directories VALUES (?)")?;
            for id in removed {
                insert.execute([id])?;
            }
        }
        let blocks = connection.prepare(
            "SELECT id FROM blocks WHERE directory IN (SELECT id FROM temp.ignored_directories)",
        )?.query_map([], |row| row.get::<_, i64>(0).map(|id| id as u64))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        if !blocks.is_empty() {
            let mut select = connection.prepare("SELECT gram,data FROM postings ORDER BY gram")?;
            let mut rows = select.query([])?;
            let mut update = connection.prepare("UPDATE postings SET data=?,n=? WHERE gram=?")?;
            let mut delete = connection.prepare("DELETE FROM postings WHERE gram=?")?;
            while let Some(row) = rows.next()? {
                let gram: i64 = row.get(0)?;
                let mut ids = unpack(row.get_ref(1)?.as_blob()?)?;
                let before = ids.len();
                ids.retain(|id| !blocks.contains(id));
                if ids.len() == before {
                    continue;
                }
                if ids.is_empty() {
                    delete.execute([gram])?;
                } else {
                    update.execute(params![pack(&ids), ids.len() as i64, gram])?;
                }
            }
        }
        connection.execute_batch(
            "DELETE FROM blocks WHERE directory IN (SELECT id FROM temp.ignored_directories);
             DELETE FROM children WHERE directory IN (SELECT id FROM temp.ignored_directories);
             DELETE FROM directory_grams WHERE directory IN (SELECT id FROM temp.ignored_directories);
             DELETE FROM directories WHERE id IN (SELECT id FROM temp.ignored_directories);
             DROP TABLE temp.ignored_directories;",
        )?;
        Ok(parents)
    }

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
