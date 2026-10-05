use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, BufWriter, IsTerminal, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{Config, IndexConfig},
    ui::{self, Tone},
};

pub fn build_indexes(
    cfg: &Config,
    names: &[String],
    folders: &[PathBuf],
    progress: bool,
) -> Result<()> {
    if folders.is_empty() {
        for idx in cfg.select(names)? {
            build_one(cfg, idx, progress)?;
        }
        return Ok(());
    }
    let mut selected = Vec::new();
    for folder in folders {
        let folder = fs::canonicalize(folder)
            .with_context(|| format!("cannot resolve folder {}", folder.display()))?;
        if !folder.is_dir() {
            bail!("not a directory: {}", folder.display());
        }
        let (owner, root) = cfg
            .index
            .iter()
            .filter_map(|idx| {
                let root = fs::canonicalize(&idx.root).ok()?;
                folder.starts_with(&root).then_some((idx, root))
            })
            .max_by_key(|(_, root)| root.components().count())
            .with_context(|| {
                format!(
                    "folder {} is outside all available index roots",
                    folder.display()
                )
            })?;
        if folder != root {
            let relative = folder.strip_prefix(&root)?;
            if relative.components().any(|part| {
                owner
                    .filters
                    .exclude_dirs
                    .iter()
                    .any(|name| part.as_os_str() == name.as_str())
            }) {
                bail!(
                    "folder is excluded by directory rules: {}",
                    folder.display()
                );
            }
            for excluded in &owner.filters.exclude_paths {
                let excluded = if excluded.is_absolute() {
                    excluded.clone()
                } else {
                    root.join(excluded)
                };
                let excluded = fs::canonicalize(&excluded).unwrap_or(excluded);
                if folder.starts_with(excluded) {
                    bail!("folder is excluded by path rules: {}", folder.display());
                }
            }
            if !owner.database.is_file() {
                bail!("build the main DB first: fs updatedb {}", owner.name);
            }
        }
        if !selected.iter().any(|(_, path, _)| path == &folder) {
            selected.push((owner, folder, root));
        }
    }
    for (idx, folder, root) in &selected {
        // An ancestor selection already covers all selected descendants in the same DB.
        if selected.iter().any(|(other, ancestor, _)| {
            other.name == idx.name && ancestor != folder && folder.starts_with(ancestor)
        }) {
            continue;
        }
        if folder == root {
            build_one(cfg, idx, progress)?;
        } else {
            merge_folder(cfg, idx, folder, root, progress)?;
        }
    }
    Ok(())
}

// The OS releases this advisory lock even if the process crashes.
fn lock_database(database: &Path) -> Result<fs::File> {
    if let Some(parent) = database.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut path = database.as_os_str().to_os_string();
    path.push(".lock");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    lock.try_lock()
        .with_context(|| format!("database is already being updated: {}", database.display()))?;
    Ok(lock)
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(database: &Path) -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = database
            .parent()
            .context("database has no parent")?
            .join(format!(".fs-{}-{nonce}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn merge_folder(
    cfg: &Config,
    idx: &IndexConfig,
    folder: &Path,
    root: &Path,
    progress: bool,
) -> Result<()> {
    let _lock = lock_database(&idx.database)?;
    crate::database::reject_retired_index(&idx.database)?;
    let workspace = Workspace::new(&idx.database)?;
    let mut subtree = idx.clone();
    subtree.root = folder.to_path_buf();
    subtree.database = workspace.0.join("subtree.db");
    // Relative exclusions still refer to the original index root, not the subtree.
    for path in &mut subtree.filters.exclude_paths {
        if path.is_relative() {
            *path = root.join(&*path);
        }
    }
    build_one(cfg, &subtree, progress)?;
    let paths = workspace.0.join("paths.txt");
    let mut output = BufWriter::new(fs::File::create(&paths)?);
    // Replace the whole subtree's old list, so deletions and renames are handled too.
    export_paths(cfg, &idx.database, Some(folder), &mut output)?;
    export_paths(cfg, &subtree.database, None, &mut output)?;
    output.flush()?;
    drop(output);
    let sorted = workspace.0.join("sorted.txt");
    let status = Command::new(&cfg.tools.sort)
        .env("LC_ALL", "C")
        .arg("-u")
        .arg("-T")
        .arg(&workspace.0)
        .arg("--")
        .arg(&paths)
        .stdout(fs::File::create(&sorted)?)
        .status()
        .context("failed to run sort")?;
    if !status.success() {
        bail!("sort failed: {status}");
    }
    let merged = workspace.0.join("merged.db");
    let status = Command::new(&cfg.tools.plocate_build)
        .args(["-p", "-l", "0"])
        .arg(&sorted)
        .arg(&merged)
        .status()
        .context("failed to run plocate-build")?;
    if !status.success() {
        bail!("plocate-build failed: {status}");
    }
    fs::set_permissions(&merged, fs::metadata(&idx.database)?.permissions())?;
    fs::File::open(&merged)?.sync_all()?;
    fs::rename(&merged, &idx.database).context("failed to replace main database")?;
    ui::log(
        Tone::Success,
        format_args!(
            "merged {} into {}",
            folder.display(),
            idx.database.display()
        ),
    );
    Ok(())
}

fn database_arg(database: &Path) -> OsString {
    // plocate's -d list syntax requires escaping ':' and backslash.
    let mut escaped = Vec::new();
    for &byte in database.as_os_str().as_bytes() {
        if matches!(byte, b':' | b'\\') {
            escaped.push(b'\\');
        }
        escaped.push(byte);
    }
    OsString::from_vec(escaped)
}

fn estimated_entries(cfg: &Config, database: &Path) -> Option<u64> {
    let output = Command::new(&cfg.tools.plocate)
        .env_remove("LOCATE_PATH")
        .arg("-c")
        .arg("-d")
        .arg(database_arg(database))
        .args(["--", "*"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|count| *count > 0)
}

fn export_paths(
    cfg: &Config,
    database: &Path,
    removed: Option<&Path>,
    output: &mut impl Write,
) -> Result<()> {
    let mut child = Command::new(&cfg.tools.plocate)
        .env_remove("LOCATE_PATH")
        .arg("-d")
        .arg(database_arg(database))
        .args(["-0", "--", "*"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stderr = child.stderr.take().context("missing plocate stderr")?;
    let diagnostics =
        thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::stderr().lock()));
    let mut reader = BufReader::new(child.stdout.take().context("missing plocate stdout")?);
    let result: Result<()> = (|| {
        let mut path = Vec::new();
        loop {
            path.clear();
            if reader.read_until(0, &mut path)? == 0 {
                break;
            }
            path.pop();
            let path_obj = Path::new(std::ffi::OsStr::from_bytes(&path));
            if removed.is_some_and(|root| path_obj.starts_with(root)) {
                continue;
            }
            if path.contains(&b'\n') {
                bail!(
                    "partial merge cannot import filenames containing newlines; use a full index update"
                );
            }
            output.write_all(&path)?;
            output.write_all(b"\n")?;
        }
        Ok(())
    })();
    if result.is_err() {
        child.kill().ok();
    }
    drop(reader);
    let status = child.wait()?;
    let diagnostic_bytes = diagnostics
        .join()
        .map_err(|_| anyhow::anyhow!("diagnostic reader panicked"))??;
    result?;
    if !(status.success() || (status.code() == Some(1) && diagnostic_bytes == 0)) {
        bail!("cannot export database {}: {status}", database.display());
    }
    Ok(())
}

// PRUNENAMES is whitespace-separated and cannot represent these names.
// Discover their exact paths without descending into excluded directories.
fn spaced_directory_paths(idx: &IndexConfig) -> Result<Vec<PathBuf>> {
    if !idx
        .filters
        .exclude_dirs
        .iter()
        .any(|name| name.chars().any(char::is_whitespace))
    {
        return Ok(Vec::new());
    }
    let excluded_paths: Vec<_> = idx
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
        .collect();
    let mut pending = vec![idx.root.clone()];
    let mut found = Vec::new();
    while let Some(directory) = pending.pop() {
        if excluded_paths
            .iter()
            .any(|path| directory.starts_with(path))
        {
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot scan {}", directory.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            // Do not follow symlinks, just like updatedb.
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let basename = entry.file_name();
            if let Some(name) = idx
                .filters
                .exclude_dirs
                .iter()
                .find(|name| basename == name.as_str())
            {
                if name.chars().any(char::is_whitespace) {
                    found.push(entry.path());
                }
            } else {
                pending.push(entry.path());
            }
        }
    }
    Ok(found)
}

fn build_one(cfg: &Config, idx: &IndexConfig, progress: bool) -> Result<()> {
    if !idx.root.is_dir() {
        bail!(
            "index root does not exist or is not a directory: {}",
            idx.root.display()
        );
    }
    let _lock = lock_database(&idx.database)?;
    let start = Instant::now();
    crate::database::reject_retired_index(&idx.database)?;
    let existing = idx.database.is_file();
    let expected = if progress && existing {
        estimated_entries(cfg, &idx.database)
    } else {
        None
    };

    let mut cmd = Command::new(&cfg.tools.updatedb);
    // Override system pruning defaults: they commonly exclude NAS filesystems.
    // plocate 1.1.19 lacks the newer --config-file option.
    cmd.arg("--prune-bind-mounts")
        .arg("no")
        .arg("--prunefs")
        .arg("")
        .arg("--prunepaths")
        .arg("")
        .arg("--prunenames")
        .arg(
            idx.filters
                .exclude_dirs
                .iter()
                .filter(|name| !name.chars().any(char::is_whitespace))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        )
        .arg("-l")
        .arg("0")
        .arg("-U")
        .arg(&idx.root)
        .arg("-o")
        .arg(&idx.database);

    for path in &idx.filters.exclude_paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            idx.root.join(path)
        };
        cmd.arg("--add-single-prunepath").arg(absolute);
    }

    if progress
        && idx
            .filters
            .exclude_dirs
            .iter()
            .any(|name| name.chars().any(char::is_whitespace))
    {
        ui::log(
            Tone::Warning,
            format_args!(
                "{}: preparing whitespace directory exclusions (ETA --)",
                idx.name
            ),
        );
    }
    for path in spaced_directory_paths(idx)? {
        cmd.arg("--add-single-prunepath").arg(path);
    }

    if progress {
        cmd.arg("-v").stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null());
    }
    cmd.stderr(Stdio::inherit());

    let estimate = match (progress, expected) {
        (true, Some(total)) => format!("; estimated total ~{total}"),
        (true, None) => "; total unknown".into(),
        (false, _) => String::new(),
    };
    ui::log(
        Tone::Info,
        format_args!(
            "indexing {}: {} -> {} [{}]{}",
            idx.name,
            idx.root.display(),
            idx.database.display(),
            if existing { "update" } else { "init" },
            estimate
        ),
    );

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {}", cfg.tools.updatedb))?;

    let (status, count) = if progress {
        wait_with_progress(&mut child, &idx.name, expected, start)?
    } else {
        (child.wait().context("failed waiting for updatedb")?, 0)
    };
    if !status.success() {
        bail!(
            "updatedb failed for index {} with status {}",
            idx.name,
            status
        );
    }

    let elapsed = start.elapsed();
    if progress {
        let rate = if elapsed.as_secs_f64() > 0.0 {
            count as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };
        ui::log(
            Tone::Success,
            format_args!(
                "done {}: {} entries in {:.1}s ({:.0}/s)",
                idx.name,
                count,
                elapsed.as_secs_f64(),
                rate
            ),
        );
    } else {
        ui::log(
            Tone::Success,
            format_args!("done {} in {:.1}s", idx.name, elapsed.as_secs_f64()),
        );
    }
    Ok(())
}

fn wait_with_progress(
    child: &mut Child,
    name: &str,
    expected: Option<u64>,
    start: Instant,
) -> Result<(ExitStatus, u64)> {
    let stdout = child
        .stdout
        .take()
        .context("updatedb stdout was not captured")?;
    let processed = Arc::new(AtomicU64::new(0));
    let reader_count = Arc::clone(&processed);
    let reader = thread::spawn(move || -> std::io::Result<u64> {
        let mut input = BufReader::new(stdout);
        let mut line = Vec::new();
        let mut count = 0;
        loop {
            line.clear();
            if input.read_until(b'\n', &mut line)? == 0 {
                return Ok(count);
            }
            count += 1;
            reader_count.store(count, Ordering::Relaxed);
        }
    });
    let terminal = std::io::stderr().is_terminal();
    let interval = Duration::from_millis(250);
    let result = (|| -> Result<ExitStatus> {
        let mut last_report = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            // Only interactive terminals get live updates. Emitting periodic
            // snapshots to a pipe/log creates an ever-growing wall of lines.
            if terminal && last_report.elapsed() >= interval {
                let status = progress_status(
                    processed.load(Ordering::Relaxed),
                    start.elapsed(),
                    expected,
                    reader.is_finished(),
                );
                let tone = if reader.is_finished()
                    || expected.is_some_and(|total| processed.load(Ordering::Relaxed) >= total)
                {
                    Tone::Warning
                } else {
                    Tone::Info
                };
                ui::write_progress(tone, format_args!("{name}: {status}"))
                    .context("failed to write progress")?;
                last_report = Instant::now();
            }
            thread::sleep(Duration::from_millis(100));
        }
    })();
    if result.is_err() {
        child.kill().ok();
        child.wait().ok();
    }
    if terminal {
        write!(std::io::stderr(), "\r\x1b[2K").ok();
        std::io::stderr().flush().ok();
    }
    let count = reader
        .join()
        .map_err(|_| anyhow::anyhow!("progress reader panicked"))?
        .context("failed to read updatedb progress")?;
    Ok((result?, count))
}

fn progress_status(
    count: u64,
    elapsed: Duration,
    expected: Option<u64>,
    finalizing: bool,
) -> String {
    let rate = count as f64 / elapsed.as_secs_f64().max(0.001);
    let (bar, estimate) = match expected.filter(|total| *total > 0) {
        Some(total) => {
            // Historical totals are estimates, not a promise of exact completion.
            let fraction = (count as f64 / total as f64).min(0.99);
            let filled = (fraction * 8.0) as usize;
            let bar = format!(
                "[{}{}] ~{:.0}%",
                "=".repeat(filled),
                "-".repeat(8 - filled),
                fraction * 100.0
            );
            let estimate = if finalizing {
                "finishing; ETA unknown".into()
            } else if count >= total {
                "past estimate; ETA unknown".into()
            } else if count == 0 {
                "ETA estimating".into()
            } else {
                let remaining = ((total - count) as f64 / rate).ceil() as u64;
                format!("ETA ~{}m{:02}s", remaining / 60, remaining % 60)
            };
            (bar, estimate)
        }
        None => {
            let position = (elapsed.as_millis() / 250 % 4) as usize;
            let bar = ["|", "/", "-", "\\"][position].to_string();
            (
                bar,
                if finalizing {
                    "finishing; ETA unknown"
                } else {
                    "ETA --"
                }
                .into(),
            )
        }
    };
    format!(
        "{bar} {count} · {rate:.0}/s · {:.0}s · {estimate}",
        elapsed.as_secs_f64()
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/updatedb/indexer.rs"]
mod tests;
