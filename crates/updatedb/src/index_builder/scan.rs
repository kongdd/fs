//! Bounded read-ahead for fresh builds. Only the caller touches SQLite.
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::ErrorKind,
    mem,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
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

pub(super) fn stamp_io(path: &Path, root: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = directory_metadata(path, root)?;
    if !metadata.is_dir() {
        return Err(ErrorKind::NotADirectory.into());
    }
    Ok(stamp(&metadata))
}

pub(super) fn warn_unreadable(path: &Path, error: &std::io::Error) {
    crate::ui::log(
        crate::ui::Tone::Warning,
        format_args!("skipped unreadable {}: {error}", path.display()),
    );
}

pub(super) fn read_stamp(path: &Path, root: &Path) -> Result<Vec<u8>> {
    stamp_io(path, root)
        .with_context(|| format!("cannot stat {} (index unchanged)", path.display()))
}

fn note_skipped(skipped: &AtomicUsize, path: &Path, error: &std::io::Error) {
    skipped.fetch_add(1, Ordering::Relaxed);
    warn_unreadable(path, error);
}

fn unreadable(
    path: &Path,
    start: &Path,
    error: std::io::Error,
    skipped: &AtomicUsize,
) -> Result<()> {
    // A missing scan root is a user/path error. Nested or permission failures
    // must not discard the rest of the index.
    if path == start && error.kind() == ErrorKind::NotFound {
        return Err(error)
            .with_context(|| format!("cannot read {} (index unchanged)", path.display()));
    }
    note_skipped(skipped, path, &error);
    Ok(())
}

pub(super) struct Scanner {
    receiver: Option<Receiver<Result<Chunk>>>,
    worker: Option<JoinHandle<()>>,
    profile: Option<Profile>,
    skipped: Arc<AtomicUsize>,
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
    #[cfg(test)]
    pub fn new(root: &Path, start: &Path, rules: ScanRules) -> Result<Self> {
        Self::with_jobs(root, start, rules, 1)
    }

    pub fn with_jobs(root: &Path, start: &Path, rules: ScanRules, jobs: u32) -> Result<Self> {
        let root = root.to_path_buf();
        let start = start.to_path_buf();
        let jobs = jobs.clamp(1, 256);
        let profile = jobs == 1
            && ["FS_SCAN_PROFILE", "NASFIND_SCAN_PROFILE"]
                .iter()
                .any(|name| std::env::var_os(name).is_some());
        let skipped = Arc::new(AtomicUsize::new(0));
        let skipped_worker = Arc::clone(&skipped);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_SIZE);
        let worker = thread::Builder::new()
            .name("fs-scan".into())
            .spawn(move || {
                let result = if jobs == 1 {
                    scan_tree(&root, start, &rules, &sender, profile, &skipped_worker)
                } else {
                    scan_tree_parallel(&root, start, &rules, &sender, jobs as usize, skipped_worker)
                };
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            })?;
        Ok(Self {
            receiver: Some(receiver),
            worker: Some(worker),
            profile: profile.then(Profile::default),
            skipped,
        })
    }

    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed) as u64
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
    skipped: &AtomicUsize,
) -> Result<()> {
    let mut profile = profiling.then(Profile::default);
    let scan_root = start.clone();
    let mut pending = vec![start];
    while let Some(path) = pending.pop() {
        let started = profile.as_ref().map(|_| Instant::now());
        let before = match read_stamp(&path, root) {
            Ok(stamp) => stamp,
            Err(error) => {
                if path == scan_root && is_not_found(&error) {
                    return Err(error);
                }
                if let Some(io) = io_error(&error) {
                    note_skipped(skipped, &path, io);
                } else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    crate::ui::log(
                        crate::ui::Tone::Warning,
                        format_args!("skipped unreadable {}: {error:#}", path.display()),
                    );
                }
                continue;
            }
        };
        if let (Some(profile), Some(started)) = (&mut profile, started) {
            profile.metadata += started.elapsed();
            profile.directories += 1;
        }
        let mut header = Some((path.clone(), before));
        let mut entries = Vec::new();
        let mut children = Vec::new();
        let mut bytes = 0;
        let mut entry_count = 0;
        let start = profile.as_ref().map(|_| Instant::now());
        let sent_before = profile.as_ref().map(|p| p.send).unwrap_or_default();
        let iter = match fs::read_dir(&path) {
            Ok(iter) => iter,
            Err(error) => {
                unreadable(&path, &scan_root, error, skipped)?;
                continue;
            }
        };
        for entry in iter {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn_unreadable(&path, &error);
                    continue;
                }
            };
            let is_dir = match entry.file_type() {
                Ok(kind) => kind.is_dir(),
                Err(error) => {
                    warn_unreadable(&path, &error);
                    continue;
                }
            };
            let name = entry.file_name();
            if is_dir && !rules.directories.contains(&name) {
                let child = path.join(&name);
                if !rules
                    .paths
                    .iter()
                    .any(|blocked| crate::platform::path_starts_with(&child, blocked))
                {
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

fn is_not_found(error: &anyhow::Error) -> bool {
    io_error(error).is_some_and(|io| io.kind() == ErrorKind::NotFound)
}

fn io_error(error: &anyhow::Error) -> Option<&std::io::Error> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
}

struct Loaded {
    before: Vec<u8>,
    entries: Vec<Entry>,
    children: Vec<PathBuf>,
}

fn load_directory(
    path: &Path,
    root: &Path,
    start: &Path,
    rules: &ScanRules,
    skipped: &AtomicUsize,
) -> Result<Option<Loaded>> {
    let before = match read_stamp(path, root) {
        Ok(stamp) => stamp,
        Err(error) => {
            if path == start && is_not_found(&error) {
                return Err(error);
            }
            if let Some(io) = io_error(&error) {
                note_skipped(skipped, path, io);
            } else {
                skipped.fetch_add(1, Ordering::Relaxed);
                crate::ui::log(
                    crate::ui::Tone::Warning,
                    format_args!("skipped unreadable {}: {error:#}", path.display()),
                );
            }
            return Ok(None);
        }
    };
    let iter = match fs::read_dir(path) {
        Ok(iter) => iter,
        Err(error) => {
            unreadable(path, start, error, skipped)?;
            return Ok(None);
        }
    };
    let mut entries = Vec::new();
    let mut children = Vec::new();
    for entry in iter {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn_unreadable(path, &error);
                continue;
            }
        };
        let is_dir = match entry.file_type() {
            Ok(kind) => kind.is_dir(),
            Err(error) => {
                warn_unreadable(path, &error);
                continue;
            }
        };
        let name = entry.file_name();
        if is_dir && !rules.directories.contains(&name) {
            let child = path.join(&name);
            if !rules
                .paths
                .iter()
                .any(|blocked| crate::platform::path_starts_with(&child, blocked))
            {
                children.push(child);
            }
        }
        entries.push((name, is_dir));
    }
    children.sort_unstable();
    Ok(Some(Loaded {
        before,
        entries,
        children,
    }))
}

fn emit_directory(
    sender: &SyncSender<Result<Chunk>>,
    path: PathBuf,
    before: Vec<u8>,
    mut entries: Vec<Entry>,
) -> Result<()> {
    // One message per directory so parallel workers cannot interleave chunks.
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut header = Some((path, before));
    send_chunk(sender, &mut header, &mut entries, false, &mut None)
}

fn scan_tree_parallel(
    root: &Path,
    start: PathBuf,
    rules: &ScanRules,
    sender: &SyncSender<Result<Chunk>>,
    jobs: usize,
    skipped: Arc<AtomicUsize>,
) -> Result<()> {
    struct Queue {
        paths: Vec<PathBuf>,
        inflight: usize,
        stop: bool,
    }
    let queue = Arc::new((
        Mutex::new(Queue {
            paths: vec![start.clone()],
            inflight: 0,
            stop: false,
        }),
        Condvar::new(),
    ));
    let fatal = Arc::new(Mutex::new(None));
    let mut handles = Vec::with_capacity(jobs);
    for index in 0..jobs {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let rules = rules.clone();
        let root = root.to_path_buf();
        let start = start.clone();
        let fatal = Arc::clone(&fatal);
        let skipped = Arc::clone(&skipped);
        handles.push(
            thread::Builder::new()
                .name(format!("fs-scan-{index}"))
                .spawn(move || -> Result<()> {
                    let (lock, cv) = &*queue;
                    loop {
                        let path = {
                            let mut queue = lock.lock().unwrap();
                            loop {
                                if queue.stop {
                                    return Ok(());
                                }
                                if let Some(path) = queue.paths.pop() {
                                    queue.inflight += 1;
                                    break path;
                                }
                                if queue.inflight == 0 {
                                    cv.notify_all();
                                    return Ok(());
                                }
                                queue = cv.wait(queue).unwrap();
                            }
                        };
                        let loaded = load_directory(&path, &root, &start, &rules, &skipped);
                        let mut queue = lock.lock().unwrap();
                        match loaded {
                            Ok(Some(_loaded)) if queue.stop => {
                                queue.inflight -= 1;
                                cv.notify_all();
                                return Ok(());
                            }
                            Ok(Some(loaded)) => {
                                let children = loaded.children;
                                let before = loaded.before;
                                let entries = loaded.entries;
                                queue.paths.extend(children);
                                queue.inflight -= 1;
                                cv.notify_all();
                                drop(queue);
                                emit_directory(&sender, path, before, entries)?;
                            }
                            Ok(None) => {
                                queue.inflight -= 1;
                                cv.notify_all();
                            }
                            Err(error) => {
                                queue.stop = true;
                                *fatal.lock().unwrap() = Some(error);
                                queue.inflight -= 1;
                                cv.notify_all();
                                return Ok(());
                            }
                        }
                    }
                })?,
        );
    }
    let mut error = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                error.get_or_insert(err);
            }
            Err(_) => {
                error.get_or_insert_with(|| anyhow::anyhow!("directory scanner panicked"));
            }
        }
    }
    if let Some(error) = fatal.lock().unwrap().take().or(error) {
        return Err(error);
    }
    Ok(())
}

/// Parallel directory stats for incremental updates. Filesystem only.
pub(super) struct StatPool {
    job_tx: Option<mpsc::Sender<PathBuf>>,
    res_rx: Option<mpsc::Receiver<(PathBuf, std::io::Result<Vec<u8>>)>>,
    ready: HashMap<PathBuf, std::io::Result<Vec<u8>>>,
    workers: Vec<JoinHandle<()>>,
}

impl StatPool {
    pub fn new(root: &Path, jobs: usize) -> Self {
        let jobs = jobs.clamp(1, 256);
        let (job_tx, job_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, res_rx) = mpsc::channel::<(PathBuf, std::io::Result<Vec<u8>>)>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let mut workers = Vec::with_capacity(jobs);
        for index in 0..jobs {
            let job_rx = Arc::clone(&job_rx);
            let res_tx = res_tx.clone();
            let root = root.to_path_buf();
            workers.push(
                thread::Builder::new()
                    .name(format!("fs-stat-{index}"))
                    .spawn(move || {
                        loop {
                            let path = {
                                let rx = job_rx.lock().unwrap();
                                rx.recv()
                            };
                            let Ok(path) = path else { break };
                            let result = stamp_io(&path, &root);
                            if res_tx.send((path, result)).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("spawn stat worker"),
            );
        }
        drop(res_tx);
        Self {
            job_tx: Some(job_tx),
            res_rx: Some(res_rx),
            ready: HashMap::new(),
            workers,
        }
    }

    pub fn submit(&self, path: PathBuf) {
        if let Some(tx) = &self.job_tx {
            let _ = tx.send(path);
        }
    }

    pub fn take(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
        loop {
            if let Some(result) = self.ready.remove(path) {
                return result;
            }
            let (done, result) = self
                .res_rx
                .as_ref()
                .ok_or_else(|| std::io::Error::other("stat pool closed"))?
                .recv()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            self.ready.insert(done, result);
        }
    }
}

impl Drop for StatPool {
    fn drop(&mut self) {
        self.job_tx.take();
        self.res_rx.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/updatedb/scan.rs"]
mod tests;
