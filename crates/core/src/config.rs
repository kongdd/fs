use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::platform::{portable_absolute, portable_path};

#[derive(Debug, Clone)]
pub struct Config {
    pub tools: Tools,
    pub index: Vec<IndexConfig>,
    /// Persisted by `fs config engine`. `None` keeps the built-in rust default.
    pub engine: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tools {
    #[serde(default = "default_plocate")]
    pub plocate: String,
    #[serde(default = "default_updatedb")]
    pub updatedb: String,
    #[serde(default = "default_plocate_build")]
    pub plocate_build: String,
    #[serde(default = "default_sort")]
    pub sort: String,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            plocate: default_plocate(),
            updatedb: default_updatedb(),
            plocate_build: default_plocate_build(),
            sort: default_sort(),
        }
    }
}

fn default_plocate() -> String {
    tool_path("plocate")
}

fn default_updatedb() -> String {
    tool_path("updatedb")
}

fn default_plocate_build() -> String {
    tool_path("plocate-build")
}

fn default_sort() -> String {
    tool_path("sort")
}

fn tool_path(name: &str) -> String {
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        for directory in [
            parent.join("tools/bin"),
            parent.join("../lib/fs/tools/bin"),
            parent.join("../lib/nasfind/tools/bin"),
        ] {
            let tool = directory.join(name);
            if tool.is_file() {
                return tool.to_string_lossy().into_owned();
            }
        }
    }
    name.into()
}

#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub name: String,
    pub root: PathBuf,
    /// Fallback for both commands when a role-specific path is omitted.
    pub database: PathBuf,
    /// `fs updatedb` writes this file when set.
    pub update_database: Option<PathBuf>,
    /// `fs locate` / `fs stats` read this file when set.
    pub search_database: Option<PathBuf>,
    pub filters: Filters,
}

impl IndexConfig {
    pub fn update_database(&self) -> &Path {
        self.update_database.as_deref().unwrap_or(&self.database)
    }

    pub fn search_database(&self) -> &Path {
        self.search_database.as_deref().unwrap_or(&self.database)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filters {
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    #[serde(default)]
    pub exclude_paths: Vec<PathBuf>,
    #[serde(default)]
    pub exclude_extensions: Vec<String>,
    #[serde(default)]
    pub exclude_files: Vec<String>,
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    tools: Tools,
    #[serde(default)]
    filters: Filters,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    outdir: Option<PathBuf>,
    index: Vec<RawIndex>,
}

#[derive(Deserialize)]
struct RawIndex {
    name: String,
    root: PathBuf,
    #[serde(default)]
    database: Option<PathBuf>,
    #[serde(default)]
    update_database: Option<PathBuf>,
    #[serde(default)]
    search_database: Option<PathBuf>,
    #[serde(flatten)]
    filters: LocalFilters,
}

#[derive(Default, Deserialize)]
struct LocalFilters {
    exclude_dirs: Option<Vec<String>>,
    exclude_paths: Option<Vec<PathBuf>>,
    exclude_extensions: Option<Vec<String>>,
    exclude_files: Option<Vec<String>>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path().ok_or_else(|| {
                anyhow::anyhow!("no config found; pass --config PATH or run fs init")
            })?,
        };

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let cfg = Self::parse(&text)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        Ok((cfg, path))
    }

    fn parse(text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text)?;
        let outdir = raw.outdir.map(portable_path);
        if let Some(path) = &outdir
            && !portable_absolute(path)
        {
            bail!("outdir must be absolute: {}", path.display());
        }
        let global = raw.filters;
        let cfg = Self {
            tools: raw.tools,
            engine: normalize_engine(raw.engine)?,
            index: raw
                .index
                .into_iter()
                .map(|idx| {
                    let database = match idx.database {
                        Some(path) => portable_path(path),
                        None => {
                            let directory = outdir.as_ref().with_context(|| {
                                format!("index {} needs database or global outdir", idx.name)
                            })?;
                            if idx.name.trim().is_empty()
                                || matches!(idx.name.as_str(), "." | "..")
                                || idx.name.contains(['/', '\\', ':', '\0'])
                            {
                                bail!("invalid database name: {:?}", idx.name);
                            }
                            portable_path(directory.join(format!("{}.db", idx.name)))
                        }
                    };
                    Ok(IndexConfig {
                        name: idx.name,
                        root: portable_path(idx.root),
                        database,
                        update_database: idx.update_database.map(portable_path),
                        search_database: idx.search_database.map(portable_path),
                        filters: Filters {
                            exclude_dirs: idx
                                .filters
                                .exclude_dirs
                                .unwrap_or_else(|| global.exclude_dirs.clone()),
                            exclude_paths: idx
                                .filters
                                .exclude_paths
                                .unwrap_or_else(|| global.exclude_paths.clone())
                                .into_iter()
                                .map(portable_path)
                                .collect(),
                            exclude_extensions: idx
                                .filters
                                .exclude_extensions
                                .unwrap_or_else(|| global.exclude_extensions.clone()),
                            exclude_files: idx
                                .filters
                                .exclude_files
                                .unwrap_or_else(|| global.exclude_files.clone()),
                        },
                    })
                })
                .collect::<Result<_>>()?,
        };
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
            if !portable_absolute(&idx.root) {
                bail!(
                    "index {} root must be absolute (use /volume, C:/, or //server/share; a bare drive letter such as C: is the drive root): {}",
                    idx.name,
                    idx.root.display()
                );
            }
            for (role, path) in [
                ("database", idx.database.as_path()),
                ("update_database", idx.update_database()),
                ("search_database", idx.search_database()),
            ] {
                if !portable_absolute(path) {
                    bail!(
                        "index {} {role} path must be absolute: {}",
                        idx.name,
                        path.display()
                    );
                }
            }
            let update = idx.update_database();
            let search = idx.search_database();
            if !dbs.insert(update.to_path_buf()) {
                bail!("duplicate database path: {}", update.display());
            }
            if search != update && !dbs.insert(search.to_path_buf()) {
                bail!("duplicate database path: {}", search.display());
            }
            for name in &idx.filters.exclude_dirs {
                if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
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

    /// Copy with `database` set to the file `updatedb` should write.
    pub fn for_update(&self) -> Self {
        self.with_role_database(IndexConfig::update_database)
    }

    /// Copy with `database` set to the file `locate` and `stats` should read.
    pub fn for_search(&self) -> Self {
        self.with_role_database(IndexConfig::search_database)
    }

    fn with_role_database(&self, pick: fn(&IndexConfig) -> &Path) -> Self {
        let mut cfg = self.clone();
        for idx in &mut cfg.index {
            idx.database = pick(idx).to_path_buf();
            idx.update_database = None;
            idx.search_database = None;
        }
        cfg
    }
}

/// `C:` is drive-relative in Windows path APIs. Treat a bare drive letter as the volume root.
pub fn default_config_path() -> Option<PathBuf> {
    // Keep existing nasfind configurations usable after the rename.
    for variable in ["FS_CONFIG", "NASFIND_CONFIG"] {
        if let Some(path) = env::var_os(variable).map(PathBuf::from)
            && path.is_file()
        {
            return Some(path);
        }
    }
    let mut directories = Vec::new();
    if let Some(home) = crate::platform::home_dir() {
        directories.push(home.join(".config"));
    }
    #[cfg(windows)]
    if let Some(path) = env::var_os("APPDATA") {
        directories.push(PathBuf::from(path));
    }
    #[cfg(unix)]
    directories.push(PathBuf::from("/etc"));
    directories
        .into_iter()
        .flat_map(|directory| {
            ["fs", "nasfind"].map(|name| directory.join(name).join("config.toml"))
        })
        .find(|path| path.is_file())
}

fn normalize_engine(engine: Option<String>) -> Result<Option<String>> {
    let Some(engine) = engine else {
        return Ok(None);
    };
    match engine.as_str() {
        "rust" | "plocate" => Ok(Some(engine)),
        _ => bail!("engine must be rust or plocate, not {engine:?}"),
    }
}

#[cfg(unix)]
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config/updatedb_nas.toml");
#[cfg(windows)]
pub const EXAMPLE_CONFIG: &str = include_str!("../../../config/updatedb_win.toml");

#[cfg(test)]
#[path = "../../../tests/unit/core/config_outdir.rs"]
mod outdir_tests;

#[cfg(all(test, unix))]
#[path = "../../../tests/unit/core/config.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "../../../tests/unit/core/windows_config.rs"]
mod tests;
