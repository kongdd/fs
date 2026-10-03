use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::{Config, Counts, Rank, StatsOptions};
use crate::platform::{file_identity, path_bytes};

pub(super) struct Cached {
    pub(super) id: i64,
    pub(super) snapshot: Vec<u8>,
    total: i64,
    largest: i64,
}

pub(super) fn open(cfg: &Config) -> Result<Connection> {
    let parent = cfg
        .index
        .first()
        .context("no indexes configured")?
        .database
        .parent()
        .context("database has no parent")?;
    let path = parent.join("stats.db");
    if cfg.index.iter().any(|idx| idx.database == path) {
        bail!(
            "stats.db is reserved for the statistics cache: {}",
            path.display()
        );
    }
    fs::create_dir_all(parent)?;
    let connection = Connection::open(&path)
        .with_context(|| format!("cannot open statistics cache {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(30))?;
    schema(&connection)?;
    Ok(connection)
}

pub(super) fn schema(connection: &Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 0..=2) {
        bail!("unsupported stats.db schema version {version}");
    }
    connection.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA cache_size=-32768;")?;
    if version == 2 {
        return Ok(());
    }
    connection.execute_batch("PRAGMA journal_mode=WAL;")?;
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 2 {
        return Ok(());
    }
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS selections (
            id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE,
            snapshot BLOB NOT NULL, total INTEGER NOT NULL, largest INTEGER NOT NULL
        );
        CREATE TABLE directories (
            id INTEGER PRIMARY KEY, path BLOB NOT NULL,
            parent INTEGER, path_hash INTEGER NOT NULL
        );
        CREATE INDEX path_lookup ON directories(path_hash);
        CREATE INDEX directory_children ON directories(parent);
        CREATE TABLE counts_v2 (
            selection INTEGER NOT NULL, directory INTEGER NOT NULL,
            direct_count INTEGER NOT NULL, recursive_count INTEGER NOT NULL,
            PRIMARY KEY (selection, directory)
        ) WITHOUT ROWID;
    ",
    )?;
    if version == 1 {
        let paths = transaction
            .prepare("SELECT DISTINCT path FROM counts ORDER BY path")?
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut dictionary = HashMap::new();
        for path in paths {
            intern_directory(&transaction, &mut dictionary, &path)?;
        }
        transaction.execute_batch(
            "
            INSERT INTO counts_v2
                SELECT c.selection, d.id, c.direct_count, c.recursive_count
                FROM counts c JOIN directories d ON d.path=c.path;
            DROP TABLE counts;
        ",
        )?;
    }
    transaction.execute_batch(
        "
        ALTER TABLE counts_v2 RENAME TO counts;
        CREATE INDEX recursive_rank ON counts(selection, recursive_count DESC, directory);
        CREATE INDEX direct_rank ON counts(selection, direct_count DESC, directory);
        PRAGMA user_version=2;
    ",
    )?;
    transaction.commit()?;
    if version == 1 {
        connection.execute_batch("VACUUM;")?;
    }
    Ok(())
}

// Stable FNV-1a key: the index stores only integers. Full-byte comparison below
// handles hash collisions, so neither correctness nor Unicode relies on the hash.
fn path_hash(path: &[u8]) -> i64 {
    path.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }) as i64
}

fn intern_directory(
    connection: &Connection,
    dictionary: &mut HashMap<Vec<u8>, i64>,
    path: &[u8],
) -> rusqlite::Result<i64> {
    if let Some(&id) = dictionary.get(path) {
        return Ok(id);
    }
    let parent = if super::is_root(path) && path != b"/" && path.iter().all(|&b| b == b'/') {
        Some(b"/".as_slice())
    } else {
        super::parent_path(path).filter(|parent| *parent != path)
    };
    let parent = parent
        .map(|parent| intern_directory(connection, dictionary, parent))
        .transpose()?;
    connection
        .prepare_cached("INSERT INTO directories(path, parent, path_hash) VALUES (?1, ?2, ?3)")?
        .execute(params![path, parent, path_hash(path)])?;
    let id = connection.last_insert_rowid();
    dictionary.insert(path.to_vec(), id);
    Ok(id)
}

pub(super) fn fingerprint(cfg: &Config, indexes: &[String]) -> Result<(String, Vec<u8>)> {
    let selected = cfg.select(indexes)?;
    let key = serde_json::to_string(&selected.iter().map(|idx| &idx.name).collect::<Vec<_>>())?;
    let mut sources = Vec::new();
    for idx in selected {
        let metadata = fs::metadata(&idx.database).with_context(|| {
            format!(
                "database for index {} is unavailable; run nasfind index update",
                idx.name
            )
        })?;
        let (modified, nanos, identity) = file_identity(&metadata);
        sources.push((
            path_bytes(&idx.root),
            path_bytes(&idx.database),
            metadata.len(),
            modified,
            nanos,
            identity,
            &idx.filters.exclude_dirs,
            idx.filters
                .exclude_paths
                .iter()
                .map(|path| path_bytes(path))
                .collect::<Vec<_>>(),
            &idx.filters.exclude_extensions,
            &idx.filters.exclude_files,
        ));
    }
    let snapshot = serde_json::to_vec(&(1, &cfg.tools.plocate, sources))?;
    Ok((key, snapshot))
}

pub(super) fn metadata(connection: &Connection, id: i64) -> Result<Cached> {
    Ok(connection.query_row(
        "SELECT total, largest, snapshot FROM selections WHERE id=?",
        [id],
        |row| {
            Ok(Cached {
                id,
                total: row.get(0)?,
                largest: row.get(1)?,
                snapshot: row.get(2)?,
            })
        },
    )?)
}

pub(super) fn ensure(
    connection: &mut Connection,
    cfg: &Config,
    indexes: &[String],
) -> Result<Cached> {
    let (key, snapshot) = fingerprint(cfg, indexes)?;
    let previous = connection
        .query_row(
            "SELECT id, snapshot FROM selections WHERE key=?",
            [&key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    if let Some((id, previous)) = previous
        && previous == snapshot
    {
        return metadata(connection, id);
    }
    crate::ui::log(
        crate::ui::Tone::Info,
        format_args!("building stats cache: {key}"),
    );
    let start = Instant::now();
    let counts = super::collect(cfg, indexes)?;
    if fingerprint(cfg, indexes)?.1 != snapshot {
        bail!("index databases changed while building statistics; retry the command");
    }
    let directories = counts.directories.len();
    let cached = store(connection, &key, &snapshot, counts)?;
    crate::ui::log(
        crate::ui::Tone::Success,
        format_args!(
            "stats cached: {directories} directories in {:.2}s",
            start.elapsed().as_secs_f64()
        ),
    );
    Ok(cached)
}

pub(super) fn store(
    connection: &mut Connection,
    key: &str,
    snapshot: &[u8],
    counts: Counts,
) -> Result<Cached> {
    let largest = counts
        .nodes
        .iter()
        .map(|node| node.recursive)
        .max()
        .unwrap_or(0);
    let transaction = connection.transaction()?;
    transaction.execute(
        "
        INSERT INTO selections(key, snapshot, total, largest) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(key) DO UPDATE SET snapshot=excluded.snapshot,
            total=excluded.total, largest=excluded.largest",
        params![key, snapshot, counts.total, largest],
    )?;
    let id = transaction.query_row("SELECT id FROM selections WHERE key=?", [key], |row| {
        row.get(0)
    })?;
    transaction.execute("DELETE FROM counts WHERE selection=?", [id])?;
    {
        let mut dictionary = transaction
            .prepare("SELECT path, id FROM directories")?
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        let mut insert = transaction.prepare("INSERT INTO counts VALUES (?1, ?2, ?3, ?4)")?;
        let mut directories: Vec<_> = counts.directories.into_iter().collect();
        directories.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (path, node) in directories {
            let node = &counts.nodes[node];
            let directory = intern_directory(&transaction, &mut dictionary, &path)?;
            insert.execute(params![id, directory, node.direct, node.recursive])?;
        }
    }
    // Retain paths referenced by any selection and their structural ancestors.
    transaction.execute_batch(
        "
        WITH RECURSIVE live(id) AS (
            SELECT directory FROM counts
            UNION SELECT d.parent FROM directories d JOIN live ON d.id=live.id
                WHERE d.parent IS NOT NULL
        ) DELETE FROM directories WHERE id NOT IN (SELECT id FROM live);
    ",
    )?;
    transaction.commit()?;
    Ok(Cached {
        id,
        total: counts.total,
        largest,
        snapshot: snapshot.to_vec(),
    })
}

pub(super) fn total(connection: &Connection, cached: &Cached, root: Option<&Path>) -> Result<i64> {
    match root {
        None => Ok(cached.total),
        Some(root) if root == Path::new("/") => Ok(cached.total),
        Some(root) => Ok(connection
            .query_row(
                "SELECT c.recursive_count FROM directories d JOIN counts c ON c.directory=d.id
                    WHERE c.selection=?1 AND d.path_hash=?2 AND d.path=?3",
                params![
                    cached.id,
                    path_hash(&path_bytes(root)),
                    path_bytes(root).as_ref()
                ],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0)),
    }
}

pub(super) fn ranked(
    connection: &Connection,
    cached: &Cached,
    options: &StatsOptions,
    root: Option<&Path>,
) -> Result<Vec<Rank>> {
    let column = if options.recursive {
        "recursive_count"
    } else {
        "direct_count"
    };
    let base = format!(
        "SELECT d.path, c.{column} FROM counts c JOIN directories d ON d.id=c.directory
        WHERE c.selection=?1 AND c.{column}>0"
    );
    let order = format!(" ORDER BY c.{column} DESC, d.path ASC LIMIT ?2");
    let top = i64::try_from(options.top.get()).unwrap_or(i64::MAX);
    let decode = |row: &rusqlite::Row<'_>| {
        Ok(Rank {
            path: row.get(0)?,
            count: row.get(1)?,
        })
    };
    let rows = if let Some(root) = root {
        let bytes = path_bytes(root);
        let root = bytes.as_ref();
        // Integer parent links find descendants without repeating path strings in indexes.
        let sql = format!(
            "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM directories WHERE path_hash=?3 AND path=?4
            UNION ALL SELECT d.id FROM directories d JOIN subtree s ON d.parent=s.id
        ) {base} AND c.directory IN (SELECT id FROM subtree){order}"
        );
        connection
            .prepare(&sql)?
            .query_map(params![cached.id, top, path_hash(root), root], decode)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else if options.recursive {
        let sql = format!("{base} AND c.{column}<?3{order}");
        connection
            .prepare(&sql)?
            .query_map(params![cached.id, top, cached.largest], decode)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        connection
            .prepare(&(base + &order))?
            .query_map(params![cached.id, top], decode)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}
