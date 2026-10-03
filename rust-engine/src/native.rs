//! Byte-safe Rust search engine. SQLite provides durable transactions and page
//! lookup; matching, directory reuse and compressed trigram postings are ours.
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use regex::bytes::{Regex, RegexBuilder};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::{
    config::IndexConfig,
    search::SearchOptions,
    ui::{self, Tone},
};

const APPLICATION_ID: i64 = 0x4e465231;
const VERSION: i64 = 1;

pub fn is_native(path: &Path) -> Result<bool> {
    let mut header = [0; 100];
    let mut file = fs::File::open(path)?;
    let length = file.read(&mut header)?;
    if length < 16 || &header[..16] != b"SQLite format 3\0" {
        return Ok(false);
    }
    if length < 100 {
        bail!("truncated SQLite index: {}", path.display());
    }
    let application = u32::from_be_bytes(header[68..72].try_into()?);
    let version = u32::from_be_bytes(header[60..64].try_into()?);
    if i64::from(application) != APPLICATION_ID || i64::from(version) != VERSION {
        bail!("not a supported nasfind Rust index: {}", path.display());
    }
    Ok(true)
}

fn open_read(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    Ok(connection)
}

fn validate(connection: &Connection) -> Result<()> {
    let app: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if app != APPLICATION_ID || version != VERSION {
        bail!("not a supported nasfind Rust index (application={app}, version={version})");
    }
    Ok(())
}

fn schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(&format!("
        PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={VERSION};
        CREATE TABLE meta(key TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
        CREATE TABLE directories(id INTEGER PRIMARY KEY, path BLOB NOT NULL UNIQUE, stamp BLOB NOT NULL);
        CREATE TABLE entries(id INTEGER PRIMARY KEY, name BLOB NOT NULL, directory INTEGER NOT NULL, kind INTEGER NOT NULL);
        CREATE VIEW paths AS SELECT e.id,e.directory,e.kind,
            CASE WHEN e.directory=0 THEN e.name
                 ELSE CAST((CASE WHEN substr(d.path,-1,1)=X'2f' THEN d.path ELSE d.path || '/' END) || e.name AS BLOB)
            END AS path
            FROM entries e LEFT JOIN directories d ON e.directory=d.id;
        CREATE TABLE postings(gram INTEGER NOT NULL, directory INTEGER NOT NULL, data BLOB NOT NULL, n INTEGER NOT NULL,
            PRIMARY KEY(gram,directory)) WITHOUT ROWID;
        CREATE TABLE frequencies(gram INTEGER PRIMARY KEY, n INTEGER NOT NULL);
    "))?;
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
        idx.root.as_os_str().as_bytes(),
        &idx.filters.exclude_dirs,
        idx.filters
            .exclude_paths
            .iter()
            .map(|p| p.as_os_str().as_bytes())
            .collect::<Vec<_>>(),
    ))?)
}

fn stamp(metadata: &fs::Metadata) -> Vec<u8> {
    [
        metadata.dev(),
        metadata.ino(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ]
    .into_iter()
    .flat_map(u64::to_le_bytes)
    .collect()
}

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

fn trigram_keys(path: &[u8]) -> impl Iterator<Item = u32> + '_ {
    path.windows(3).map(|b| {
        (u32::from(b[0].to_ascii_lowercase()) << 16)
            | (u32::from(b[1].to_ascii_lowercase()) << 8)
            | u32::from(b[2].to_ascii_lowercase())
    })
}

fn grams(path: &[u8]) -> Vec<u32> {
    let mut result: Vec<_> = trigram_keys(path).collect();
    result.sort_unstable();
    result.dedup();
    result
}

fn pack(ids: &[u64]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut previous = 0;
    for &id in ids {
        let mut delta = id - previous;
        while delta >= 128 {
            data.push((delta as u8 & 127) | 128);
            delta >>= 7;
        }
        data.push(delta as u8);
        previous = id;
    }
    data
}

fn unpack(data: &[u8]) -> Result<Vec<u64>> {
    let mut ids = Vec::new();
    let (mut previous, mut delta, mut shift) = (0_u64, 0_u64, 0_u32);
    for &byte in data {
        if shift > 63 || (shift == 63 && byte > 1) {
            bail!("corrupt posting list: varint overflow");
        }
        delta |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            previous = previous.checked_add(delta).context("posting ID overflow")?;
            if ids.last().is_some_and(|&last| previous <= last) {
                bail!("corrupt posting list: IDs are not increasing");
            }
            ids.push(previous);
            delta = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    if shift != 0 {
        bail!("corrupt posting list: truncated varint");
    }
    Ok(ids)
}

fn clear_postings(connection: &Connection, directory: i64) -> Result<()> {
    connection.execute("UPDATE frequencies SET n=n-COALESCE((SELECT n FROM postings WHERE gram=frequencies.gram AND directory=?1),0)
        WHERE gram IN (SELECT gram FROM postings WHERE directory=?1)", [directory])?;
    connection.execute("DELETE FROM frequencies WHERE n=0 AND gram IN (SELECT gram FROM postings WHERE directory=?)", [directory])?;
    connection.execute("DELETE FROM postings WHERE directory=?", [directory])?;
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
    let mut connection = Connection::open(&idx.database)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    // DELETE journaling keeps each committed snapshot in the DB file itself;
    // stats fingerprints and copied indexes don't depend on WAL sidecars.
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-32768;",
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
        .is_some_and(|root| root != idx.root.as_os_str().as_bytes())
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
    let mut pending = vec![scan_root.to_path_buf()];
    let mut visited = HashSet::new();
    let mut report = UpdateReport::default();
    let terminal = progress && std::io::stderr().is_terminal();
    let live_line = LiveLine(terminal);
    let label: String = idx.name.chars().take(12).collect();
    let label = ui::paint(&label, Tone::Info, ui::stderr_color());
    let mut last = Instant::now();
    // Root itself is an indexed record, but never a traversal child.
    if !existing {
        let root = idx.root.as_os_str().as_bytes();
        transaction.execute("INSERT INTO meta(key,value) VALUES ('root',?)", [root])?;
        transaction.execute(
            "INSERT INTO entries(name,directory,kind) VALUES (?,0,1)",
            [root],
        )?;
        let root_id = transaction.last_insert_rowid() as u64;
        for gram in grams(root) {
            transaction.execute(
                "INSERT INTO postings(gram,directory,data,n) VALUES (?,0,?,1)",
                params![gram, pack(&[root_id])],
            )?;
        }
    }
    while let Some(directory) = pending.pop() {
        let scan_start = Instant::now();
        let metadata = directory_metadata(&directory, &idx.root)
            .with_context(|| format!("cannot stat {} (index unchanged)", directory.display()))?;
        if !metadata.is_dir() {
            bail!(
                "directory changed type during update: {}",
                directory.display()
            );
        }
        let before = stamp(&metadata);
        let previous: Option<(i64, Vec<u8>)> = if existing {
            transaction
                .prepare_cached("SELECT id,stamp FROM directories WHERE path=?")?
                .query_row([directory.as_os_str().as_bytes()], |r| {
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
                .execute(params![directory.as_os_str().as_bytes(), &before])?;
            transaction.last_insert_rowid()
        };
        if existing {
            visited.insert(id);
        }
        if !force && previous.as_ref().is_some_and(|(_, old)| old == &before) {
            let children = transaction
                .prepare_cached(
                    "SELECT path FROM paths WHERE directory=? AND kind=1 ORDER BY path",
                )?
                .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|p| PathBuf::from(std::ffi::OsString::from_vec(p))),
            );
            report.reused += 1;
            report.scan_elapsed += scan_start.elapsed();
        } else {
            // Abort instead of silently deleting unreadable directories from the
            // index. Changes are committed only after the entire scan succeeds.
            let mut children = fs::read_dir(&directory)
                .with_context(|| format!("cannot read {} (index unchanged)", directory.display()))?
                .map(|entry| -> Result<_> {
                    let entry = entry?;
                    let kind = entry.file_type()?.is_dir();
                    // Linux std uses dirent.d_type here, with a stat fallback
                    // only when the filesystem cannot supply the type.
                    Ok((entry.file_name(), kind))
                })
                .collect::<Result<Vec<_>>>()?;
            children.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            report.scan_elapsed += scan_start.elapsed();
            let index_start = Instant::now();
            if previous.is_some() {
                clear_postings(&transaction, id)?;
                transaction.execute("DELETE FROM entries WHERE directory=?", [id])?;
            }
            // Hash accumulation avoids a tree lookup for every filename gram.
            // Sort only the final unique keys before writing SQLite pages.
            let mut postings: HashMap<u32, Vec<u64>> = HashMap::new();
            let mut prefix = directory.as_os_str().as_bytes().to_vec();
            if !prefix.ends_with(b"/") {
                prefix.push(b'/');
            }
            let common_grams = grams(&prefix);
            let mut entry_count = 0_i64;
            let mut entry_insert = transaction
                .prepare_cached("INSERT INTO entries(name,directory,kind) VALUES (?,?,?)")?;
            let mut suffix = Vec::new();
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
                let name = name.as_bytes();
                entry_insert.execute(params![name, id, i64::from(is_dir)])?;
                let entry_id = transaction.last_insert_rowid() as u64;
                entry_count += 1;
                // Include the prefix/name boundary without re-tokenizing the
                // entire common directory prefix for every entry.
                suffix.clear();
                suffix.extend_from_slice(&prefix[prefix.len().saturating_sub(2)..]);
                suffix.extend_from_slice(name);
                for gram in trigram_keys(&suffix) {
                    if common_grams.binary_search(&gram).is_err() {
                        let ids = postings.entry(gram).or_default();
                        // IDs are monotonic. Last-ID comparison deduplicates
                        // repeated trigrams without sorting each filename.
                        if ids.last() != Some(&entry_id) {
                            ids.push(entry_id);
                        }
                    }
                }
                if is_dir {
                    pending.push(path.context("directory path missing")?);
                }
            }
            drop(entry_insert);
            {
                let mut insert = transaction.prepare_cached(
                    "INSERT INTO postings(gram,directory,data,n) VALUES (?,?,?,?)",
                )?;
                let mut frequency = if existing {
                    Some(transaction.prepare_cached("INSERT INTO frequencies(gram,n) VALUES (?,?) ON CONFLICT(gram) DO UPDATE SET n=n+excluded.n")?)
                } else {
                    None
                };
                // Empty posting data denotes every immediate entry.
                if entry_count > 0 {
                    for gram in common_grams {
                        insert.execute(params![gram, id, &[] as &[u8], entry_count])?;
                        if let Some(frequency) = &mut frequency {
                            frequency.execute(params![gram, entry_count])?;
                        }
                    }
                }
                let mut postings: Vec<_> = postings.into_iter().collect();
                postings.sort_unstable_by_key(|(gram, _)| *gram);
                for (gram, ids) in postings {
                    let n = ids.len() as i64;
                    insert.execute(params![gram, id, pack(&ids), n])?;
                    if let Some(frequency) = &mut frequency {
                        frequency.execute(params![gram, n])?;
                    }
                }
            }
            let after = stamp(&directory_metadata(&directory, &idx.root)?);
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
    let finalize_start = Instant::now();
    if !existing {
        // Bulk build: one ordered aggregation, then secondary-index creation.
        // Keep FULL durability and the same schema as incremental updates.
        transaction.execute_batch(
            "INSERT INTO frequencies SELECT gram,SUM(n) FROM postings GROUP BY gram;
            CREATE INDEX directory_entries ON entries(directory,kind);
            CREATE INDEX directory_postings ON postings(directory);",
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
        let path = Path::new(std::ffi::OsStr::from_bytes(&path));
        if path.starts_with(scan_root) && !visited.contains(&id) {
            clear_postings(&transaction, id)?;
            transaction.execute("DELETE FROM entries WHERE directory=?", [id])?;
            transaction.execute("DELETE FROM directories WHERE id=?", [id])?;
        }
    }
    if force {
        transaction.execute("INSERT INTO meta(key,value) VALUES ('filters',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&key])?;
    }
    report.entries =
        transaction.query_row("SELECT count(*) FROM entries", [], |r| r.get::<_, i64>(0))? as u64;
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

use std::io::IsTerminal;
use std::os::unix::ffi::OsStringExt;

pub struct Query {
    matchers: Vec<Matcher>,
    grams: Vec<u32>,
    basename: bool,
    ignore_case: bool,
}

enum Matcher {
    Literal(Vec<u8>),
    Regex(Regex),
}

impl Query {
    pub fn new(options: &SearchOptions) -> Result<Self> {
        let mut matchers = Vec::new();
        let mut candidates = Vec::new();
        for pattern in &options.patterns {
            let bytes = pattern.as_bytes();
            if options.regex {
                let expression = pattern.to_str().context("regex patterns must be UTF-8")?;
                matchers.push(Matcher::Regex(
                    RegexBuilder::new(expression)
                        .unicode(false)
                        .case_insensitive(options.ignore_case)
                        .build()
                        .context("invalid regular expression")?,
                ));
                // Regex alternatives/quantifiers aren't necessarily mandatory
                // literals. Full scan rather than an unsound candidate shortcut.
            } else if bytes.iter().any(|b| matches!(b, b'*' | b'?' | b'[')) {
                let expression = glob_regex(bytes)?;
                matchers.push(Matcher::Regex(
                    RegexBuilder::new(&expression)
                        .unicode(false)
                        .case_insensitive(options.ignore_case)
                        .build()?,
                ));
                // Only runs outside character classes are mandatory.
                let mut run = Vec::new();
                let mut in_class = false;
                for &byte in bytes {
                    if byte == b'[' {
                        candidates.extend(grams(&run));
                        run.clear();
                        in_class = true;
                    } else if byte == b']' && in_class {
                        in_class = false;
                    } else if in_class {
                        continue;
                    } else if matches!(byte, b'*' | b'?') {
                        candidates.extend(grams(&run));
                        run.clear();
                    } else {
                        run.push(byte);
                    }
                }
                candidates.extend(grams(&run));
            } else {
                candidates.extend(grams(bytes));
                matchers.push(Matcher::Literal(bytes.to_vec()));
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        Ok(Self {
            matchers,
            grams: candidates,
            basename: options.basename,
            ignore_case: options.ignore_case,
        })
    }

    pub(crate) fn matches(&self, path: &[u8]) -> bool {
        let value = if self.basename {
            path.rsplit(|b| *b == b'/').next().unwrap_or(path)
        } else {
            path
        };
        self.matchers.iter().all(|matcher| match matcher {
            Matcher::Literal(needle) => {
                needle.is_empty()
                    || value.windows(needle.len()).any(|window| {
                        if self.ignore_case {
                            window.eq_ignore_ascii_case(needle)
                        } else {
                            window == needle
                        }
                    })
            }
            Matcher::Regex(regex) => regex.is_match(value),
        })
    }
}

// Glob matching is anchored like plocate; non-glob text is substring matching.
fn glob_regex(pattern: &[u8]) -> Result<String> {
    let mut result = String::from("(?s)^");
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'*' => result.push_str(".*"),
            b'?' => result.push('.'),
            b'[' => {
                result.push('[');
                i += 1;
                if i == pattern.len() {
                    bail!("unclosed glob character class");
                }
                if matches!(pattern[i], b'!' | b'^') {
                    result.push('^');
                    i += 1;
                }
                let start = i;
                while i < pattern.len() && pattern[i] != b']' {
                    if pattern[i] == b'-' {
                        result.push('-');
                    } else {
                        result.push_str(&format!("\\x{:02x}", pattern[i]));
                    }
                    i += 1;
                }
                if i == pattern.len() || i == start {
                    bail!("invalid glob character class");
                }
                result.push(']');
            }
            byte => result.push_str(&format!("\\x{byte:02x}")),
        }
        i += 1;
    }
    result.push('$');
    Ok(result)
}

fn intersect(left: &[u64], right: &[u64]) -> Vec<u64> {
    let (mut a, mut b) = (0, 0);
    let mut out = Vec::new();
    while a < left.len() && b < right.len() {
        match left[a].cmp(&right[b]) {
            std::cmp::Ordering::Less => a += 1,
            std::cmp::Ordering::Greater => b += 1,
            std::cmp::Ordering::Equal => {
                out.push(left[a]);
                a += 1;
                b += 1;
            }
        }
    }
    out
}

/// Returns false when the caller reached its result limit. No NAS access.
pub fn visit(
    idx: &IndexConfig,
    query: &Query,
    mut visitor: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    let mut connection = open_read(&idx.database)?;
    validate(&connection)?;
    let transaction = connection.transaction()?; // one consistent read snapshot
    let mut ranked = Vec::new();
    for &gram in &query.grams {
        let n: Option<i64> = transaction
            .query_row("SELECT n FROM frequencies WHERE gram=?", [gram], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(n) = n else {
            return Ok(true);
        };
        ranked.push((n, gram));
    }
    ranked.sort_unstable();
    // The rarest trigram reduces candidates first. Intersect at most 6 lists;
    // exact matching is always authoritative (trigrams admit false positives).
    let mut candidates: Option<Vec<u64>> = None;
    for (_, gram) in ranked.into_iter().take(6) {
        let mut ids = Vec::new();
        let mut statement =
            transaction.prepare_cached("SELECT data,directory FROM postings WHERE gram=?")?;
        let mut rows = statement.query([gram])?;
        while let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            if data.is_empty() {
                let directory: i64 = row.get(1)?;
                ids.extend(
                    transaction
                        .prepare_cached("SELECT id FROM entries WHERE directory=? ORDER BY id")?
                        .query_map([directory], |r| r.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                        .into_iter()
                        .map(|id| id as u64),
                );
            } else {
                ids.extend(unpack(&data)?);
            }
        }
        ids.sort_unstable();
        let previous_length = candidates.as_ref().map(Vec::len);
        candidates = Some(match candidates {
            Some(previous) => intersect(&previous, &ids),
            None => ids,
        });
        if candidates.as_ref().is_some_and(Vec::is_empty) {
            return Ok(true);
        }
        if candidates.as_ref().is_some_and(|ids| {
            ids.len() <= 64
                || previous_length
                    .is_some_and(|previous| ids.len() >= previous.saturating_mul(3) / 4)
        }) {
            break; // Further intersections are unlikely to pay for their I/O.
        }
    }
    let mut prefixes = HashMap::new();
    let mut buffer = Vec::new();
    if let Some(ids) = candidates {
        // Batch row lookups, keeping memory bounded and allowing limits to
        // stop after the first batch instead of materializing every path.
        let mut statement = transaction.prepare_cached(
            "SELECT name,directory FROM entries WHERE id IN (SELECT value FROM json_each(?)) ORDER BY id",
        )?;
        for chunk in ids.chunks(512) {
            if chunk.iter().any(|&id| id > i64::MAX as u64) {
                bail!("posting ID exceeds SQLite range");
            }
            let encoded = serde_json::to_string(chunk)?;
            let mut rows = statement.query([encoded])?;
            let mut count = 0;
            while let Some(row) = rows.next()? {
                count += 1;
                if !visit_row(
                    row,
                    &transaction,
                    query,
                    &mut prefixes,
                    &mut buffer,
                    &mut visitor,
                )? {
                    return Ok(false);
                }
            }
            if count != chunk.len() {
                bail!("corrupt posting list: missing entries");
            }
        }
    } else {
        let mut statement =
            transaction.prepare("SELECT name,directory FROM entries ORDER BY id")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if !visit_row(
                row,
                &transaction,
                query,
                &mut prefixes,
                &mut buffer,
                &mut visitor,
            )? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn visit_row(
    row: &rusqlite::Row<'_>,
    connection: &Connection,
    query: &Query,
    prefixes: &mut HashMap<i64, Vec<u8>>,
    buffer: &mut Vec<u8>,
    visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    let name = row.get_ref(0)?.as_blob()?;
    if query.basename && !query.matches(name) {
        return Ok(true);
    }
    let directory: i64 = row.get(1)?;
    buffer.clear();
    if directory != 0 {
        if let std::collections::hash_map::Entry::Vacant(entry) = prefixes.entry(directory) {
            entry.insert(
                connection
                    .prepare_cached("SELECT path FROM directories WHERE id=?")?
                    .query_row([directory], |r| r.get(0))?,
            );
        }
        buffer.extend_from_slice(&prefixes[&directory]);
        if !buffer.ends_with(b"/") {
            buffer.push(b'/');
        }
    }
    buffer.extend_from_slice(name);
    if query.basename || query.matches(buffer) {
        visitor(buffer)
    } else {
        Ok(true)
    }
}

#[cfg(test)]
#[path = "../tests/unit/native.rs"]
mod tests;
