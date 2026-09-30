use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::{Config, IndexConfig};

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub indexes: Vec<String>,
    pub patterns: Vec<String>,
    pub ignore_case: bool,
    pub basename: bool,
    pub existing: bool,
    pub limit: Option<usize>,
    pub json: bool,
    pub null: bool,
}

#[derive(Serialize)]
struct JsonResult<'a> {
    path: &'a str,
}

pub fn search(cfg: &Config, options: &SearchOptions) -> Result<usize> {
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
        .stderr(Stdio::inherit());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {}", cfg.tools.plocate))?;
    let stdout = child
        .stdout
        .take()
        .context("plocate stdout was not captured")?;
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut accepted = Vec::<String>::new();
    let mut written = 0_usize;
    let mut stopped_early = false;

    loop {
        buf.clear();
        let n = reader
            .read_until(0, &mut buf)
            .context("failed reading plocate output")?;
        if n == 0 {
            break;
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        let path = String::from_utf8_lossy(&buf).into_owned();
        if is_excluded_by_extension(&path, &indexes) {
            continue;
        }

        if options.json {
            accepted.push(path);
        } else if options.null {
            std::io::stdout().write_all(path.as_bytes())?;
            std::io::stdout().write_all(&[0])?;
        } else {
            println!("{path}");
        }
        written += 1;

        if has_filters && options.limit.is_some_and(|limit| written >= limit) {
            stopped_early = true;
            child.kill().ok();
            break;
        }
    }

    if options.json {
        let rows: Vec<_> = accepted.iter().map(|path| JsonResult { path }).collect();
        serde_json::to_writer_pretty(std::io::stdout(), &rows)?;
        println!();
    }

    let status = child.wait().context("failed waiting for plocate")?;
    // plocate exits 1 both for errors and no matches. Empty successful output is normal for us.
    // If we intentionally killed the process after reaching a filtered limit, ignore the status.
    if !stopped_early && !status.success() && written > 0 {
        bail!("plocate exited with status {status}");
    }
    Ok(written)
}

fn is_excluded_by_extension(path: &str, indexes: &[&IndexConfig]) -> bool {
    let path_obj = Path::new(path);
    let Some(ext) = path_obj.extension().and_then(|s| s.to_str()) else {
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
        idx.exclude_extensions
            .iter()
            .any(|blocked| ext.eq_ignore_ascii_case(blocked.trim_start_matches('.')))
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
        assert!(is_excluded_by_extension("/data/x.TMP", &[&a]));
        assert!(is_excluded_by_extension("/data/x.pyc", &[&a]));
        assert!(!is_excluded_by_extension("/data/x.nc", &[&a]));
    }

    #[test]
    fn nested_root_uses_longest_match() {
        let outer = idx("outer", "/data", &["tmp"]);
        let inner = idx("inner", "/data/keep", &[]);
        assert!(!is_excluded_by_extension(
            "/data/keep/file.tmp",
            &[&outer, &inner]
        ));
    }
}
