//! Bounded, ordered full scans. SQLite and all path/output work stay on the
//! caller's single read snapshot; workers only validate/decode/filter bytes.
use super::{BLOCK_SIZE, BlockReader, MAX_BLOCK_BYTES, decode_block, expand_root_record};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Row};
use std::{sync::mpsc, thread};

const WARMUP_BLOCKS: usize = 512;
const BATCH_BLOCKS: usize = 64;
const BATCH_BYTES: usize = 512 * 1024;
const MAX_WORKERS: usize = 7;

struct EncodedBlock {
    directory: i64,
    size: usize,
    n: usize,
    data: Vec<u8>,
}

impl EncodedBlock {
    fn from_row(row: &Row<'_>) -> Result<Self> {
        let size = usize::try_from(row.get::<_, i64>(2)?)?;
        let n = usize::try_from(row.get::<_, i64>(3)?)?;
        if size == 0 || size > MAX_BLOCK_BYTES || n == 0 || n > BLOCK_SIZE {
            bail!("corrupt filename block: invalid size or count");
        }
        let data = row.get_ref(0)?.as_blob()?;
        // Do not copy an unbounded blob out of a potentially corrupt DB.
        if data.len() > zstd::zstd_safe::compress_bound(MAX_BLOCK_BYTES) {
            bail!("corrupt filename block: excessive compressed size");
        }
        Ok(Self {
            directory: row.get(1)?,
            size,
            n,
            data: data.to_vec(),
        })
    }

    fn filter(
        self,
        decoder: &mut zstd::bulk::Decompressor<'_>,
        decoded: &mut Vec<u8>,
        basename: bool,
        root: &[u8],
        predicates: (&impl Fn(&[u8]) -> bool, &impl Fn(&[u8]) -> bool),
    ) -> Result<FilteredBlock> {
        decode_block(
            decoder,
            decoded,
            &self.data,
            self.size,
            self.n,
            self.directory,
        )?;
        if self.directory == 0 {
            expand_root_record(root, decoded)?;
        }
        let (matches, name_matches) = predicates;
        let mut names = Vec::new();
        for name in decoded[..decoded.len() - 1].split(|&b| b == 0) {
            if name_matches(name) && (!basename || matches(name)) {
                names.extend_from_slice(name);
                names.push(0);
            }
        }
        Ok(FilteredBlock {
            directory: self.directory,
            names,
        })
    }
}

struct FilteredBlock {
    directory: i64,
    names: Vec<u8>,
}

pub(super) fn sequential(
    connection: &Connection,
    mut reader: BlockReader,
    basename: bool,
    predicates: (&impl Fn(&[u8]) -> bool, &impl Fn(&[u8]) -> bool),
    visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    let mut statement =
        connection.prepare("SELECT data,directory,size,n FROM blocks ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if !reader.visit_filtered(row, connection, basename, predicates, visitor)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn parallel(
    connection: &Connection,
    mut reader: BlockReader,
    basename: bool,
    predicates: (
        &(impl Fn(&[u8]) -> bool + Sync),
        &(impl Fn(&[u8]) -> bool + Sync),
    ),
    visitor: &mut impl FnMut(&[u8]) -> Result<bool>,
    workers: usize,
) -> Result<bool> {
    let workers = workers.clamp(1, MAX_WORKERS);
    if workers == 1 {
        return sequential(connection, reader, basename, predicates, visitor);
    }
    let mut statement =
        connection.prepare("SELECT data,directory,size,n FROM blocks ORDER BY id")?;
    let mut rows = statement.query([])?;
    // Small indexes and queries that reach their limit early pay no thread or
    // blob-copy overhead. Do not COUNT(*) or pre-read the whole index.
    for _ in 0..WARMUP_BLOCKS {
        let Some(row) = rows.next()? else {
            return Ok(true);
        };
        if !reader.visit_filtered(row, connection, basename, predicates, visitor)? {
            return Ok(false);
        }
    }
    let Some(row) = rows.next()? else {
        return Ok(true);
    };
    let mut pending = Some(EncodedBlock::from_row(row));
    let (matches, _) = predicates;
    let root = reader.paths.format(connection)?.root.clone();
    let root = root.as_slice();
    thread::scope(|scope| {
        let mut pipes = Vec::new();
        let mut handles = Vec::new();
        for number in 0..workers {
            let (send, receive) = mpsc::sync_channel::<Vec<Result<EncodedBlock>>>(1);
            let (result_send, result_receive) = mpsc::sync_channel::<Vec<Result<FilteredBlock>>>(1);
            let mut decoder = zstd::bulk::Decompressor::new()?;
            handles.push(
                thread::Builder::new()
                    .name(format!("fs-scan-{number}"))
                    .spawn_scoped(scope, move || {
                        let mut decoded = Vec::new();
                        while let Ok(batch) = receive.recv() {
                            let mut result = Vec::with_capacity(batch.len());
                            for block in batch {
                                let filtered = block.and_then(|block| {
                                    block.filter(
                                        &mut decoder,
                                        &mut decoded,
                                        basename,
                                        root,
                                        predicates,
                                    )
                                });
                                let failed = filtered.is_err();
                                result.push(filtered);
                                if failed {
                                    break;
                                }
                            }
                            // Keep an error at its original block position:
                            // an earlier limit must not inspect later corrupt blocks.
                            if result_send.send(result).is_err() {
                                break;
                            }
                        }
                    })?,
            );
            pipes.push((send, result_receive));
        }
        let outcome = (|| {
            let mut ended = false;
            while !ended {
                let mut submitted = 0;
                for (sender, _) in &pipes {
                    let mut batch = Vec::with_capacity(BATCH_BLOCKS);
                    let mut bytes = 0;
                    while batch.len() < BATCH_BLOCKS && bytes < BATCH_BYTES {
                        let block = if let Some(block) = pending.take() {
                            block
                        } else {
                            match rows.next() {
                                Ok(Some(row)) => EncodedBlock::from_row(row),
                                Ok(None) => {
                                    ended = true;
                                    break;
                                }
                                Err(error) => Err(error.into()),
                            }
                        };
                        if let Ok(block) = &block {
                            bytes += block.size + block.data.len();
                        } else {
                            ended = true;
                        }
                        batch.push(block);
                        if ended {
                            break;
                        }
                    }
                    if batch.is_empty() {
                        break;
                    }
                    sender.send(batch).context("scan worker disconnected")?;
                    submitted += 1;
                    if ended {
                        break;
                    }
                }
                // One round in flight, consumed in submission order. A slow
                // first batch cannot cause an unbounded reorder buffer.
                for (_, receiver) in pipes.iter().take(submitted) {
                    for block in receiver.recv().context("scan worker disconnected")? {
                        let block = block?;
                        if block.names.is_empty() {
                            continue;
                        }
                        if !reader.paths.visit(
                            block.names[..block.names.len() - 1].split(|&b| b == 0),
                            connection,
                            block.directory,
                            basename,
                            matches,
                            visitor,
                        )? {
                            return Ok(false);
                        }
                    }
                }
            }
            Ok(true)
        })();
        // Disconnect BOTH directions before joining, including errors/limits.
        // Workers never block forever sending an abandoned result.
        drop(pipes);
        let mut panicked = false;
        for handle in handles {
            panicked |= handle.join().is_err();
        }
        if panicked {
            bail!("scan worker panicked");
        }
        outcome
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/core/scan.rs"]
mod tests;
