use std::{
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::{Config, IndexConfig},
    ui::{self, Tone},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
#[cfg(unix)]
#[path = "plocate.rs"]
mod plocate;

pub fn build_indexes(
    cfg: &Config,
    names: &[String],
    folders: &[PathBuf],
    progress: bool,
    native: bool,
) -> Result<()> {
    if !native {
        #[cfg(unix)]
        return plocate::build_indexes(cfg, names, folders, progress);
        #[cfg(windows)]
        bail!("plocate is unavailable on Windows; use --engine rust");
    }
    if folders.is_empty() {
        for idx in cfg.select(names)? {
            build_one(idx, progress)?;
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
            build_one(idx, progress)?;
        } else {
            let _lock = lock_database(&idx.database)?;
            let scope = idx.root.join(folder.strip_prefix(root)?);
            crate::native::update(idx, Some(&scope), progress)?;
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
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600);
    let lock = options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
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
        let builder = &mut fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn build_one(idx: &IndexConfig, progress: bool) -> Result<()> {
    if !idx.root.is_dir() {
        bail!(
            "index root does not exist or is not a directory: {}",
            idx.root.display()
        );
    }
    let _lock = lock_database(&idx.database)?;
    let start = Instant::now();
    if idx.database.starts_with(&idx.root) {
        bail!(
            "Rust index database must be outside its root: {}",
            idx.database.display()
        );
    }
    ui::log(
        Tone::Info,
        format_args!("indexing {} [rust]: {}", idx.name, idx.root.display()),
    );
    if idx.database.exists() {
        crate::native::update(idx, None, progress)?;
    } else {
        let workspace = Workspace::new(&idx.database)?;
        let mut temporary = idx.clone();
        temporary.database = workspace.0.join("index.db");
        crate::native::update(&temporary, None, progress)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&temporary.database)?
            .sync_all()?;
        fs::rename(&temporary.database, &idx.database)?;
    }
    ui::log(
        Tone::Success,
        format_args!("done {} in {:.2}s", idx.name, start.elapsed().as_secs_f64()),
    );
    Ok(())
}
