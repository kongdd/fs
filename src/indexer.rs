use std::{
    fs,
    io::{BufRead, BufReader, IsTerminal, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::config::{Config, IndexConfig};

pub fn build_indexes(cfg: &Config, names: &[String], progress: bool) -> Result<()> {
    let selected = cfg.select(names)?;
    for idx in selected {
        build_one(cfg, idx, progress)?;
    }
    Ok(())
}

fn build_one(cfg: &Config, idx: &IndexConfig, progress: bool) -> Result<()> {
    if !idx.root.is_dir() {
        bail!(
            "index root does not exist or is not a directory: {}",
            idx.root.display()
        );
    }
    if let Some(parent) = idx.database.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create database directory {}", parent.display()))?;
    }

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
        .arg(idx.filters.exclude_dirs.join(" "))
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

    if progress {
        cmd.arg("-v").stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null());
    }
    cmd.stderr(Stdio::inherit());

    eprintln!(
        "indexing {}: {} -> {}",
        idx.name,
        idx.root.display(),
        idx.database.display()
    );

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {}", cfg.tools.updatedb))?;

    let mut count = 0_u64;
    if progress {
        let stdout = child
            .stdout
            .take()
            .context("updatedb stdout was not captured")?;
        let mut reader = BufReader::new(stdout);
        let mut current = Vec::new();
        let terminal = std::io::stderr().is_terminal();
        let mut last_report = Instant::now();

        loop {
            current.clear();
            if reader
                .read_until(b'\n', &mut current)
                .context("failed to read updatedb progress")?
                == 0
            {
                break;
            }
            count += 1;
            if (terminal && last_report.elapsed() >= Duration::from_millis(250))
                || (!terminal && count.is_multiple_of(100_000))
            {
                report_progress(
                    &idx.name,
                    count,
                    start.elapsed(),
                    String::from_utf8_lossy(&current).trim_end(),
                    terminal,
                )?;
                last_report = Instant::now();
            }
        }
        if terminal {
            eprint!("\r\x1b[2K");
            std::io::stderr().flush().ok();
        }
    }

    let status = child.wait().context("failed waiting for updatedb")?;
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
        eprintln!(
            "done {}: {} entries in {:.1}s ({:.0}/s)",
            idx.name,
            count,
            elapsed.as_secs_f64(),
            rate
        );
    } else {
        eprintln!("done {} in {:.1}s", idx.name, elapsed.as_secs_f64());
    }
    Ok(())
}

fn report_progress(
    name: &str,
    count: u64,
    elapsed: Duration,
    current: &str,
    terminal: bool,
) -> Result<()> {
    let rate = if elapsed.as_secs_f64() > 0.0 {
        count as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let short = truncate_left(current, 72);
    if terminal {
        eprint!(
            "\r\x1b[2K{name}: {count} entries | {rate:.0}/s | {:.1}s | {short}",
            elapsed.as_secs_f64()
        );
        std::io::stderr()
            .flush()
            .context("failed to flush progress")?;
    } else if count.is_multiple_of(100_000) {
        eprintln!(
            "{name}: {count} entries | {rate:.0}/s | {:.1}s | {short}",
            elapsed.as_secs_f64()
        );
    }
    Ok(())
}

fn truncate_left(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        return s.to_owned();
    }
    let tail: String = s.chars().skip(len - max_chars + 1).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_paths() {
        let s = "/a/very/long/path/to/something/file.txt";
        let out = truncate_left(s, 12);
        assert!(out.starts_with('…'));
        assert!(out.ends_with("file.txt"));
    }
}
