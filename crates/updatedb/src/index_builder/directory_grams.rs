//! Compression of update-only reverse gram lists; search never reads these.
use anyhow::{Context, Result, bail};
use fs_core::index_store::{pack, unpack};
use rusqlite::{Connection, OptionalExtension, params};

const DICTIONARY_KEY: &str = "directory_grams_dictionary";
const DICTIONARY_BYTES: usize = 16 * 1024;
const SAMPLE_BYTES: usize = 1024 * 1024;
// A sorted set of 24-bit keys takes at most one byte per key plus three
// extra bytes for its first delta. Bound allocations from untrusted metadata.
const MAX_BYTES: usize = (1 << 24) + 3;

pub(super) struct DirectoryGrams {
    staging: bool,
    compressor: Option<zstd::bulk::Compressor<'static>>,
    decompressor: Option<zstd::bulk::Decompressor<'static>>,
}

impl DirectoryGrams {
    pub(super) fn new(connection: &Connection) -> Result<Self> {
        let dictionary: Option<Vec<u8>> = connection
            .query_row(
                "SELECT value FROM meta WHERE key=?",
                [DICTIONARY_KEY],
                |r| r.get(0),
            )
            .optional()?;
        Self::with_dictionary(dictionary.as_deref())
    }

    fn with_dictionary(dictionary: Option<&[u8]>) -> Result<Self> {
        if dictionary.is_some_and(|d| d.is_empty() || d.len() > DICTIONARY_BYTES) {
            bail!("corrupt directory grams: invalid dictionary size");
        }
        Ok(Self {
            staging: false,
            compressor: dictionary
                .map(|d| zstd::bulk::Compressor::with_dictionary(3, d))
                .transpose()?,
            decompressor: dictionary
                .map(zstd::bulk::Decompressor::with_dictionary)
                .transpose()?,
        })
    }

    pub(super) fn stage_fresh(&mut self, connection: &Connection) -> Result<()> {
        // Do not allocate permanent pages for raw lists that will later shrink.
        // temp_store=FILE keeps this bounded without vacuuming the whole DB.
        connection.execute_batch(
            "CREATE TEMP TABLE fresh_directory_grams(
                directory INTEGER PRIMARY KEY, data BLOB NOT NULL, size INTEGER NOT NULL DEFAULT 0);",
        )?;
        self.staging = true;
        Ok(())
    }

    fn encode(&mut self, raw: Vec<u8>) -> Result<(Vec<u8>, usize)> {
        if raw.len() >= 64
            && let Some(compressor) = &mut self.compressor
        {
            let compressed = compressor.compress(&raw)?;
            if compressed.len() + 4 < raw.len() {
                return Ok((compressed, raw.len()));
            }
        }
        Ok((raw, 0))
    }

    fn decode(&mut self, data: &[u8], size: i64) -> Result<Vec<u64>> {
        let size = usize::try_from(size).context("corrupt directory grams: negative size")?;
        if size > MAX_BYTES || data.len() > MAX_BYTES {
            bail!("corrupt directory grams: oversized list");
        }
        let decoded;
        let raw = if size == 0 {
            data
        } else {
            decoded = self
                .decompressor
                .as_mut()
                .context("corrupt directory grams: missing dictionary")?
                .decompress(data, size)?;
            if decoded.len() != size {
                bail!("corrupt directory grams: size mismatch");
            }
            &decoded
        };
        let keys = unpack(raw)?;
        if keys.last().is_some_and(|&key| key > 0xffffff) {
            bail!("corrupt directory grams: key exceeds 24 bits");
        }
        Ok(keys)
    }

    pub(super) fn store(
        &mut self,
        connection: &Connection,
        directory: i64,
        mut keys: Vec<u64>,
    ) -> Result<()> {
        keys.sort_unstable();
        let (data, size) = self.encode(pack(&keys))?;
        let sql = if self.staging {
            "INSERT INTO temp.fresh_directory_grams(directory,data,size) VALUES (?,?,?)"
        } else {
            "INSERT INTO directory_grams(directory,data,size) VALUES (?,?,?)"
        };
        connection
            .prepare_cached(sql)?
            .execute(params![directory, data, size as i64])?;
        Ok(())
    }

    pub(super) fn read(&mut self, connection: &Connection, directory: i64) -> Result<Vec<u64>> {
        let (data, size): (Vec<u8>, i64) = connection
            .prepare_cached("SELECT data,size FROM directory_grams WHERE directory=?")?
            .query_row([directory], |r| Ok((r.get(0)?, r.get(1)?)))?;
        self.decode(&data, size)
    }

    /// Train once after a fresh build; incremental updates keep the dictionary.
    fn train(connection: &Connection) -> Result<Option<Vec<u8>>> {
        let mut samples = Vec::new();
        let mut bytes = 0;
        let mut statement = connection.prepare(
            "SELECT data FROM temp.fresh_directory_grams WHERE length(data) BETWEEN 64 AND 8192
             ORDER BY (directory * 2654435761) % 4294967296 LIMIT 4096",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let data: Vec<u8> = row.get(0)?;
            if bytes + data.len() > SAMPLE_BYTES {
                break;
            }
            bytes += data.len();
            samples.push(data);
        }
        // Small indexes stay raw: training overhead and a dictionary would
        // outweigh the savings. Training failure likewise leaves valid raw rows.
        if samples.len() < 64 || bytes < DICTIONARY_BYTES * 8 {
            return Ok(None);
        }
        Ok(zstd::dict::from_samples(&samples, DICTIONARY_BYTES).ok())
    }

    pub(super) fn finish_fresh(&mut self, connection: &Connection) -> Result<()> {
        debug_assert!(self.staging);
        let Some(dictionary) = Self::train(connection)? else {
            connection.execute_batch(
                "INSERT INTO directory_grams SELECT * FROM temp.fresh_directory_grams;
                 DROP TABLE temp.fresh_directory_grams;",
            )?;
            self.staging = false;
            return Ok(());
        };
        *self = Self::with_dictionary(Some(&dictionary))?;
        connection.execute(
            "INSERT INTO meta(key,value) VALUES (?,?)",
            params![DICTIONARY_KEY, dictionary],
        )?;
        // Page through IDs; memory stays bounded by one row and the sample.
        let mut last = -1;
        loop {
            let row: Option<(i64, Vec<u8>)> = connection
                .prepare_cached("SELECT directory,data FROM temp.fresh_directory_grams WHERE directory>? ORDER BY directory LIMIT 1")?
                .query_row([last], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((directory, raw)) = row else { break };
            let (data, size) = self.encode(raw)?;
            connection
                .prepare_cached("INSERT INTO directory_grams(directory,data,size) VALUES (?,?,?)")?
                .execute(params![directory, data, size as i64])?;
            last = directory;
        }
        connection.execute_batch("DROP TABLE temp.fresh_directory_grams;")?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/updatedb/directory_grams.rs"]
mod tests;
