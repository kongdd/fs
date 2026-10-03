//! Experimental statistics adapter; production stats.rs remains unchanged.
use std::{
    collections::{HashMap, HashSet},
    env,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::Config,
    search::{self, SearchOptions},
};

#[path = "../../src/stats_cache.rs"]
mod cache;

#[derive(clap::Args, Debug)]
pub struct StatsOptions {
    /// Restrict ranking to this subtree. No filesystem traversal is performed.
    pub path: Option<PathBuf>,
    /// Number of results.
    #[arg(short = 'n', long = "top", default_value = "10")]
    pub top: NonZeroUsize,
    /// Select a configured index; may be repeated.
    #[arg(short = 'd', long = "index")]
    pub indexes: Vec<String>,
    /// Include descendants; false counts direct children only.
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub recursive: bool,
}

pub fn stats(cfg: &Config, options: &StatsOptions) -> Result<()> {
    for idx in cfg.select(&options.indexes)? {
        crate::native::is_native(&idx.database)?;
    }
    let root = options.path.as_deref().map(absolute_path).transpose()?;
    let mut connection = cache::open(cfg)?;
    let cached = cache::ensure(&mut connection, cfg, &options.indexes)?;
    // Keep the totals and rankings in one SQLite read snapshot during refreshes.
    let transaction = connection.transaction()?;
    let cached = cache::metadata(&transaction, cached.id)?;
    if cache::fingerprint(cfg, &options.indexes)?.1 != cached.snapshot {
        bail!("index databases changed while querying statistics; retry the command");
    }
    let total = cache::total(&transaction, &cached, root.as_deref())?;
    let rows = if total == 0 {
        Vec::new()
    } else {
        cache::ranked(&transaction, &cached, options, root.as_deref())?
    };
    let mut output = BufWriter::new(io::stdout().lock());
    writeln!(output, "{:>12}  DIRECTORY", "ENTRIES")?;
    for row in rows {
        write!(output, "{:>12}  ", separated(row.count))?;
        output.write_all(&row.path)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    let mode = if options.recursive {
        "Recursive counts overlap between parent and child folders."
    } else {
        "Direct-child counts only."
    };
    crate::ui::log(
        crate::ui::Tone::Info,
        format_args!(
            "Total: {} unique indexed paths (including directories; config filters applied). {mode}",
            separated(total)
        ),
    );
    Ok(())
}

// Called only after indexing/merging has completed. Refresh changed single-index
// caches and the full union; arbitrary index combinations are cached on demand.
pub fn refresh(cfg: &Config, names: &[String]) -> Result<()> {
    let mut connection = cache::open(cfg)?;
    for idx in cfg.select(names)? {
        if idx.database.is_file() {
            cache::ensure(&mut connection, cfg, std::slice::from_ref(&idx.name))?;
        }
    }
    if cfg.index.iter().all(|idx| idx.database.is_file()) {
        cache::ensure(&mut connection, cfg, &[])?;
    }
    Ok(())
}

struct Directory {
    parent: Option<usize>,
    direct: i64,
    recursive: i64,
}

struct Counts {
    directories: HashMap<Vec<u8>, usize>,
    nodes: Vec<Directory>,
    seen: Option<HashSet<Vec<u8>>>,
    total: i64,
}

impl Counts {
    fn new(deduplicate: bool) -> Self {
        Self {
            directories: HashMap::new(),
            nodes: Vec::new(),
            seen: deduplicate.then(HashSet::new),
            total: 0,
        }
    }

    fn add(&mut self, path: &[u8]) {
        if path.is_empty()
            || self
                .seen
                .as_mut()
                .is_some_and(|seen| !seen.insert(path.to_vec()))
        {
            return;
        }
        self.total += 1;
        let Some(parent) = parent_path(path) else {
            return;
        };
        if parent == path {
            return;
        }
        let id = self.directory(parent);
        self.nodes[id].direct += 1;
        if !is_root(parent) || cfg!(windows) {
            self.nodes[id].recursive += 1;
        }
    }

    fn add_children(&mut self, path: &[u8], n: i64) {
        if n == 0 {
            return;
        }
        // Match parent_path() for filenames joined to a trailing-slash root.
        let path = if is_root(path) {
            path
        } else {
            &path[..=path.iter().rposition(|&byte| byte != b'/').unwrap()]
        };
        self.total += n;
        let id = self.directory(path);
        self.nodes[id].direct += n;
        if !is_root(path) || cfg!(windows) {
            self.nodes[id].recursive += n;
        }
    }

    fn directory(&mut self, path: &[u8]) -> usize {
        if let Some(&id) = self.directories.get(path) {
            return id;
        }
        // Parents are interned first, so aggregation needs no depth sorting.
        let parent = parent_path(path)
            .filter(|parent| *parent != path && (!is_root(parent) || cfg!(windows)))
            .map(|parent| self.directory(parent));
        let id = self.nodes.len();
        self.nodes.push(Directory {
            parent,
            direct: 0,
            recursive: 0,
        });
        self.directories.insert(path.to_vec(), id);
        id
    }

    fn aggregate(&mut self) {
        self.seen = None;
        for id in (0..self.nodes.len()).rev() {
            if let Some(parent) = self.nodes[id].parent {
                self.nodes[parent].recursive += self.nodes[id].recursive;
            }
        }
    }
}

fn collect(cfg: &Config, indexes: &[String]) -> Result<Counts> {
    let selected = cfg.select(indexes)?;
    // Counts are authoritative only without query-time file filters or union
    // deduplication. Other cases retain the existing per-path implementation.
    if let [idx] = selected.as_slice()
        && idx.root.is_absolute()
        && idx.filters.exclude_extensions.is_empty()
        && idx.filters.exclude_files.is_empty()
        && crate::native::is_native(&idx.database)?
    {
        let mut counts = Counts::new(false);
        let root = crate::native::directory_counts(idx, |path, n| {
            counts.add_children(path, n);
            Ok(())
        })?;
        counts.add(&root);
        counts.aggregate();
        return Ok(counts);
    }
    // A single index already has unique paths; do not retain all records.
    let mut counts = Counts::new(selected.len() > 1);
    let query = SearchOptions {
        indexes: indexes.to_vec(),
        patterns: vec!["/".into()],
        ignore_case: false,
        basename: false,
        existing: false,
        limit: None,
        json: false,
        null: true,
        ..Default::default()
    };
    search::visit_paths(cfg, &query, |path| {
        counts.add(path);
        Ok(())
    })?;
    counts.aggregate();
    Ok(counts)
}

#[derive(Debug, Eq, PartialEq)]
struct Rank {
    count: i64,
    path: Vec<u8>,
}

fn is_root(path: &[u8]) -> bool {
    #[cfg(windows)]
    if let Ok(path) = crate::platform::path_from_bytes(path)
        && path.is_absolute()
        && path.parent().is_none()
    {
        return true;
    }
    !path.is_empty() && path.iter().all(|byte| *byte == b'/')
}

fn parent_path(path: &[u8]) -> Option<&[u8]> {
    #[cfg(windows)]
    if is_root(path) {
        return Some(path);
    }
    let head = &path[..=path.iter().rposition(|byte| *byte == b'/')?];
    if is_root(head) {
        Some(head)
    } else {
        Some(&head[..=head.iter().rposition(|byte| *byte != b'/')?])
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = if let Ok(relative) = path.strip_prefix("~") {
        crate::platform::home_dir()
            .context("home directory is not set")?
            .join(relative)
    } else {
        path.to_path_buf()
    };
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

fn separated(count: i64) -> String {
    let digits = count.to_string();
    let mut result = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

#[cfg(all(test, unix))]
#[path = "../../tests/unit/stats.rs"]
mod shared_tests;

#[cfg(all(test, unix))]
#[path = "../tests/unit/stats.rs"]
mod tests;

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn drive_and_unc_roots_have_finite_parent_chains() {
        for root in [
            b"C:/".as_slice(),
            b"//server/share/",
            b"//?/C:/",
            b"//?/UNC/server/share/",
        ] {
            assert!(is_root(root));
            assert_eq!(parent_path(root), Some(root));
            let mut counts = Counts::new(false);
            counts.add(&[root, b"file"].concat());
            counts.add(&[root, b"nested/file"].concat());
            counts.aggregate();
            assert_eq!(counts.nodes[counts.directories[root]].recursive, 2);
        }
    }
}
