//! Bounded read-ahead for fresh builds. Only the caller touches SQLite.
use std::{
    ffi::OsString,
    fs, mem,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use super::{ScanRules, directory_metadata, stamp};

const QUEUE_SIZE: usize = 8;
const BATCH_ENTRIES: usize = 512;
const BATCH_BYTES: usize = 64 * 1024;

type Entry = (OsString, bool);

pub(super) struct Directory {
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub entries: Vec<Entry>,
}

struct Chunk {
    directory: Option<(PathBuf, Vec<u8>)>,
    entries: Vec<Entry>,
    more: bool,
}

pub(super) fn read_stamp(path: &Path, root: &Path) -> Result<Vec<u8>> {
    let metadata = directory_metadata(path, root)
        .with_context(|| format!("cannot stat {} (index unchanged)", path.display()))?;
    if !metadata.is_dir() {
        bail!("directory changed type during update: {}", path.display());
    }
    Ok(stamp(&metadata))
}

pub(super) struct Scanner {
    receiver: Option<Receiver<Result<Chunk>>>,
    worker: Option<JoinHandle<()>>,
    profile: Option<Profile>,
}

#[derive(Default)]
struct Profile {
    metadata: Duration,
    enumerate: Duration,
    sort: Duration,
    send: Duration,
    receive: Duration,
    merge: Duration,
    directories: u64,
    entries: u64,
    slowest: (Duration, PathBuf),
}

impl Scanner {
    pub fn new(root: &Path, start: &Path, rules: ScanRules) -> Result<Self> {
        let root = root.to_path_buf();
        let start = start.to_path_buf();
        let profile = ["FS_SCAN_PROFILE", "NASFIND_SCAN_PROFILE"]
            .iter()
            .any(|name| std::env::var_os(name).is_some());
        let (sender, receiver) = mpsc::sync_channel(QUEUE_SIZE);
        let worker = thread::Builder::new()
            .name("fs-scan".into())
            .spawn(move || {
                if let Err(error) = scan_tree(&root, start, &rules, &sender, profile) {
                    let _ = sender.send(Err(error));
                }
            })?;
        Ok(Self {
            receiver: Some(receiver),
            worker: Some(worker),
            profile: profile.then(Profile::default),
        })
    }

    fn receive(&mut self) -> Result<Option<Chunk>> {
        let start = self.profile.as_ref().map(|_| Instant::now());
        let message = self.receiver.as_ref().context("scanner closed")?.recv();
        if let (Some(profile), Some(start)) = (&mut self.profile, start) {
            profile.receive += start.elapsed();
        }
        match message {
            Ok(chunk) => chunk.map(Some),
            Err(_) => {
                if let Some(worker) = self.worker.take() {
                    worker
                        .join()
                        .map_err(|_| anyhow::anyhow!("directory scanner panicked"))?;
                }
                if let Some(profile) = self.profile.take() {
                    crate::ui::log(
                        crate::ui::Tone::Info,
                        format_args!(
                            "scan-detail consumer: receive {:.6}s · merge {:.6}s · sort {:.6}s",
                            profile.receive.as_secs_f64(),
                            profile.merge.as_secs_f64(),
                            profile.sort.as_secs_f64(),
                        ),
                    );
                }
                Ok(None)
            }
        }
    }

    pub fn next(&mut self) -> Result<Option<Directory>> {
        let Some(mut chunk) = self.receive()? else {
            return Ok(None);
        };
        let (path, before) = chunk.directory.take().context("missing scan directory")?;
        let mut entries = chunk.entries;
        let split = chunk.more;
        while chunk.more {
            chunk = self.receive()?.context("incomplete directory scan")?;
            if chunk.directory.is_some() {
                bail!("unexpected directory in scan continuation");
            }
            let start = self.profile.as_ref().map(|_| Instant::now());
            entries.append(&mut chunk.entries);
            if let (Some(profile), Some(start)) = (&mut self.profile, start) {
                profile.merge += start.elapsed();
            }
        }
        if split {
            let start = self.profile.as_ref().map(|_| Instant::now());
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            if let (Some(profile), Some(start)) = (&mut self.profile, start) {
                profile.sort += start.elapsed();
            }
        }
        Ok(Some(Directory {
            path,
            before,
            entries,
        }))
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        drop(self.receiver.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn send_chunk(
    sender: &SyncSender<Result<Chunk>>,
    directory: &mut Option<(PathBuf, Vec<u8>)>,
    entries: &mut Vec<Entry>,
    more: bool,
    profile: &mut Option<Profile>,
) -> Result<()> {
    let start = profile.as_ref().map(|_| Instant::now());
    let result = sender
        .send(Ok(Chunk {
            directory: directory.take(),
            entries: mem::take(entries),
            more,
        }))
        .map_err(|_| anyhow::anyhow!("directory scan cancelled"));
    if let (Some(profile), Some(start)) = (profile, start) {
        profile.send += start.elapsed();
    }
    result
}

fn scan_tree(
    root: &Path,
    start: PathBuf,
    rules: &ScanRules,
    sender: &SyncSender<Result<Chunk>>,
    profiling: bool,
) -> Result<()> {
    let mut profile = profiling.then(Profile::default);
    let mut pending = vec![start];
    while let Some(path) = pending.pop() {
        let start = profile.as_ref().map(|_| Instant::now());
        let before = read_stamp(&path, root)?;
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            profile.metadata += start.elapsed();
            profile.directories += 1;
        }
        let mut header = Some((path.clone(), before));
        let mut entries = Vec::new();
        let mut children = Vec::new();
        let mut bytes = 0;
        let mut entry_count = 0;
        let start = profile.as_ref().map(|_| Instant::now());
        let sent_before = profile.as_ref().map(|p| p.send).unwrap_or_default();
        for entry in fs::read_dir(&path)
            .with_context(|| format!("cannot read {} (index unchanged)", path.display()))?
        {
            let entry = entry?;
            let is_dir = entry.file_type()?.is_dir();
            let name = entry.file_name();
            if is_dir && !rules.directories.contains(&name) {
                let child = path.join(&name);
                if !rules.paths.iter().any(|blocked| child.starts_with(blocked)) {
                    children.push(child);
                }
            }
            bytes += name.as_encoded_bytes().len() + mem::size_of::<Entry>();
            entries.push((name, is_dir));
            entry_count += 1;
            if entries.len() >= BATCH_ENTRIES || bytes >= BATCH_BYTES {
                send_chunk(sender, &mut header, &mut entries, true, &mut profile)?;
                bytes = 0;
            }
        }
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            // Includes read_dir/file_type, allocation and pruning; excludes
            // channel send/backpressure. No per-entry timers in the hot loop.
            let elapsed = start.elapsed().saturating_sub(profile.send - sent_before);
            profile.enumerate += elapsed;
            profile.entries += entry_count;
            if elapsed > profile.slowest.0 {
                profile.slowest = (elapsed, path.clone());
            }
        }
        let start = profile.as_ref().map(|_| Instant::now());
        if header.is_some() {
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        }
        children.sort_unstable();
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            profile.sort += start.elapsed();
        }
        send_chunk(sender, &mut header, &mut entries, false, &mut profile)?;
        pending.extend(children);
    }
    if let Some(profile) = profile {
        crate::ui::log(
            crate::ui::Tone::Info,
            format_args!(
                "scan-detail worker: metadata {:.6}s · enumerate {:.6}s · sort {:.6}s · send {:.6}s · directories {} · entries {}",
                profile.metadata.as_secs_f64(),
                profile.enumerate.as_secs_f64(),
                profile.sort.as_secs_f64(),
                profile.send.as_secs_f64(),
                profile.directories,
                profile.entries,
            ),
        );
        crate::ui::log(
            crate::ui::Tone::Info,
            format_args!(
                "scan-detail slowest-enumerate: {:.6}s {}",
                profile.slowest.0.as_secs_f64(),
                profile.slowest.1.display(),
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/updatedb/scan.rs"]
mod tests;
