use std::{
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, BufWriter, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::Path,
    process::{Command, Stdio},
    thread,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::{Config, IndexConfig};

#[derive(clap::Args, Debug)]
pub struct SearchOptions {
    /// Restrict search to named indexes. May be repeated.
    #[arg(short = 'd', long = "index")]
    pub indexes: Vec<String>,
    /// One or more plocate patterns. Multiple patterns are ANDed.
    #[arg(required = true)]
    pub patterns: Vec<OsString>,
    /// Match without ASCII/locale case sensitivity.
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    /// Match filenames only, ignoring directory names.
    #[arg(short = 'b', long)]
    pub basename: bool,
    /// Check that matches still exist (accesses the filesystem/NAS).
    #[arg(short = 'e', long)]
    pub existing: bool,
    /// Stop after this many unfiltered matches.
    #[arg(short = 'l', long)]
    pub limit: Option<usize>,
    /// Write a JSON array of paths.
    #[arg(long, conflicts_with = "null")]
    pub json: bool,
    /// Separate raw paths with NUL bytes, for piping to other tools.
    #[arg(short = '0', long = "null")]
    pub null: bool,
}

#[derive(Serialize)]
struct JsonResult<'a> {
    path: &'a str,
}

pub fn search(cfg: &Config, options: &SearchOptions) -> Result<()> {
    let mut output = BufWriter::new(io::stdout().lock());
    let mut written = 0;
    visit_paths(cfg, options, |path| {
        if options.json {
            output.write_all(if written == 0 { b"[" } else { b"," })?;
            let path = String::from_utf8_lossy(path);
            serde_json::to_writer(&mut output, &JsonResult { path: &path })?;
        } else {
            output.write_all(path)?;
            output.write_all(if options.null { b"\0" } else { b"\n" })?;
        }
        written += 1;
        Ok(())
    })?;
    if options.json {
        if written == 0 {
            output.write_all(b"[")?;
        }
        output.write_all(b"]\n")?;
    }
    output.flush()?;
    Ok(())
}

// Share byte-safe streaming, filters and child cleanup with stats.
pub fn visit_paths(
    cfg: &Config,
    options: &SearchOptions,
    mut visit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    if options.patterns.is_empty() {
        bail!("at least one search pattern is required");
    }
    if options.limit == Some(0) {
        bail!("--limit must be greater than zero");
    }
    if options.json && options.null {
        bail!("--json and --null cannot be used together");
    }

    let indexes = cfg.select(&options.indexes)?;
    for idx in &indexes {
        if !idx.database.is_file() {
            bail!(
                "database for index {} does not exist: {}; run `nasfind index {}` first",
                idx.name,
                idx.database.display(),
                idx.name
            );
        }
    }
    let has_filters = indexes.iter().any(|idx| {
        !idx.filters.exclude_extensions.is_empty() || !idx.filters.exclude_files.is_empty()
    });

    let mut cmd = Command::new(&cfg.tools.plocate);
    cmd.env_remove("LOCATE_PATH");
    for idx in &indexes {
        // -d accepts a colon-separated list with backslash escaping.
        let mut database = Vec::new();
        for &byte in idx.database.as_os_str().as_bytes() {
            if matches!(byte, b':' | b'\\') {
                database.push(b'\\');
            }
            database.push(byte);
        }
        cmd.arg("-d").arg(OsString::from_vec(database));
    }
    if options.ignore_case {
        cmd.arg("-i");
    }
    if options.basename {
        cmd.arg("-b");
    }
    if options.existing {
        cmd.arg("-e");
    }
    // NUL output makes every filename unambiguous, including names containing newlines.
    cmd.arg("-0");

    // When no post-filter is required, let plocate enforce the limit itself.
    if !has_filters && let Some(limit) = options.limit {
        cmd.arg("-l").arg(limit.to_string());
    }
    cmd.arg("--")
        .args(&options.patterns)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {}", cfg.tools.plocate))?;
    let stdout = child
        .stdout
        .take()
        .context("plocate stdout was not captured")?;
    // Drain diagnostics concurrently so errors cannot fill a pipe and block queries.
    let mut stderr = child
        .stderr
        .take()
        .context("plocate stderr was not captured")?;
    let diagnostics = thread::spawn(move || io::copy(&mut stderr, &mut io::stderr().lock()));
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut written = 0_usize;
    let mut stopped_early = false;

    let result: Result<()> = (|| {
        loop {
            buf.clear();
            if reader
                .read_until(0, &mut buf)
                .context("failed reading plocate output")?
                == 0
            {
                break;
            }
            if buf.last() == Some(&0) {
                buf.pop();
            }
            if buf.is_empty() || (has_filters && is_excluded(&buf, &indexes)) {
                continue;
            }

            visit(&buf)?;
            written += 1;
            if has_filters && options.limit.is_some_and(|limit| written >= limit) {
                stopped_early = true;
                break;
            }
        }
        Ok(())
    })();

    // Reap the child even if output fails (for example, a downstream pipe closes).
    if stopped_early || result.is_err() {
        child.kill().ok();
    }
    drop(reader);
    let status = child.wait().context("failed waiting for plocate")?;
    let diagnostic_bytes = diagnostics
        .join()
        .map_err(|_| anyhow::anyhow!("diagnostic reader panicked"))??;
    result?;
    // A multi-DB query may exit 1 even with output if one DB has no matches.
    // Backend errors also use 1, but emit diagnostics.
    let empty_database = status.code() == Some(1) && diagnostic_bytes == 0;
    if !(status.success() || empty_database || (stopped_early && diagnostic_bytes == 0)) {
        bail!("plocate exited with status {status}");
    }
    Ok(())
}

fn is_excluded(path: &[u8], indexes: &[&IndexConfig]) -> bool {
    let path_obj = Path::new(OsStr::from_bytes(path));

    // A result should belong to exactly one configured root. Longest-prefix matching
    // handles nested roots deterministically.
    let mut owner: Option<&IndexConfig> = None;
    for idx in indexes {
        if path_obj.starts_with(&idx.root)
            && owner
                .is_none_or(|current| idx.root.as_os_str().len() > current.root.as_os_str().len())
        {
            owner = Some(idx);
        }
    }

    owner.is_some_and(|idx| {
        path_obj.file_name().is_some_and(|name| {
            idx.filters
                .exclude_files
                .iter()
                .any(|blocked| name.as_bytes().eq_ignore_ascii_case(blocked.as_bytes()))
        }) || path_obj.extension().is_some_and(|ext| {
            idx.filters.exclude_extensions.iter().any(|blocked| {
                ext.as_bytes()
                    .eq_ignore_ascii_case(blocked.trim_start_matches('.').as_bytes())
            })
        })
    })
}

#[cfg(test)]
#[path = "../tests/unit/search.rs"]
mod tests;
