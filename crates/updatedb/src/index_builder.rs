//! Rust index builder and incremental directory scanner.
use crate::{
    config::IndexConfig,
    platform::{
        directory_stamp as stamp, os_string, path_bytes, path_from_bytes, path_starts_with,
        same_path, stamp_matches,
    },
    ui::{self, Tone},
};
use anyhow::{Context, Result, bail};
use fs_core::index_store::{
    APPLICATION_ID, BLOCK_SIZE, DirectoryPaths, MAX_BLOCK_BYTES, VERSION, grams, is_rust_index,
    pack, push_varint, trigram_keys, unpack, validate,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
mod directory_grams;
mod scan;
use directory_grams::DirectoryGrams;

fn schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!(
        "
        PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={VERSION};
        CREATE TABLE meta(key TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
        CREATE TABLE directories(id INTEGER PRIMARY KEY, path BLOB NOT NULL, stamp BLOB NOT NULL);
        CREATE TABLE blocks(id INTEGER PRIMARY KEY, directory INTEGER NOT NULL,
            data BLOB NOT NULL, size INTEGER NOT NULL, n INTEGER NOT NULL);
        CREATE TABLE children(directory INTEGER NOT NULL, name BLOB NOT NULL,
            PRIMARY KEY(directory,name)) WITHOUT ROWID;
        CREATE TABLE postings(gram INTEGER PRIMARY KEY, data BLOB NOT NULL, n INTEGER NOT NULL);
        CREATE TABLE directory_grams(directory INTEGER PRIMARY KEY, data BLOB NOT NULL,
            size INTEGER NOT NULL DEFAULT 0);
    "
    ))?;
    Ok(())
}

struct LiveLine(bool);

impl Drop for LiveLine {
    fn drop(&mut self) {
        if self.0 {
            let _ = write!(std::io::stderr(), "\r\x1b[2K");
            let _ = std::io::stderr().flush();
        }
    }
}

fn directory_metadata(path: &Path, root: &Path) -> std::io::Result<fs::Metadata> {
    if path == root {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }
}

#[derive(Debug, Default)]
pub struct UpdateReport {
    pub entries: u64,
    pub scanned: u64,
    pub reused: u64,
    pub skipped: u64,
    pub scan_elapsed: Duration,
    pub index_elapsed: Duration,
    pub finalize_elapsed: Duration,
}

fn filter_key(idx: &IndexConfig) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        &idx.filters.exclude_dirs,
        idx.filters
            .exclude_paths
            .iter()
            .map(|p| path_bytes(p))
            .collect::<Vec<_>>(),
    ))?)
}

#[derive(Clone)]
struct ScanRules {
    directories: HashSet<std::ffi::OsString>,
    paths: Vec<PathBuf>,
}

impl ScanRules {
    #[cfg(all(test, unix))]
    fn new(idx: &IndexConfig) -> Self {
        Self::with_exclusions(idx, &[])
    }

    fn with_exclusions(idx: &IndexConfig, extra: &[PathBuf]) -> Self {
        Self {
            directories: idx
                .filters
                .exclude_dirs
                .iter()
                .map(std::ffi::OsString::from)
                .collect(),
            paths: idx
                .filters
                .exclude_paths
                .iter()
                .map(|path| {
                    if path.is_absolute() {
                        path.clone()
                    } else {
                        idx.root.join(path)
                    }
                })
                .chain(index_artifacts(&idx.database))
                .chain(extra.iter().cloned())
                .collect(),
        }
    }
}

fn index_artifacts(database: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for suffix in ["", "-journal", "-wal", "-shm", ".lock"] {
        let mut name = database.as_os_str().to_os_string();
        name.push(suffix);
        paths.push(PathBuf::from(name));
    }
    if let Some(parent) = database.parent()
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".fs-"))
    {
        paths.push(parent.to_path_buf());
    }
    paths
}

fn artifact_names_in(directory: &Path, database: &Path) -> HashSet<std::ffi::OsString> {
    index_artifacts(database)
        .into_iter()
        .filter(|path| {
            path.parent()
                .is_some_and(|parent| same_path(parent, directory))
        })
        .filter_map(|path| path.file_name().map(|name| name.to_os_string()))
        .collect()
}

pub(crate) fn artifact_paths(database: &Path) -> Vec<PathBuf> {
    index_artifacts(database)
}

fn read_children(directory: &Path) -> std::io::Result<Vec<(std::ffi::OsString, bool)>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        children.push((entry.file_name(), entry.file_type()?.is_dir()));
    }
    children.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(children)
}

struct ListingFilter<'a> {
    names: HashSet<std::ffi::OsString>,
    prefixes: Vec<&'a Path>,
}

fn listing_filter<'a>(
    directory: &Path,
    database: &Path,
    rules: &'a ScanRules,
) -> ListingFilter<'a> {
    let mut names = artifact_names_in(directory, database);
    let mut prefixes = Vec::new();
    for blocked in &rules.paths {
        if blocked
            .parent()
            .is_some_and(|parent| same_path(parent, directory))
        {
            if let Some(name) = blocked.file_name() {
                names.insert(name.to_os_string());
            }
        } else if path_starts_with(blocked, directory) {
            prefixes.push(blocked.as_path());
        }
    }
    ListingFilter { names, prefixes }
}

fn entry_excluded(
    directory: &Path,
    name: &std::ffi::OsString,
    is_dir: bool,
    filter: &ListingFilter<'_>,
) -> bool {
    if filter.names.contains(name) {
        return true;
    }
    if !is_dir || filter.prefixes.is_empty() {
        return false;
    }
    let child = directory.join(name);
    filter
        .prefixes
        .iter()
        .any(|blocked| path_starts_with(&child, blocked))
}

fn write_scan_progress(
    terminal: bool,
    last: &mut Instant,
    label: &str,
    done: f64,
    expected: u64,
    started: Instant,
    current: &Path,
) -> Result<()> {
    if !terminal || last.elapsed() < Duration::from_millis(250) {
        return Ok(());
    }
    let status = scan_progress(done, expected, started.elapsed(), current);
    write!(std::io::stderr(), "\r\x1b[2K{label}: {status}")?;
    std::io::stderr().flush()?;
    *last = Instant::now();
    Ok(())
}

fn scan_progress(done: f64, expected: u64, elapsed: Duration, current: &Path) -> String {
    let rate = done / elapsed.as_secs_f64().max(0.001);
    let place = current
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| current.display().to_string());
    if expected == 0 {
        return format!("{done:.0} dirs · {rate:.0}/s · ETA unavailable · {place}");
    }
    let fraction = (done / expected as f64).clamp(0.0, 0.99);
    let filled = (fraction * 8.0) as usize;
    let eta = if done < 1.0 {
        "ETA estimating".to_string()
    } else if done >= expected as f64 {
        "past estimate".to_string()
    } else {
        let remaining = ((expected as f64 - done) / rate).ceil() as u64;
        format!("ETA ~{}m{:02}s", remaining / 60, remaining % 60)
    };
    format!(
        "[{}{}] ~{:.0}% · {done:.0}/{expected} dirs · {rate:.0}/s · {eta} · {place}",
        "=".repeat(filled),
        "-".repeat(8 - filled),
        fraction * 100.0
    )
}

fn listing_without_artifacts(
    directory: &Path,
    database: &Path,
    entries: &[(std::ffi::OsString, bool)],
) -> Vec<(std::ffi::OsString, bool)> {
    let names = artifact_names_in(directory, database);
    let mut kept: Vec<_> = entries
        .iter()
        .filter(|(name, _)| !names.contains(name))
        .cloned()
        .collect();
    kept.sort_unstable();
    kept
}

trait BlockPostings {
    fn add_gram(&mut self, gram: u32, id: u64) -> Result<()>;
    fn end_block(&mut self, _id: u64) -> Result<()> {
        Ok(())
    }
}

impl BlockPostings for HashMap<u32, Vec<u64>> {
    fn add_gram(&mut self, gram: u32, id: u64) -> Result<()> {
        let ids = self.entry(gram).or_default();
        if ids.last() != Some(&id) {
            ids.push(id);
        }
        Ok(())
    }
}

struct BlockEncoder {
    compressor: zstd::bulk::Compressor<'static>,
    compressed: Vec<u8>,
    suffix: Vec<u8>,
}

impl BlockEncoder {
    fn new() -> Result<Self> {
        Ok(Self {
            compressor: zstd::bulk::Compressor::new(3)?,
            compressed: Vec::new(),
            suffix: Vec::new(),
        })
    }
}

// Postings refer to blocks, not filenames. Grams from different names may
// intersect; exact matching after decompression removes these false positives.
fn write_block<P: BlockPostings>(
    connection: &Connection,
    directory: i64,
    data: &[u8],
    names: usize,
    prefix: &[u8],
    postings: &mut P,
    encoder: &mut BlockEncoder,
) -> Result<()> {
    if data.len() > MAX_BLOCK_BYTES {
        bail!("filename block exceeds size limit");
    }
    let BlockEncoder {
        compressor,
        compressed,
        suffix,
    } = encoder;
    compressed.clear();
    compressed.reserve(zstd::zstd_safe::compress_bound(data.len()));
    compressor.compress_to_buffer(data, compressed)?;
    connection
        .prepare_cached("INSERT INTO blocks(directory,data,size,n) VALUES (?,?,?,?)")?
        .execute(params![
            directory,
            &*compressed,
            data.len() as i64,
            names as i64
        ])?;
    let id = connection.last_insert_rowid() as u64;
    for name in data[..data.len() - 1].split(|&b| b == 0) {
        suffix.clear();
        suffix.extend_from_slice(&prefix[prefix.len().saturating_sub(2)..]);
        suffix.extend_from_slice(name);
        for gram in trigram_keys(suffix) {
            postings.add_gram(gram, id)?;
        }
    }
    postings.end_block(id)
}

fn store_directory_grams(
    connection: &Connection,
    directory: i64,
    lists: &HashMap<u32, Vec<u64>>,
    codec: &mut DirectoryGrams,
) -> Result<()> {
    let keys = lists.keys().map(|&gram| u64::from(gram)).collect();
    codec.store(connection, directory, keys)
}

#[derive(Default)]
struct EncodedPosting {
    data: Vec<u8>,
    last: u64,
    count: u64,
}

// Trigrams are 24-bit keys. Lazily allocate 256-slot pages instead of hashing
// every filename byte window. Slots contain entry index + 1; zero means absent.
// The page directory costs 512 KiB on 64-bit targets, each used page 1 KiB.
struct FreshPostingTable {
    pages: Vec<Option<Box<[u32; 256]>>>,
    entries: Vec<(u32, EncodedPosting)>,
}

impl Default for FreshPostingTable {
    fn default() -> Self {
        Self {
            pages: vec![None; 1 << 16],
            entries: Vec::new(),
        }
    }
}

impl FreshPostingTable {
    fn get_or_insert(&mut self, gram: u32) -> &mut EncodedPosting {
        debug_assert!(gram < 1 << 24);
        let page = self.pages[(gram >> 8) as usize].get_or_insert_with(|| Box::new([0; 256]));
        let slot = &mut page[(gram & 255) as usize];
        if *slot == 0 {
            self.entries.push((gram, EncodedPosting::default()));
            *slot = self.entries.len() as u32;
        }
        &mut self.entries[(*slot - 1) as usize].1
    }

    fn get(&self, gram: u32) -> Option<&EncodedPosting> {
        let page = self.pages.get((gram >> 8) as usize)?.as_ref()?;
        let slot = page[(gram & 255) as usize];
        slot.checked_sub(1)
            .map(|index| &self.entries[index as usize].1)
    }
}

// Fresh block IDs are globally increasing. Encode each delta once; never read,
// decode, sort or rewrite a previously published posting during the scan.
struct FreshPostings {
    lists: FreshPostingTable,
    buffered_bytes: usize,
    byte_limit: usize,
    segments: i64,
}

impl Default for FreshPostings {
    fn default() -> Self {
        Self {
            lists: FreshPostingTable::default(),
            buffered_bytes: 0,
            byte_limit: 16 * 1024 * 1024,
            segments: 0,
        }
    }
}

impl FreshPostings {
    fn append(&mut self, gram: u32, id: u64, first_block: u64) -> Result<bool> {
        let posting = self.lists.get_or_insert(gram);
        if posting.count > 0 {
            if id == posting.last {
                return Ok(false); // Repeated gram in the same block.
            }
            if id < posting.last {
                bail!("fresh posting IDs are not increasing");
            }
        }
        let first_in_directory = posting.count == 0 || posting.last < first_block;
        let before = posting.data.len();
        push_varint(&mut posting.data, id - posting.last);
        posting.last = id;
        posting.count += 1;
        self.buffered_bytes += posting.data.len() - before;
        Ok(first_in_directory)
    }

    // Test the encoder/spill boundary separately from filesystem traversal.
    #[cfg(all(test, unix))]
    fn add(&mut self, connection: &Connection, lists: HashMap<u32, Vec<u64>>) -> Result<()> {
        for (gram, ids) in lists {
            for id in ids {
                if self
                    .lists
                    .get(gram)
                    .is_some_and(|posting| posting.count > 0 && id <= posting.last)
                {
                    bail!("fresh posting IDs are not increasing");
                }
                self.append(gram, id, id)?;
            }
        }
        if self.buffered_bytes >= self.byte_limit {
            self.spill(connection)?;
        }
        Ok(())
    }

    fn spill(&mut self, connection: &Connection) -> Result<()> {
        if self.buffered_bytes == 0 {
            return Ok(());
        }
        if self.segments == 0 {
            connection.execute_batch(
                "CREATE TEMP TABLE fresh_posting_segments(
                    gram INTEGER NOT NULL, segment INTEGER NOT NULL, data BLOB NOT NULL,
                    PRIMARY KEY(gram,segment)) WITHOUT ROWID;
                 PRAGMA temp.cache_size=-2048;",
            )?;
        }
        let mut lists: Vec<_> = self.lists.entries.iter_mut().collect();
        lists.sort_unstable_by_key(|(gram, _)| *gram);
        let mut insert = connection.prepare_cached(
            "INSERT INTO temp.fresh_posting_segments(gram,segment,data) VALUES (?,?,?)",
        )?;
        for (gram, posting) in lists {
            if !posting.data.is_empty() {
                insert.execute(params![*gram, self.segments, &posting.data])?;
                // Keep last/count across spills; following bytes continue the
                // same delta stream, so final assembly is plain concatenation.
                posting.data = Vec::new();
            }
        }
        self.segments += 1;
        self.buffered_bytes = 0;
        Ok(())
    }

    fn finish(&mut self, connection: &Connection) -> Result<()> {
        if self.segments == 0 {
            let mut lists: Vec<_> = self.lists.entries.iter_mut().collect();
            lists.sort_unstable_by_key(|(gram, _)| *gram);
            let mut insert =
                connection.prepare_cached("INSERT INTO postings(gram,data,n) VALUES (?,?,?)")?;
            for (gram, posting) in lists {
                if posting.count > 0 {
                    insert.execute(params![*gram, &posting.data, posting.count as i64])?;
                    posting.data = Vec::new();
                }
            }
        } else {
            self.spill(connection)?;
            {
                let mut select = connection.prepare(
                    "SELECT gram,data FROM temp.fresh_posting_segments ORDER BY gram,segment",
                )?;
                let mut rows = select.query([])?;
                let mut insert = connection
                    .prepare_cached("INSERT INTO postings(gram,data,n) VALUES (?,?,?)")?;
                let mut current = None;
                let mut data = Vec::new();
                while let Some(row) = rows.next()? {
                    let gram = row.get::<_, u32>(0)?;
                    if current.is_some_and(|previous| previous != gram) {
                        let previous = current.context("missing posting gram")?;
                        insert.execute(params![
                            previous,
                            &data,
                            self.lists
                                .get(previous)
                                .context("missing posting gram")?
                                .count as i64
                        ])?;
                        data.clear();
                    }
                    current = Some(gram);
                    data.extend_from_slice(row.get_ref(1)?.as_blob()?);
                }
                if let Some(gram) = current {
                    insert.execute(params![
                        gram,
                        &data,
                        self.lists.get(gram).context("missing posting gram")?.count as i64
                    ])?;
                }
            }
            connection.execute_batch("DROP TABLE temp.fresh_posting_segments;")?;
        }
        self.buffered_bytes = 0;
        self.segments = 0;
        self.lists = FreshPostingTable::default();
        Ok(())
    }
}

enum PostingWriter {
    Fresh(FreshPostings),
    Update(PostingBatch),
}

// Fresh builds retain only one gram-key vector per directory, rather than a
// separately allocated block-ID vector for every directory/gram pair.
enum DirectoryPostings<'a> {
    Fresh {
        postings: &'a mut FreshPostings,
        prefix_grams: &'a [u32],
        keys: Vec<u64>,
        first_block: Option<u64>,
    },
    Update {
        postings: &'a mut PostingBatch,
        lists: HashMap<u32, Vec<u64>>,
    },
}

impl BlockPostings for DirectoryPostings<'_> {
    fn add_gram(&mut self, gram: u32, id: u64) -> Result<()> {
        match self {
            Self::Fresh {
                postings,
                keys,
                first_block,
                ..
            } => {
                if postings.append(gram, id, *first_block.get_or_insert(id))? {
                    keys.push(u64::from(gram));
                }
                Ok(())
            }
            Self::Update { lists, .. } => lists.add_gram(gram, id),
        }
    }

    fn end_block(&mut self, id: u64) -> Result<()> {
        if let Self::Fresh {
            postings,
            prefix_grams,
            keys,
            first_block,
        } = self
        {
            let first = *first_block.get_or_insert(id);
            for &gram in *prefix_grams {
                if postings.append(gram, id, first)? {
                    keys.push(u64::from(gram));
                }
            }
        }
        Ok(())
    }
}

impl DirectoryPostings<'_> {
    fn common_blocks(&mut self, gram: u32, ids: &[u64]) {
        if let Self::Update { lists, .. } = self {
            lists.insert(gram, ids.to_vec());
        }
    }

    fn finish(
        self,
        connection: &Connection,
        directory: i64,
        codec: &mut DirectoryGrams,
    ) -> Result<()> {
        match self {
            Self::Fresh { postings, keys, .. } => {
                codec.store(connection, directory, keys)?;
                // Bounds encoded accumulation, not total RSS: lookup pages,
                // one directory and one final posting remain additional costs.
                if postings.buffered_bytes >= postings.byte_limit {
                    postings.spill(connection)?;
                }
                Ok(())
            }
            Self::Update { postings, lists } => {
                postings.add(connection, directory, lists, codec)?;
                postings.flush(connection)
            }
        }
    }
}

impl PostingWriter {
    fn directory<'a>(&'a mut self, prefix_grams: &'a [u32]) -> DirectoryPostings<'a> {
        match self {
            Self::Fresh(postings) => DirectoryPostings::Fresh {
                postings,
                prefix_grams,
                keys: Vec::new(),
                first_block: None,
            },
            Self::Update(postings) => DirectoryPostings::Update {
                postings,
                lists: HashMap::new(),
            },
        }
    }

    fn flush(&mut self, connection: &Connection) -> Result<()> {
        match self {
            Self::Fresh(postings) => postings.finish(connection),
            Self::Update(postings) => postings.flush(connection),
        }
    }
}

#[derive(Default)]
struct PostingBatch {
    lists: HashMap<u32, Vec<u64>>,
    count: usize,
}

impl PostingBatch {
    fn add(
        &mut self,
        connection: &Connection,
        directory: i64,
        lists: HashMap<u32, Vec<u64>>,
        codec: &mut DirectoryGrams,
    ) -> Result<()> {
        store_directory_grams(connection, directory, &lists, codec)?;
        for (gram, ids) in lists {
            self.count += ids.len();
            self.lists.entry(gram).or_default().extend(ids);
        }
        // Bound cross-directory accumulation; this is not a total RSS limit.
        if self.count >= 262_144 {
            self.flush(connection)?;
        }
        Ok(())
    }

    fn flush(&mut self, connection: &Connection) -> Result<()> {
        let mut lists: Vec<_> = self.lists.drain().collect();
        lists.sort_unstable_by_key(|(gram, _)| *gram);
        let mut select = connection.prepare_cached("SELECT data FROM postings WHERE gram=?")?;
        let mut insert = connection.prepare_cached("INSERT INTO postings(gram,data,n) VALUES (?,?,?) ON CONFLICT(gram) DO UPDATE SET data=excluded.data,n=excluded.n")?;
        for (gram, mut ids) in lists {
            if let Some(data) = select
                .query_row([gram], |r| r.get::<_, Vec<u8>>(0))
                .optional()?
            {
                let mut previous = unpack(&data)?;
                previous.append(&mut ids);
                ids = previous;
            }
            ids.sort_unstable();
            ids.dedup();
            insert.execute(params![gram, pack(&ids), ids.len() as i64])?;
        }
        self.count = 0;
        Ok(())
    }
}

fn clear_postings(
    connection: &Connection,
    directory: i64,
    codec: &mut DirectoryGrams,
) -> Result<()> {
    let keys = codec.read(connection, directory)?;
    let old_ids = connection
        .prepare_cached("SELECT id FROM blocks WHERE directory=? ORDER BY id")?
        .query_map([directory], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut select = connection.prepare_cached("SELECT data FROM postings WHERE gram=?")?;
    let mut update = connection.prepare_cached("UPDATE postings SET data=?,n=? WHERE gram=?")?;
    let mut delete = connection.prepare_cached("DELETE FROM postings WHERE gram=?")?;
    for gram in keys {
        let data: Vec<u8> = select.query_row([gram as i64], |r| r.get(0))?;
        let mut ids = unpack(&data)?;
        ids.retain(|&id| old_ids.binary_search(&(id as i64)).is_err());
        if ids.is_empty() {
            delete.execute([gram as i64])?;
        } else {
            update.execute(params![pack(&ids), ids.len() as i64, gram as i64])?;
        }
    }
    connection.execute("DELETE FROM directory_grams WHERE directory=?", [directory])?;
    Ok(())
}

fn retain_indexed(
    transaction: &rusqlite::Transaction,
    paths: &DirectoryPaths,
    directory: &Path,
    id: i64,
    visited: &mut HashSet<i64>,
) -> Result<()> {
    let mut stack = vec![(id, directory.to_path_buf())];
    while let Some((id, directory)) = stack.pop() {
        visited.insert(id);
        let names = transaction
            .prepare_cached("SELECT name FROM children WHERE directory=?")?
            .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for name in names {
            let child = directory.join(os_string(name)?);
            let stored = paths.encode(&child)?;
            let child_id: Option<i64> = transaction
                .prepare_cached("SELECT id FROM directories WHERE path=?")?
                .query_row([stored.as_ref()], |r| r.get(0))
                .optional()?;
            if let Some(child_id) = child_id
                && !visited.contains(&child_id)
            {
                stack.push((child_id, child));
            }
        }
    }
    Ok(())
}

fn scoped_directories(connection: &Connection, mut scope: &[u8]) -> Result<Vec<(i64, Vec<u8>)>> {
    while let Some(parent) = scope.strip_suffix(b"/") {
        scope = parent;
    }
    let row = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?));
    if scope.is_empty() {
        return Ok(connection
            .prepare("SELECT id,path FROM directories")?
            .query_map([], row)?
            .collect::<rusqlite::Result<_>>()?);
    }
    let mut lower = scope.to_vec();
    lower.push(b'/');
    let mut upper = lower.clone();
    // All descendants lie in [scope + "/", scope + "0").
    *upper.last_mut().unwrap() = b'0';
    Ok(connection
        .prepare("SELECT id,path FROM directories WHERE path=?1 OR (path>=?2 AND path<?3)")?
        .query_map(params![scope, lower, upper], row)?
        .collect::<rusqlite::Result<_>>()?)
}

/// Directory mtime permits reusing its immediate entries, NOT skipping its
/// entire subtree. Every known child directory is still stat'ed independently.
pub fn update(idx: &IndexConfig, scope: Option<&Path>, progress: bool) -> Result<UpdateReport> {
    update_with_jobs(idx, scope, progress, 1)
}

pub fn update_with_jobs(
    idx: &IndexConfig,
    scope: Option<&Path>,
    progress: bool,
    jobs: u32,
) -> Result<UpdateReport> {
    update_with_exclusions(idx, scope, progress, jobs, &[])
}

pub fn update_with_exclusions(
    idx: &IndexConfig,
    scope: Option<&Path>,
    progress: bool,
    jobs: u32,
    extra_exclusions: &[PathBuf],
) -> Result<UpdateReport> {
    if !idx.root.is_dir() {
        bail!("index root is not a directory: {}", idx.root.display());
    }
    if scope.is_some_and(|scope| !scope.starts_with(&idx.root)) {
        bail!("update scope is outside the index root");
    }
    let existing = idx.database.exists();
    if existing && !is_rust_index(&idx.database)? {
        bail!(
            "{} is a legacy plocate index; use a NEW database path for the Rust engine, or --engine plocate",
            idx.database.display()
        );
    }
    let mut encoder = BlockEncoder::new()?;
    let mut connection = Connection::open(&idx.database)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    // DELETE journaling keeps each committed snapshot in the DB file itself;
    // stats fingerprints and copied indexes don't depend on WAL sidecars.
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-32768;
         PRAGMA temp_store=FILE;",
    )?;
    let transaction = connection.transaction()?;
    if !existing {
        schema(&transaction)?;
    } else {
        validate(&transaction)?;
    }
    let mut gram_codec = DirectoryGrams::new(&transaction)?;
    if !existing {
        gram_codec.stage_fresh(&transaction)?;
    }
    let stored_root: Option<Vec<u8>> = transaction
        .query_row("SELECT value FROM meta WHERE key='root'", [], |r| r.get(0))
        .optional()?;
    if existing && stored_root.is_none() {
        bail!("corrupt index: missing root");
    }
    if stored_root
        .as_deref()
        .is_some_and(|root| root != path_bytes(&idx.root).as_ref())
    {
        bail!("index root changed; use a new database path");
    }
    let paths = DirectoryPaths::relative(path_bytes(&idx.root).to_vec())?;
    let key = filter_key(idx)?;
    let old_key: Option<Vec<u8>> = transaction
        .query_row("SELECT value FROM meta WHERE key='filters'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if scope.is_some() && old_key.as_ref() != Some(&key) {
        bail!("root or directory exclusions changed; run a full index update before --folder");
    }
    let force = old_key.as_ref() != Some(&key);
    let scan_root = scope.unwrap_or(&idx.root);
    let rules = ScanRules::with_exclusions(idx, extra_exclusions);
    let mut scanner = if existing {
        None
    } else {
        Some(scan::Scanner::with_jobs(
            &idx.root,
            scan_root,
            rules.clone(),
            jobs,
        )?)
    };
    let mut pending = vec![scan_root.to_path_buf()];
    let mut stat_pool =
        (existing && jobs > 1).then(|| scan::StatPool::new(&idx.root, jobs as usize));
    let mut visited = HashSet::new();
    let mut report = UpdateReport::default();
    if progress {
        ui::enable_ansi();
    }
    let expected_dirs: u64 = if existing {
        transaction
            .query_row("SELECT COUNT(*) FROM directories", [], |row| row.get(0))
            .map(|count: i64| count.max(0) as u64)?
    } else {
        0
    };
    let progress_started = Instant::now();
    let terminal = progress && std::io::stderr().is_terminal();
    let live_line = LiveLine(terminal);
    let label: String = idx.name.chars().take(12).collect();
    let label = ui::paint(&label, Tone::Info, ui::stderr_color());
    let mut last = Instant::now();
    let mut batch = if existing {
        PostingWriter::Update(PostingBatch::default())
    } else {
        PostingWriter::Fresh(FreshPostings::default())
    };
    // Root itself is an indexed record, but never a traversal child.
    if !existing {
        let root = path_bytes(&idx.root);
        transaction.execute(
            "INSERT INTO meta(key,value) VALUES ('root',?)",
            [root.as_ref()],
        )?;
        let root_grams = grams(&root);
        let mut postings = batch.directory(&root_grams);
        // Root is a marker; the full root string lives only in meta.root.
        write_block(&transaction, 0, b"\0", 1, &[], &mut postings, &mut encoder)?;
        postings.finish(&transaction, 0, &mut gram_codec)?;
    }
    let profiling = ["FS_SCAN_PROFILE", "NASFIND_SCAN_PROFILE"]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let mut directory_sql_elapsed = Duration::ZERO;
    let mut post_metadata_elapsed = Duration::ZERO;
    let mut data = Vec::new();
    loop {
        let scan_start = Instant::now();
        let (directory, before, prefetched) = if let Some(scanner) = &mut scanner {
            let Some(directory) = scanner.next()? else {
                break;
            };
            (directory.path, directory.before, Some(directory.entries))
        } else {
            if let Some(pool) = &mut stat_pool {
                pool.prefetch(&pending);
            }
            let Some(directory) = pending.pop() else {
                break;
            };
            let before = match stat_pool
                .as_mut()
                .map(|pool| pool.take(&directory))
                .unwrap_or_else(|| scan::stamp_io(&directory, &idx.root))
            {
                Ok(stamp) => stamp,
                Err(error) => {
                    let vanished = matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    );
                    if directory == scan_root && vanished {
                        return Err(error).with_context(|| {
                            format!("cannot read {} (index unchanged)", directory.display())
                        });
                    }
                    scan::warn_unreadable(&directory, &error);
                    report.skipped += 1;
                    if existing && !vanished {
                        let stored = paths.encode(&directory)?;
                        let id: Option<i64> = transaction
                            .prepare_cached("SELECT id FROM directories WHERE path=?")?
                            .query_row([stored.as_ref()], |row| row.get(0))
                            .optional()?;
                        if let Some(id) = id {
                            retain_indexed(&transaction, &paths, &directory, id, &mut visited)?;
                        }
                    }
                    continue;
                }
            };
            (directory, before, None)
        };
        let sql_start = profiling.then(Instant::now);
        let stored_path = paths.encode(&directory)?;
        let previous: Option<(i64, Vec<u8>)> = if existing {
            transaction
                .prepare_cached("SELECT id,stamp FROM directories WHERE path=?")?
                .query_row([stored_path.as_ref()], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
        } else {
            None
        };
        let id = if let Some((id, _)) = &previous {
            *id
        } else {
            transaction
                .prepare_cached("INSERT INTO directories(path,stamp) VALUES (?,?)")?
                .execute(params![stored_path.as_ref(), &before])?;
            transaction.last_insert_rowid()
        };
        if let Some(start) = sql_start {
            directory_sql_elapsed += start.elapsed();
        }
        if existing {
            visited.insert(id);
        }
        if !force
            && previous
                .as_ref()
                .is_some_and(|(_, old)| stamp_matches(old, &before))
        {
            let children = transaction
                .prepare_cached("SELECT name FROM children WHERE directory=? ORDER BY name")?
                .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let children = children
                .into_iter()
                .rev()
                .map(|p| os_string(p).map(|name| directory.join(name)))
                .collect::<Result<Vec<_>>>()?;
            pending.extend(children);
            report.reused += 1;
            report.scan_elapsed += scan_start.elapsed();
        } else {
            // An unreadable directory keeps its previous entries. Deleting them
            // would treat a permission error as a removal.
            let children = if let Some(children) = prefetched {
                children
            } else {
                write_scan_progress(
                    terminal,
                    &mut last,
                    &label,
                    (report.scanned + report.reused + report.skipped) as f64,
                    expected_dirs,
                    progress_started,
                    &directory,
                )?;
                match read_children(&directory) {
                    Ok(children) => children,
                    Err(error) => {
                        let vanished = matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        );
                        if directory == scan_root && vanished {
                            return Err(error).with_context(|| {
                                format!("cannot read {} (index unchanged)", directory.display())
                            });
                        }
                        scan::warn_unreadable(&directory, &error);
                        report.skipped += 1;
                        if previous.is_some() && !vanished {
                            retain_indexed(&transaction, &paths, &directory, id, &mut visited)?;
                        } else if existing {
                            visited.remove(&id);
                        }
                        continue;
                    }
                }
            };
            report.scan_elapsed += scan_start.elapsed();
            let index_start = Instant::now();
            if previous.is_some() {
                clear_postings(&transaction, id, &mut gram_codec)?;
                transaction.execute("DELETE FROM blocks WHERE directory=?", [id])?;
                transaction.execute("DELETE FROM children WHERE directory=?", [id])?;
            }
            // Hash accumulation avoids a tree lookup for every filename gram.
            // Sort only the final unique keys before writing SQLite pages.
            let mut prefix = path_bytes(&directory).to_vec();
            if !prefix.ends_with(b"/") {
                prefix.push(b'/');
            }
            let common_grams = grams(&prefix);
            let mut postings = batch.directory(&common_grams);
            let mut block_ids = Vec::new();
            data.clear();
            let mut names = 0;
            let indexed_children = children.clone();
            let filter = listing_filter(&directory, &idx.database, &rules);
            let child_total = children.len().max(1) as f64;
            for (index, (name, is_dir)) in children.into_iter().enumerate() {
                // Keep basenames like updatedb: full paths are only necessary
                // for traversal or explicit path exclusions, not every file.
                // Symlinks are indexed as records, never followed.
                if is_dir && rules.directories.contains(&name) {
                    continue;
                }
                if entry_excluded(&directory, &name, is_dir, &filter) {
                    continue;
                }
                if index.is_multiple_of(256) {
                    let partial = index as f64 / child_total;
                    write_scan_progress(
                        terminal,
                        &mut last,
                        &label,
                        (report.scanned + report.reused + report.skipped) as f64 + partial,
                        expected_dirs,
                        progress_started,
                        &directory,
                    )?;
                }
                if is_dir && existing {
                    let child = directory.join(&name);
                    pending.push(child);
                }
                let name = name.as_encoded_bytes();
                data.extend_from_slice(name);
                data.push(0);
                names += 1;
                if names == BLOCK_SIZE {
                    write_block(
                        &transaction,
                        id,
                        &data,
                        names,
                        &prefix,
                        &mut postings,
                        &mut encoder,
                    )?;
                    data.clear();
                    names = 0;
                    if existing {
                        block_ids.push(transaction.last_insert_rowid() as u64);
                    }
                }
                if is_dir {
                    transaction
                        .prepare_cached("INSERT INTO children(directory,name) VALUES (?,?)")?
                        .execute(params![id, name])?;
                }
            }
            if names > 0 {
                write_block(
                    &transaction,
                    id,
                    &data,
                    names,
                    &prefix,
                    &mut postings,
                    &mut encoder,
                )?;
                if existing {
                    block_ids.push(transaction.last_insert_rowid() as u64);
                }
            }
            if !block_ids.is_empty() {
                for &gram in &common_grams {
                    postings.common_blocks(gram, &block_ids);
                }
            }
            postings.finish(&transaction, id, &mut gram_codec)?;
            let metadata_start = profiling.then(Instant::now);
            let after = stamp(&directory_metadata(&directory, &idx.root)?);
            if let Some(start) = metadata_start {
                post_metadata_elapsed += start.elapsed();
            }
            if !stamp_matches(&before, &after) {
                let unchanged =
                    listing_without_artifacts(&directory, &idx.database, &indexed_children);
                let reread = read_children(&directory)
                    .map(|entries| {
                        listing_without_artifacts(&directory, &idx.database, &entries) == unchanged
                    })
                    .unwrap_or(false);
                if artifact_names_in(&directory, &idx.database).is_empty() || !reread {
                    bail!(
                        "directory changed during scan: {}; retry (index unchanged)",
                        directory.display()
                    );
                }
            }
            if previous.is_some() {
                transaction
                    .prepare_cached("UPDATE directories SET stamp=? WHERE id=?")?
                    .execute(params![before, id])?;
            }
            report.scanned += 1;
            report.index_elapsed += index_start.elapsed();
        }
        write_scan_progress(
            terminal,
            &mut last,
            &label,
            (report.scanned + report.reused + report.skipped) as f64,
            expected_dirs,
            progress_started,
            &directory,
        )?;
    }
    if let Some(scanner) = &scanner {
        report.skipped += scanner.skipped();
    }
    drop(scanner);
    if profiling {
        ui::log(
            Tone::Info,
            format_args!(
                "scan-detail writer: directory_sql {:.6}s · post_metadata {:.6}s",
                directory_sql_elapsed.as_secs_f64(),
                post_metadata_elapsed.as_secs_f64(),
            ),
        );
    }
    let finalize_start = Instant::now();
    batch.flush(&transaction)?;
    if !existing {
        // Bulk index construction avoids per-directory B-tree maintenance and
        // leaves the same lookup/uniqueness guarantees on committed indexes.
        transaction.execute_batch(
            "CREATE UNIQUE INDEX directory_paths ON directories(path);
             CREATE INDEX directory_blocks ON blocks(directory);",
        )?;
    }
    // Query only the updated subtree; BLOB ranges preserve raw filename bytes.
    let stored_scope = paths.encode(scan_root)?;
    let old_directories = if existing {
        scoped_directories(&transaction, &stored_scope)?
    } else {
        Vec::new()
    };
    let stored_scope = path_from_bytes(&stored_scope)?;
    for (id, path) in old_directories {
        let path = path_from_bytes(&path)?;
        if path.starts_with(stored_scope.as_ref()) && !visited.contains(&id) {
            clear_postings(&transaction, id, &mut gram_codec)?;
            transaction.execute("DELETE FROM blocks WHERE directory=?", [id])?;
            transaction.execute("DELETE FROM children WHERE directory=?", [id])?;
            transaction.execute("DELETE FROM directories WHERE id=?", [id])?;
        }
    }
    if !existing {
        gram_codec.finish_fresh(&transaction)?;
    }
    if force {
        transaction.execute("INSERT INTO meta(key,value) VALUES ('filters',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&key])?;
    }
    report.entries = transaction.query_row("SELECT COALESCE(SUM(n),0) FROM blocks", [], |r| {
        r.get::<_, i64>(0)
    })? as u64;
    transaction.commit()?;
    report.finalize_elapsed = finalize_start.elapsed();
    drop(live_line);
    let summary = if report.skipped == 0 {
        format!(
            "{}: {} entries; {} dirs scanned, {} reused",
            idx.name, report.entries, report.scanned, report.reused
        )
    } else {
        format!(
            "{}: {} entries; {} dirs scanned, {} reused, {} unreadable skipped",
            idx.name, report.entries, report.scanned, report.reused, report.skipped
        )
    };
    ui::log(Tone::Success, format_args!("{summary}"));
    ui::log(
        Tone::Info,
        format_args!(
            "{} timings: scan {:.3}s · index {:.3}s · finalize {:.3}s",
            idx.name,
            report.scan_elapsed.as_secs_f64(),
            report.index_elapsed.as_secs_f64(),
            report.finalize_elapsed.as_secs_f64()
        ),
    );
    Ok(report)
}

#[cfg(all(test, unix))]
#[path = "../../../tests/unit/updatedb/index_builder.rs"]
mod tests;

#[cfg(test)]
mod progress_tests {
    use super::scan_progress;
    use std::{path::Path, time::Duration};

    #[test]
    fn ratio_uses_previous_directory_count() {
        let status = scan_progress(
            50.0,
            100,
            Duration::from_secs(10),
            Path::new("/data/Windows"),
        );
        assert!(status.contains("~50%"), "{status}");
        assert!(status.contains("50/100"), "{status}");
        assert!(status.contains("ETA ~0m10s"), "{status}");
        assert!(status.contains("Windows"), "{status}");
        let fresh = scan_progress(12.0, 0, Duration::from_secs(2), Path::new("c:/"));
        assert!(fresh.contains("ETA unavailable"), "{fresh}");
    }
}
