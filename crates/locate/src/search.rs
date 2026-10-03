use std::{
    io::{self, BufWriter, Write},
    path::{Component, Path, PathBuf},
};

use crate::platform::path_from_bytes;
#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use serde::Serialize;
#[cfg(unix)]
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    thread,
};

use crate::config::{Config, IndexConfig};
pub use fs_core::query::SearchOptions;

#[derive(Serialize)]
struct JsonResult<'a> {
    path: &'a str,
}

pub fn search(cfg: &Config, options: &SearchOptions) -> Result<()> {
    let mut output = BufWriter::new(io::stdout().lock());
    let mut written = 0;
    if options.limit == Some(0) {
        bail!("--limit must be greater than zero");
    }
    if options.json && options.null {
        bail!("--json and --null cannot be used together");
    }
    let mut write_path = |path: &[u8]| {
        let mapped = options.mnt.then(|| map_mount_path(path)).flatten();
        let path = mapped.as_deref().unwrap_or(path);
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
    };
    if options.locate {
        visit_paths(cfg, options, &mut write_path)?;
    } else if options.regex {
        let effective = SearchOptions {
            ignore_case: !options.case_sensitive,
            basename: !options.match_path,
            ..options.clone()
        };
        visit_paths(cfg, &effective, &mut write_path)?;
    } else {
        crate::everything::visit(cfg, options, &mut write_path)?;
    }
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
    visit_paths_until(cfg, options, |path| {
        visit(path)?;
        Ok(true)
    })
}

pub(crate) fn visit_paths_until(
    cfg: &Config,
    options: &SearchOptions,
    mut visit: impl FnMut(&[u8]) -> Result<bool>,
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

    let scope = options.path.as_deref().map(resolve_scope).transpose()?;
    if options
        .extensions
        .iter()
        .any(|ext| ext.trim_start_matches('.').is_empty() || ext.contains(['/', '\\']))
    {
        bail!("--ext must contain nonempty extensions, not paths");
    }
    let indexes = cfg.select(&options.indexes)?;
    for idx in &indexes {
        if !idx.database.is_file() {
            bail!(
                "database for index {} does not exist: {}; run `fs updatedb {}` first",
                idx.name,
                idx.database.display(),
                idx.name
            );
        }
    }
    let native = indexes
        .iter()
        .map(|idx| crate::native::is_native(&idx.database))
        .collect::<Result<Vec<_>>>()?;
    if native.iter().any(|native| *native) {
        if native.iter().any(|native| !native) {
            bail!("cannot mix Rust and plocate indexes in one query; select indexes with -d");
        }
        let query = crate::native::Query::new(options)?;
        let mut skipped = 0;
        let mut written = 0;
        for idx in &indexes {
            let complete = crate::native::visit(idx, &query, |path| {
                if is_excluded(path, &indexes)?
                    || !matches_selection(path, scope.as_deref(), &options.extensions)?
                    || !matches_kind(path, options.dirs, options.files)?
                    || is_ignored_dir(path, &options.ignored_dirs)?
                {
                    return Ok(true);
                }
                if options.existing && !path_from_bytes(path)?.try_exists()? {
                    return Ok(true);
                }
                if skipped < options.offset {
                    skipped += 1;
                    return Ok(true);
                }
                let keep_going = visit(path)?;
                written += 1;
                Ok(keep_going && options.limit.is_none_or(|limit| written < limit))
            })?;
            if !complete {
                break;
            }
        }
        return Ok(());
    }
    #[cfg(unix)]
    return visit_plocate(cfg, options, &indexes, scope.as_deref(), visit);
    #[cfg(windows)]
    bail!("plocate databases are unsupported on Windows; select Rust indexes");
}

#[cfg(unix)]
fn visit_plocate(
    cfg: &Config,
    options: &SearchOptions,
    indexes: &[&IndexConfig],
    scope: Option<&Path>,
    mut visit: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<()> {
    let has_filters = indexes.iter().any(|idx| {
        !idx.filters.exclude_extensions.is_empty()
            || !idx.filters.exclude_files.is_empty()
            || !idx.filters.exclude_paths.is_empty()
    });
    let post_filter = has_filters
        || scope.is_some()
        || !options.extensions.is_empty()
        || options.dirs
        || options.files
        || !options.ignored_dirs.is_empty()
        || options.offset > 0;

    let mut cmd = Command::new(&cfg.tools.plocate);
    cmd.env_remove("LOCATE_PATH");
    if options.ignore_case && !options.locate {
        cmd.env("LC_ALL", "C");
    }
    for idx in indexes {
        // -d accepts a colon-separated list with backslash escaping.
        let mut database = Vec::new();
        for &byte in idx.database.as_os_str().as_encoded_bytes() {
            if matches!(byte, b':' | b'\\') {
                database.push(b'\\');
            }
            database.push(byte);
        }
        cmd.arg("-d").arg(crate::platform::os_string(database)?);
    }
    if options.ignore_case {
        cmd.arg("-i");
    }
    if options.basename {
        cmd.arg("-b");
    }
    if options.regex {
        cmd.arg("--regex");
    }
    if options.existing {
        cmd.arg("-e");
    }
    // NUL output makes every filename unambiguous, including names containing newlines.
    cmd.arg("-0");

    // When no post-filter is required, let plocate enforce the limit itself.
    if !post_filter && let Some(limit) = options.limit {
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
    let mut skipped = 0_usize;
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
            if buf.is_empty()
                || (has_filters && is_excluded(&buf, indexes)?)
                || !matches_selection(&buf, scope, &options.extensions)?
                || !matches_kind(&buf, options.dirs, options.files)?
                || is_ignored_dir(&buf, &options.ignored_dirs)?
            {
                continue;
            }
            if skipped < options.offset {
                skipped += 1;
                continue;
            }

            let keep_going = visit(&buf)?;
            written += 1;
            if !keep_going || (post_filter && options.limit.is_some_and(|limit| written >= limit)) {
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

// Lexical normalization only: queries must work even for deleted/offline paths.
fn resolve_scope(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn matches_kind(path: &[u8], dirs: bool, files: bool) -> Result<bool> {
    let path = path_from_bytes(path)?;
    let is_file = path.extension().is_some_and(|ext| !ext.is_empty());
    Ok((!dirs || !is_file) && (!files || is_file))
}

fn is_ignored_dir(path: &[u8], names: &[String]) -> Result<bool> {
    let path = path_from_bytes(path)?;
    Ok(path.components().any(|part| {
        matches!(part, Component::Normal(name) if names.iter().any(|blocked| name.as_encoded_bytes() == blocked.as_bytes()))
    }))
}

fn map_mount_path(path: &[u8]) -> Option<Vec<u8>> {
    for (source, target) in [
        (b"/volume1/CMIP6".as_slice(), b"/mnt/z".as_slice()),
        (b"/volume2/GitHub", b"/mnt/x"),
        (b"/volume1/Researches", b"/mnt/y"),
        (b"/volume1/CUG-hydro", b"/mnt/o"),
    ] {
        if let Some(rest) = path.strip_prefix(source)
            && (rest.is_empty() || rest.starts_with(b"/"))
        {
            return Some([target, rest].concat());
        }
    }
    None
}

fn matches_selection(path: &[u8], scope: Option<&Path>, extensions: &[String]) -> Result<bool> {
    let path = path_from_bytes(path)?;
    Ok(scope.is_none_or(|root| path.starts_with(root))
        && (extensions.is_empty()
            || path.extension().is_some_and(|ext| {
                extensions.iter().any(|wanted| {
                    ext.as_encoded_bytes()
                        .eq_ignore_ascii_case(wanted.trim_start_matches('.').as_bytes())
                })
            })))
}

fn is_excluded(path: &[u8], indexes: &[&IndexConfig]) -> Result<bool> {
    let path_obj = path_from_bytes(path)?;

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

    Ok(owner.is_some_and(|idx| {
        idx.filters.exclude_paths.iter().any(|blocked| {
            let blocked = if blocked.is_absolute() {
                blocked.clone()
            } else {
                idx.root.join(blocked)
            };
            path_obj.starts_with(blocked)
        }) || path_obj.file_name().is_some_and(|name| {
            idx.filters.exclude_files.iter().any(|blocked| {
                name.as_encoded_bytes()
                    .eq_ignore_ascii_case(blocked.as_bytes())
            })
        }) || path_obj.extension().is_some_and(|ext| {
            idx.filters.exclude_extensions.iter().any(|blocked| {
                ext.as_encoded_bytes()
                    .eq_ignore_ascii_case(blocked.trim_start_matches('.').as_bytes())
            })
        })
    }))
}

#[cfg(all(test, unix))]
#[path = "../../../tests/unit/locate/search.rs"]
mod tests;
