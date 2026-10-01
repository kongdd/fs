use std::{
    ffi::OsStr,
    io::{self, BufRead, BufReader, BufWriter, Write},
    os::unix::ffi::OsStrExt,
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
    pub patterns: Vec<String>,
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    #[arg(short = 'b', long)]
    pub basename: bool,
    #[arg(short = 'e', long)]
    pub existing: bool,
    #[arg(short = 'l', long)]
    pub limit: Option<usize>,
    #[arg(long, conflicts_with = "null")]
    pub json: bool,
    #[arg(short = '0', long = "null")]
    pub null: bool,
}

#[derive(Serialize)]
struct JsonResult<'a> {
    path: &'a str,
}

pub fn search(cfg: &Config, options: &SearchOptions) -> Result<()> {
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
    let has_filters = indexes.iter().any(|idx| !idx.exclude_extensions.is_empty());

    let mut cmd = Command::new(&cfg.tools.plocate);
    for idx in &indexes {
        cmd.arg("-d").arg(&idx.database);
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
    cmd.args(&options.patterns)
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
    let mut output = BufWriter::new(io::stdout().lock());
    let mut buf = Vec::new();
    let mut written = 0_usize;
    let mut stopped_early = false;

    let result: Result<()> = (|| {
        if options.json {
            output.write_all(b"[")?;
        }
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
            if has_filters && is_excluded_by_extension(&buf, &indexes) {
                continue;
            }

            if options.json {
                if written > 0 {
                    output.write_all(b",")?;
                }
                // JSON requires Unicode; raw text and NUL output retain filename bytes.
                let path = String::from_utf8_lossy(&buf);
                serde_json::to_writer(&mut output, &JsonResult { path: &path })?;
            } else {
                output.write_all(&buf)?;
                output.write_all(if options.null { b"\0" } else { b"\n" })?;
            }
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
    if !(stopped_early || status.success() || empty_database) {
        bail!("plocate exited with status {status}");
    }
    if options.json {
        output.write_all(b"]\n")?;
    }
    output.flush()?;
    Ok(())
}

fn is_excluded_by_extension(path: &[u8], indexes: &[&IndexConfig]) -> bool {
    let path_obj = Path::new(OsStr::from_bytes(path));
    let Some(ext) = path_obj.extension() else {
        return false;
    };

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
        idx.exclude_extensions.iter().any(|blocked| {
            ext.as_bytes()
                .eq_ignore_ascii_case(blocked.trim_start_matches('.').as_bytes())
        })
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn idx(name: &str, root: &str, exts: &[&str]) -> IndexConfig {
        IndexConfig {
            name: name.into(),
            root: PathBuf::from(root),
            database: PathBuf::from(format!("/tmp/{name}.db")),
            exclude_dirs: vec![],
            exclude_paths: vec![],
            exclude_extensions: exts.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn extension_filter_is_case_insensitive() {
        let a = idx("a", "/data", &["tmp", ".pyc"]);
        assert!(is_excluded_by_extension(b"/data/x.TMP", &[&a]));
        assert!(is_excluded_by_extension(b"/data/x.pyc", &[&a]));
        assert!(!is_excluded_by_extension(b"/data/x.nc", &[&a]));
    }

    #[test]
    fn nested_root_uses_longest_match() {
        let outer = idx("outer", "/data", &["tmp"]);
        let inner = idx("inner", "/data/keep", &[]);
        assert!(!is_excluded_by_extension(
            b"/data/keep/file.tmp",
            &[&outer, &inner]
        ));
    }
}
