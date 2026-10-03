use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum IgnoreAction {
    /// Hide one or more directory names at any depth, including their descendants.
    Add {
        #[arg(required = true, num_args = 1..)]
        dirs: Vec<String>,
    },
    /// List ignored directory names.
    List,
    /// Stop hiding one or more directory names (does not restore entries absent from the index).
    Rm {
        #[arg(required = true, num_args = 1..)]
        dirs: Vec<String>,
    },
}

// A sidecar keeps the user's TOML, comments and indexing rules untouched.
fn store_path(config: &Path) -> PathBuf {
    let mut name = config.as_os_str().to_os_string();
    name.push(".ignore.json");
    PathBuf::from(name)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') {
        bail!("ignore requires a directory name, not a path: {name:?}");
    }
    Ok(())
}

pub fn load(config: &Path) -> Result<Vec<String>> {
    let path = store_path(config);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    let names: Vec<String> = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    for name in &names {
        validate_name(name).with_context(|| {
            format!(
                "invalid directory name in {}; replace old path rules with directory names",
                path.display()
            )
        })?;
    }
    Ok(names)
}

fn save(config: &Path, paths: &[String]) -> Result<()> {
    let path = store_path(config);
    let bytes = serde_json::to_vec_pretty(paths)?;
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to save {}", path.display()))
}

pub fn run(config: &Path, action: IgnoreAction) -> Result<()> {
    let mut paths = load(config)?;
    match action {
        IgnoreAction::List => {
            for path in paths {
                println!("{path}");
            }
        }
        IgnoreAction::Add { dirs } => {
            if dirs.is_empty() {
                bail!("at least one directory name is required");
            }
            for dir in &dirs {
                validate_name(dir)?;
            }
            let mut changed = false;
            for dir in &dirs {
                if !paths.contains(dir) {
                    paths.push(dir.clone());
                    changed = true;
                }
            }
            if changed {
                paths.sort();
                save(config, &paths)?;
            }
            for dir in dirs {
                println!("ignored {dir}");
            }
        }
        IgnoreAction::Rm { dirs } => {
            if dirs.is_empty() {
                bail!("at least one directory name is required");
            }
            for dir in &dirs {
                validate_name(dir)?;
            }
            paths.retain(|path| !dirs.contains(path));
            save(config, &paths)?;
            for dir in dirs {
                println!("unignored {dir}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/ignore.rs"]
mod tests;
