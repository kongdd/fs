//! Rust index storage, block readers, validation, and relative-path resolution.
use crate::config::IndexConfig;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{borrow::Cow, collections::HashMap, fs, io::Read, path::Path, time::Duration};

mod scan;

pub const APPLICATION_ID: i64 = 0x4e465231;
pub const VERSION: i64 = 4;

pub const BLOCK_SIZE: usize = 32;
pub const MAX_BLOCK_BYTES: usize = 4 * 1024 * 1024;

pub fn is_rust_index(path: &Path) -> Result<bool> {
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
        bail!(
            "unsupported fs Rust index (version={version}): {}; rebuild using a new database path",
            path.display()
        );
    }
    Ok(true)
}

pub fn open_read(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    Ok(connection)
}

pub fn validate(connection: &Connection) -> Result<()> {
    let app: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if app != APPLICATION_ID || version != VERSION {
        bail!(
            "unsupported fs Rust index (application={app}, version={version}); rebuild using a new database path"
        );
    }
    Ok(())
}

/// Maps relative directory paths using the single stored root.
pub struct DirectoryPaths {
    root: Vec<u8>,
}

impl DirectoryPaths {
    pub fn load(connection: &Connection) -> Result<Self> {
        let root: Vec<u8> =
            connection.query_row("SELECT value FROM meta WHERE key='root'", [], |r| r.get(0))?;
        Self::relative(root)
    }

    pub fn relative(root: Vec<u8>) -> Result<Self> {
        if root.is_empty() || root.contains(&0) || root.len() >= MAX_BLOCK_BYTES {
            bail!("corrupt index: invalid root");
        }
        Ok(Self { root })
    }

    pub fn encode<'a>(&self, path: &'a Path) -> Result<Cow<'a, [u8]>> {
        let relative = path
            .strip_prefix(crate::platform::path_from_bytes(&self.root)?.as_ref())
            .context("directory is outside the index root")?;
        Ok(crate::platform::path_bytes(relative))
    }

    pub fn resolve(&self, stored: Vec<u8>) -> Result<Vec<u8>> {
        if stored.contains(&0) {
            bail!("corrupt index: invalid directory path");
        }
        let root = &self.root;
        if !stored.is_empty()
            && stored.split(|&b| b == b'/').any(|part| {
                part.is_empty()
                    || part == b"."
                    || part == b".."
                    || (cfg!(windows) && (part.contains(&b'\\') || part.contains(&b':')))
            })
        {
            bail!("corrupt index: invalid directory path");
        }
        let mut path = Vec::with_capacity(root.len() + stored.len() + 1);
        path.extend_from_slice(root);
        if !stored.is_empty() {
            if !path.ends_with(b"/") {
                path.push(b'/');
            }
            path.extend_from_slice(&stored);
        }
        Ok(path)
    }
}

fn expand_root_record(root: &[u8], decoded: &mut Vec<u8>) -> Result<()> {
    if decoded.as_slice() != b"\0" {
        bail!("corrupt index: invalid root block");
    }
    decoded.clear();
    decoded.extend_from_slice(root);
    decoded.push(0);
    Ok(())
}

pub fn trigram_keys(path: &[u8]) -> impl Iterator<Item = u32> + '_ {
    path.windows(3).map(|b| {
        (u32::from(b[0].to_ascii_lowercase()) << 16)
            | (u32::from(b[1].to_ascii_lowercase()) << 8)
            | u32::from(b[2].to_ascii_lowercase())
    })
}

pub fn grams(path: &[u8]) -> Vec<u32> {
    let mut result: Vec<_> = trigram_keys(path).collect();
    result.sort_unstable();
    result.dedup();
    result
}

pub fn push_varint(data: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        data.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    data.push(value as u8);
}

pub fn pack(ids: &[u64]) -> Vec<u8> {
    let mut data = Vec::with_capacity(ids.len());
    let mut previous = 0;
    for &id in ids {
        push_varint(&mut data, id - previous);
        previous = id;
    }
    data
}

pub fn unpack(data: &[u8]) -> Result<Vec<u64>> {
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
    grams: &[u32],
    basename: bool,
    matches: impl Fn(&[u8]) -> bool + Sync,
    visitor: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    visit_filtered(idx, grams, basename, matches, |_| true, visitor, 1)
}

/// Apply a conservative basename predicate before constructing full paths.
/// The caller must accept every basename that could match the full query.
/// Only large full scans use workers; workers=1 keeps the scan serial.
/// Candidate lookups and all SQLite/output work stay on the caller's thread.
pub fn visit_filtered(
    idx: &IndexConfig,
    grams: &[u32],
    basename: bool,
    matches: impl Fn(&[u8]) -> bool + Sync,
    name_matches: impl Fn(&[u8]) -> bool + Sync,
    mut visitor: impl FnMut(&[u8]) -> Result<bool>,
    workers: usize,
) -> Result<bool> {
    let predicates = (&matches, &name_matches);
    let mut connection = open_read(&idx.database)?;
    validate(&connection)?;
    let transaction = connection.transaction()?; // one consistent read snapshot
    let mut ranked = Vec::new();
    for &gram in grams {
        let n: Option<i64> = transaction
            .query_row("SELECT n FROM postings WHERE gram=?", [gram], |r| r.get(0))
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
        let data: Vec<u8> = transaction
            .prepare_cached("SELECT data FROM postings WHERE gram=?")?
            .query_row([gram], |r| r.get(0))?;
        let ids = unpack(&data)?;
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
    if let Some(ids) = candidates {
        let mut reader = BlockReader::new()?;
        // Batch row lookups, keeping memory bounded and allowing limits to
        // stop after the first batch instead of materializing every path.
        let mut statement = transaction.prepare_cached(
            "SELECT data,directory,size,n FROM blocks WHERE id IN (SELECT value FROM json_each(?)) ORDER BY id",
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
                if !reader.visit_filtered(row, &transaction, basename, predicates, &mut visitor)? {
                    return Ok(false);
                }
            }
            if count != chunk.len() {
                bail!("corrupt posting list: missing entries");
            }
        }
    } else {
        return scan::parallel(&transaction, basename, predicates, &mut visitor, workers);
    }
    Ok(true)
}

// Metadata-only statistics for one unfiltered index. This validates row counts
// and references, but is not a full integrity scan of compressed filename data.
pub fn directory_counts(
    idx: &IndexConfig,
    mut visitor: impl FnMut(&[u8], i64) -> Result<()>,
) -> Result<Vec<u8>> {
    let mut connection = open_read(&idx.database)?;
    validate(&connection)?;
    let transaction = connection.transaction()?;
    let format = DirectoryPaths::load(&transaction)?;
    validate_root_record(&transaction)?;
    let mut statement = transaction.prepare(
        "SELECT d.path,SUM(b.n),MIN(b.n),MAX(b.n),MIN(b.size),MAX(b.size)
         FROM blocks b LEFT JOIN directories d ON d.id=b.directory
         WHERE b.directory<>0 GROUP BY b.directory",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let path: Option<Vec<u8>> = row.get(0)?;
        let path = format.resolve(path.context("corrupt index: missing block directory")?)?;
        let n: i64 = row.get(1)?;
        let min_n: i64 = row.get(2)?;
        let max_n: i64 = row.get(3)?;
        let min_size: i64 = row.get(4)?;
        let max_size: i64 = row.get(5)?;
        if path.is_empty()
            || path.contains(&0)
            || min_n <= 0
            || max_n > BLOCK_SIZE as i64
            || min_size <= 0
            || max_size > MAX_BLOCK_BYTES as i64
        {
            bail!("corrupt index: invalid directory or block metadata");
        }
        visitor(&path, n)?;
    }
    Ok(format.root)
}

fn validate_root_record(connection: &Connection) -> Result<()> {
    let roots: i64 =
        connection.query_row("SELECT count(*) FROM blocks WHERE directory=0", [], |row| {
            row.get(0)
        })?;
    if roots != 1 {
        bail!("corrupt index: missing or duplicate root block");
    }
    let (data, size, n): (Vec<u8>, i64, i64) = connection.query_row(
        "SELECT data,size,n FROM blocks WHERE directory=0",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if n != 1 || size != 1 {
        bail!("corrupt index: invalid root block");
    }
    if zstd::bulk::decompress(&data, 1)? != b"\0" {
        bail!("corrupt index: root block mismatch");
    }
    Ok(())
}

#[derive(Default)]
struct PathReader {
    format: Option<DirectoryPaths>,
    prefixes: HashMap<i64, Vec<u8>>,
    buffer: Vec<u8>,
}

pub struct BlockReader {
    paths: PathReader,
    decoded: Vec<u8>,
    decompressor: zstd::bulk::Decompressor<'static>,
}

impl BlockReader {
    pub fn new() -> Result<Self> {
        Ok(Self {
            paths: PathReader::default(),
            decoded: Vec::new(),
            decompressor: zstd::bulk::Decompressor::new()?,
        })
    }

    pub fn visit(
        &mut self,
        row: &rusqlite::Row<'_>,
        connection: &Connection,
        basename: bool,
        matches: &impl Fn(&[u8]) -> bool,
        visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<bool> {
        self.visit_filtered(row, connection, basename, (matches, &|_| true), visitor)
    }

    pub fn visit_filtered(
        &mut self,
        row: &rusqlite::Row<'_>,
        connection: &Connection,
        basename: bool,
        predicates: (&impl Fn(&[u8]) -> bool, &impl Fn(&[u8]) -> bool),
        visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<bool> {
        let (matches, name_matches) = predicates;
        let Self {
            paths,
            decoded,
            decompressor,
        } = self;
        let size = usize::try_from(row.get::<_, i64>(2)?)?;
        let n = usize::try_from(row.get::<_, i64>(3)?)?;
        let directory: i64 = row.get(1)?;
        decode_block(
            decompressor,
            decoded,
            row.get_ref(0)?.as_blob()?,
            size,
            n,
            directory,
        )?;
        if directory == 0 {
            expand_root_record(&paths.format(connection)?.root, decoded)?;
        }
        let names = decoded[..decoded.len() - 1]
            .split(|&b| b == 0)
            .filter(|name| name_matches(name) && (!basename || matches(name)));
        paths.visit(names, connection, directory, basename, matches, visitor)
    }
}

fn decode_block(
    decompressor: &mut zstd::bulk::Decompressor<'_>,
    decoded: &mut Vec<u8>,
    data: &[u8],
    size: usize,
    n: usize,
    directory: i64,
) -> Result<()> {
    if size == 0 || size > MAX_BLOCK_BYTES || n == 0 || n > BLOCK_SIZE {
        bail!("corrupt filename block: invalid size or count");
    }
    decoded.clear();
    decoded.reserve(size);
    let length = decompressor.decompress_to_buffer(data, decoded)?;
    if length != size
        || decoded.last() != Some(&0)
        || decoded.iter().filter(|&&b| b == 0).count() != n
    {
        bail!("corrupt filename block: size or count mismatch");
    }
    for name in decoded[..size - 1].split(|&b| b == 0) {
        if (name.is_empty() && directory != 0)
            || (directory != 0
                && (name.contains(&b'/') || (cfg!(windows) && name.contains(&b'\\'))))
        {
            bail!("corrupt filename block: invalid basename");
        }
    }
    Ok(())
}

impl PathReader {
    fn format(&mut self, connection: &Connection) -> Result<&DirectoryPaths> {
        if self.format.is_none() {
            self.format = Some(DirectoryPaths::load(connection)?);
        }
        Ok(self.format.as_ref().unwrap())
    }

    fn visit<'a>(
        &mut self,
        mut names: impl Iterator<Item = &'a [u8]>,
        connection: &Connection,
        directory: i64,
        basename: bool,
        matches: &impl Fn(&[u8]) -> bool,
        visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
    ) -> Result<bool> {
        let Some(first) = names.next() else {
            // Basename-only misses need no directory lookup or path allocation.
            // Block validation above still runs even when no names match.
            return Ok(true);
        };
        if directory != 0 && !self.prefixes.contains_key(&directory) {
            let stored: Vec<u8> = connection
                .prepare_cached("SELECT path FROM directories WHERE id=?")?
                .query_row([directory], |r| r.get(0))?;
            let mut prefix = self.format(connection)?.resolve(stored)?;
            if !prefix.ends_with(b"/") {
                prefix.push(b'/');
            }
            self.prefixes.insert(directory, prefix);
        }
        let Self {
            prefixes, buffer, ..
        } = self;
        for name in std::iter::once(first).chain(names) {
            buffer.clear();
            if directory != 0 {
                buffer.extend_from_slice(&prefixes[&directory]);
            }
            buffer.extend_from_slice(name);
            if (basename || matches(buffer)) && !visitor(buffer)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
