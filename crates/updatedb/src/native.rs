//! Native incremental scanner and index writer.
use crate::{
    config::IndexConfig,
    platform::{directory_stamp as stamp, os_string, path_bytes, path_from_bytes},
    ui::{self, Tone},
};
use anyhow::{Context, Result, bail};
use fs_core::native::{
    APPLICATION_ID, BLOCK_SIZE, MAX_BLOCK_BYTES, VERSION, grams, is_native, pack, push_varint,
    trigram_keys, unpack, validate,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
mod scan;

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
        CREATE TABLE directory_grams(directory INTEGER PRIMARY KEY, data BLOB NOT NULL);
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
    pub scan_elapsed: Duration,
    pub index_elapsed: Duration,
    pub finalize_elapsed: Duration,
}

fn filter_key(idx: &IndexConfig) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        path_bytes(&idx.root),
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
    fn new(idx: &IndexConfig) -> Self {
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
                .collect(),
        }
    }
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
) -> Result<()> {
    let keys = lists.keys().map(|&gram| u64::from(gram)).collect();
    store_directory_keys(connection, directory, keys)
}

fn store_directory_keys(connection: &Connection, directory: i64, mut keys: Vec<u64>) -> Result<()> {
    keys.sort_unstable();
    connection
        .prepare_cached("INSERT INTO directory_grams(directory,data) VALUES (?,?)")?
        .execute(params![directory, pack(&keys)])?;
    Ok(())
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

    fn finish(self, connection: &Connection, directory: i64) -> Result<()> {
        match self {
            Self::Fresh { postings, keys, .. } => {
                store_directory_keys(connection, directory, keys)?;
                // Bounds encoded accumulation, not total RSS: lookup pages,
                // one directory and one final posting remain additional costs.
                if postings.buffered_bytes >= postings.byte_limit {
                    postings.spill(connection)?;
                }
                Ok(())
            }
            Self::Update { postings, lists } => {
                postings.add(connection, directory, lists)?;
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
    ) -> Result<()> {
        store_directory_grams(connection, directory, &lists)?;
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

fn clear_postings(connection: &Connection, directory: i64) -> Result<()> {
    let data: Vec<u8> = connection
        .prepare_cached("SELECT data FROM directory_grams WHERE directory=?")?
        .query_row([directory], |r| r.get(0))?;
    let old_ids = connection
        .prepare_cached("SELECT id FROM blocks WHERE directory=? ORDER BY id")?
        .query_map([directory], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut select = connection.prepare_cached("SELECT data FROM postings WHERE gram=?")?;
    let mut update = connection.prepare_cached("UPDATE postings SET data=?,n=? WHERE gram=?")?;
    let mut delete = connection.prepare_cached("DELETE FROM postings WHERE gram=?")?;
    for gram in unpack(&data)? {
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

/// Directory mtime permits reusing its immediate entries, NOT skipping its
/// entire subtree. Every known child directory is still stat'ed independently.
pub fn update(idx: &IndexConfig, scope: Option<&Path>, progress: bool) -> Result<UpdateReport> {
    if !idx.root.is_dir() {
        bail!("index root is not a directory: {}", idx.root.display());
    }
    let actual_root = fs::canonicalize(&idx.root)?;
    let actual_database = if idx.database.exists() {
        fs::canonicalize(&idx.database)?
    } else {
        fs::canonicalize(idx.database.parent().context("database has no parent")?)?.join(
            idx.database
                .file_name()
                .context("database has no filename")?,
        )
    };
    if actual_database.starts_with(&actual_root) {
        bail!("Rust index database must be outside its root");
    }
    if scope.is_some_and(|scope| !scope.starts_with(&idx.root)) {
        bail!("update scope is outside the index root");
    }
    let existing = idx.database.exists();
    if existing && !is_native(&idx.database)? {
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
    let stored_root: Option<Vec<u8>> = transaction
        .query_row("SELECT value FROM meta WHERE key='root'", [], |r| r.get(0))
        .optional()?;
    if stored_root
        .as_deref()
        .is_some_and(|root| root != path_bytes(&idx.root).as_ref())
    {
        bail!("index root changed; use a new database path");
    }
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
    let rules = ScanRules::new(idx);
    let mut scanner = if existing {
        None
    } else {
        Some(scan::Scanner::new(&idx.root, scan_root, rules.clone())?)
    };
    let mut pending = vec![scan_root.to_path_buf()];
    let mut visited = HashSet::new();
    let mut report = UpdateReport::default();
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
        let mut postings = batch.directory(&[]);
        let mut data = root.to_vec();
        data.push(0);
        write_block(&transaction, 0, &data, 1, &[], &mut postings, &mut encoder)?;
        postings.finish(&transaction, 0)?;
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
            let Some(directory) = pending.pop() else {
                break;
            };
            let before = scan::read_stamp(&directory, &idx.root)?;
            (directory, before, None)
        };
        let sql_start = profiling.then(Instant::now);
        let previous: Option<(i64, Vec<u8>)> = if existing {
            transaction
                .prepare_cached("SELECT id,stamp FROM directories WHERE path=?")?
                .query_row([path_bytes(&directory).as_ref()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?
        } else {
            None
        };
        let id = if let Some((id, _)) = &previous {
            *id
        } else {
            transaction
                .prepare_cached("INSERT INTO directories(path,stamp) VALUES (?,?)")?
                .execute(params![path_bytes(&directory).as_ref(), &before])?;
            transaction.last_insert_rowid()
        };
        if let Some(start) = sql_start {
            directory_sql_elapsed += start.elapsed();
        }
        if existing {
            visited.insert(id);
        }
        if !force && previous.as_ref().is_some_and(|(_, old)| old == &before) {
            let children = transaction
                .prepare_cached("SELECT name FROM children WHERE directory=? ORDER BY name")?
                .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|p| os_string(p).map(|name| directory.join(name)))
                    .collect::<Result<Vec<_>>>()?,
            );
            report.reused += 1;
            report.scan_elapsed += scan_start.elapsed();
        } else {
            // Abort instead of silently deleting unreadable directories from the
            // index. Changes are committed only after the entire scan succeeds.
            let children = if let Some(children) = prefetched {
                children
            } else {
                let mut children = fs::read_dir(&directory)
                    .with_context(|| {
                        format!("cannot read {} (index unchanged)", directory.display())
                    })?
                    .map(|entry| -> Result<_> {
                        let entry = entry?;
                        let kind = entry.file_type()?.is_dir();
                        // Uses dirent.d_type, with stat only as a fallback.
                        Ok((entry.file_name(), kind))
                    })
                    .collect::<Result<Vec<_>>>()?;
                children.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                children
            };
            report.scan_elapsed += scan_start.elapsed();
            let index_start = Instant::now();
            if previous.is_some() {
                clear_postings(&transaction, id)?;
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
            for (name, is_dir) in children {
                // Keep basenames like updatedb: full paths are only necessary
                // for traversal or explicit path exclusions, not every file.
                // Symlinks are indexed as records, never followed.
                if is_dir && rules.directories.contains(&name) {
                    continue;
                }
                let path = (is_dir || !rules.paths.is_empty()).then(|| directory.join(&name));
                if path
                    .as_deref()
                    .is_some_and(|path| rules.paths.iter().any(|blocked| path.starts_with(blocked)))
                {
                    continue;
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
                    if existing {
                        pending.push(path.context("directory path missing")?);
                    }
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
            postings.finish(&transaction, id)?;
            let metadata_start = profiling.then(Instant::now);
            let after = stamp(&directory_metadata(&directory, &idx.root)?);
            if let Some(start) = metadata_start {
                post_metadata_elapsed += start.elapsed();
            }
            if before != after {
                bail!(
                    "directory changed during scan: {}; retry (index unchanged)",
                    directory.display()
                );
            }
            if previous.is_some() {
                transaction
                    .prepare_cached("UPDATE directories SET stamp=? WHERE id=?")?
                    .execute(params![before, id])?;
            }
            report.scanned += 1;
            report.index_elapsed += index_start.elapsed();
        }
        if terminal && last.elapsed() >= Duration::from_millis(250) {
            write!(
                std::io::stderr(),
                "\r\x1b[2K{}: {} scanned · {} reused dirs",
                label,
                report.scanned,
                report.reused
            )?;
            std::io::stderr().flush()?;
            last = Instant::now();
        }
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
        // leaves the same v2 lookup/uniqueness guarantees on committed indexes.
        transaction.execute_batch(
            "CREATE UNIQUE INDEX directory_paths ON directories(path);
             CREATE INDEX directory_blocks ON blocks(directory);",
        )?;
    }
    // Remove vanished subtrees only when updating an existing database.
    let old_directories = if existing {
        transaction
            .prepare("SELECT id,path FROM directories")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    for (id, path) in old_directories {
        let path = path_from_bytes(&path)?;
        if path.starts_with(scan_root) && !visited.contains(&id) {
            clear_postings(&transaction, id)?;
            transaction.execute("DELETE FROM blocks WHERE directory=?", [id])?;
            transaction.execute("DELETE FROM children WHERE directory=?", [id])?;
            transaction.execute("DELETE FROM directories WHERE id=?", [id])?;
        }
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
    ui::log(
        Tone::Success,
        format_args!(
            "{}: {} entries; {} dirs scanned, {} reused",
            idx.name, report.entries, report.scanned, report.reused
        ),
    );
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
#[path = "../../../tests/unit/updatedb/native.rs"]
mod tests;
