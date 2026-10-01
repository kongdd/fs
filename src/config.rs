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
    #[serde(default)]
    pub filters: Filters,
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
    #[serde(flatten)]
    pub filters: Filters,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filters {
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
        let cfg = Self::parse(&text)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        Ok((cfg, path))
    }

    fn parse(text: &str) -> Result<Self> {
        let mut cfg: Self = toml::from_str(text)?;
        for idx in &mut cfg.index {
            idx.filters
                .exclude_dirs
                .extend(cfg.filters.exclude_dirs.iter().cloned());
            idx.filters
                .exclude_paths
                .extend(cfg.filters.exclude_paths.iter().cloned());
            idx.filters
                .exclude_extensions
                .extend(cfg.filters.exclude_extensions.iter().cloned());
        }
        cfg.validate()?;
        Ok(cfg)
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
            if !names.insert(&idx.name) {
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
            if !dbs.insert(&idx.database) {
                bail!("duplicate database path: {}", idx.database.display());
            }
            for name in &idx.filters.exclude_dirs {
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

        for name in names {
            if !self.index.iter().any(|idx| &idx.name == name) {
                bail!("unknown index name: {name}");
            }
        }
        let selected = self
            .index
            .iter()
            .filter(|idx| names.contains(&idx.name))
            .collect();
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

pub const EXAMPLE_CONFIG: &str = include_str!("../examples/config.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.index.len(), 2);
        assert_eq!(cfg.index[0].name, "research");
        assert_eq!(cfg.tools.plocate, "plocate");
    }

    #[test]
    fn selects_named_indexes() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        let selected = cfg.select(&["archive".into()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "archive");
    }

    #[test]
    fn rejects_unknown_indexes() {
        let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
        assert!(cfg.select(&["missing".into()]).is_err());
    }

    #[test]
    fn combines_global_and_local_filters() {
        let text = EXAMPLE_CONFIG.replace(
            "name = \"research\"",
            "exclude_extensions = [\"local\"]\nname = \"research\"",
        );
        let cfg = Config::parse(&text).unwrap();
        assert!(
            cfg.index[0]
                .filters
                .exclude_extensions
                .contains(&"local".into())
        );
        for idx in &cfg.index {
            assert!(idx.filters.exclude_extensions.contains(&"tmp".into()));
            assert!(idx.filters.exclude_dirs.contains(&"node_modules".into()));
        }
        assert!(
            !cfg.index[1]
                .filters
                .exclude_extensions
                .contains(&"local".into())
        );
    }

    #[test]
    fn rejects_invalid_global_directory_filter() {
        let text = EXAMPLE_CONFIG.replace("node_modules", "cache with spaces");
        assert!(Config::parse(&text).is_err());
    }
}
