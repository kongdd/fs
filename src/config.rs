use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub tools: Tools,
    pub index: Vec<IndexConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tools {
    #[serde(default = "default_plocate")]
    pub plocate: String,
    #[serde(default = "default_updatedb")]
    pub updatedb: String,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            plocate: default_plocate(),
            updatedb: default_updatedb(),
        }
    }
}

fn default_plocate() -> String {
    "plocate".into()
}

fn default_updatedb() -> String {
    "updatedb".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexConfig {
    pub name: String,
    pub root: PathBuf,
    pub database: PathBuf,
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    #[serde(default)]
    pub exclude_paths: Vec<PathBuf>,
    #[serde(default)]
    pub exclude_extensions: Vec<String>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path().ok_or_else(|| {
                anyhow::anyhow!(
                    "no config found; pass --config PATH or create ~/.config/nasfind/config.toml"
                )
            })?,
        };

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        cfg.validate()?;
        Ok((cfg, path))
    }

    pub fn validate(&self) -> Result<()> {
        if self.index.is_empty() {
            bail!("config must contain at least one [[index]] section");
        }

        let mut names = HashSet::new();
        let mut dbs = HashSet::new();
        for idx in &self.index {
            if idx.name.trim().is_empty() {
                bail!("index name cannot be empty");
            }
            if !names.insert(idx.name.clone()) {
                bail!("duplicate index name: {}", idx.name);
            }
            if !idx.root.is_absolute() {
                bail!(
                    "index {} root must be absolute: {}",
                    idx.name,
                    idx.root.display()
                );
            }
            if !idx.database.is_absolute() {
                bail!(
                    "index {} database path must be absolute: {}",
                    idx.name,
                    idx.database.display()
                );
            }
            if !dbs.insert(idx.database.clone()) {
                bail!("duplicate database path: {}", idx.database.display());
            }
            for name in &idx.exclude_dirs {
                if name.chars().any(char::is_whitespace) {
                    bail!(
                        "index {} exclude_dirs entry {:?} contains whitespace; use exclude_paths instead",
                        idx.name,
                        name
                    );
                }
                if name.contains('/') {
                    bail!(
                        "index {} exclude_dirs entry {:?} contains '/'; use exclude_paths instead",
                        idx.name,
                        name
                    );
                }
            }
        }
        Ok(())
    }

    pub fn select<'a>(&'a self, names: &[String]) -> Result<Vec<&'a IndexConfig>> {
        if names.is_empty() {
            return Ok(self.index.iter().collect());
        }

        let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
        let selected: Vec<_> = self
            .index
            .iter()
            .filter(|idx| wanted.contains(idx.name.as_str()))
            .collect();

        let found: HashSet<&str> = selected.iter().map(|idx| idx.name.as_str()).collect();
        let missing: Vec<_> = wanted.difference(&found).copied().collect();
        if !missing.is_empty() {
            bail!("unknown index name(s): {}", missing.join(", "));
        }
        Ok(selected)
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("NASFIND_CONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(home) = env::var("HOME") {
        let path = PathBuf::from(home).join(".config/nasfind/config.toml");
        if path.is_file() {
            return Some(path);
        }
    }

    let system = PathBuf::from("/etc/nasfind/config.toml");
    system.is_file().then_some(system)
}

pub const EXAMPLE_CONFIG: &str = r##"# nasfind configuration
# Each [[index]] is maintained as a separate plocate database.

[tools]
plocate = "plocate"
updatedb = "updatedb"

[[index]]
name = "research"
root = "/volume1/research"
database = "/var/lib/nasfind/research.db"
exclude_dirs = [".git", "node_modules", "target", "@eaDir", "#recycle"]
exclude_paths = []
exclude_extensions = ["tmp", "part", "pyc"]

[[index]]
name = "archive"
root = "/volume2/archive"
database = "/var/lib/nasfind/archive.db"
exclude_dirs = ["@eaDir", "#recycle"]
exclude_paths = []
exclude_extensions = ["tmp", "part"]
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let cfg: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.index.len(), 2);
        assert_eq!(cfg.index[0].name, "research");
        assert_eq!(cfg.tools.plocate, "plocate");
    }

    #[test]
    fn selects_named_indexes() {
        let cfg: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        let selected = cfg.select(&["archive".into()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "archive");
    }

    #[test]
    fn rejects_unknown_indexes() {
        let cfg: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        assert!(cfg.select(&["missing".into()]).is_err());
    }
}
