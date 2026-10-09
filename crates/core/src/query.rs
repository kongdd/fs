use std::{ffi::OsString, path::PathBuf};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct SearchOptions {
    /// Restrict search to named indexes. May be repeated.
    #[arg(short = 'd', long = "index", value_name = "NAME")]
    pub indexes: Vec<String>,
    /// Everything expression: spaces AND, | OR, ! NOT, <...> groups, exts: and path:.
    #[arg(required = true, value_name = "EXPR")]
    pub patterns: Vec<OsString>,
    /// Internal backend matching mode; the CLI uses case_sensitive.
    #[arg(skip)]
    pub ignore_case: bool,
    /// Internal backend scope; CLI searches use filenames unless include_path is set.
    #[arg(skip)]
    pub basename: bool,
    /// Match case (searches ignore ASCII case by default).
    #[arg(short = 'C', long)]
    pub case_sensitive: bool,
    /// Include directory paths in matching, not just filenames.
    #[arg(short = 'p', long)]
    pub include_path: bool,
    /// Check that matches still exist (accesses the filesystem/NAS).
    #[arg(short = 'e', long)]
    pub existing: bool,
    /// Interpret patterns as regular expressions (Rust syntax for native indexes) (slower).
    #[arg(short = 'r', long)]
    pub regex: bool,
    /// Include only these extensions (ASCII case-insensitive). Repeat or use commas.
    #[arg(
        long = "exts",
        alias = "ext",
        value_delimiter = ',',
        value_name = "EXTS"
    )]
    pub extensions: Vec<String>,
    /// Infer directories from names without an extension; no metadata checks.
    #[arg(long, conflicts_with = "files")]
    pub dirs: bool,
    /// Infer files from names with an extension; no metadata checks.
    #[arg(long, conflicts_with = "dirs")]
    pub files: bool,
    /// Directory names from the query-time ignore sidecar, not indexing rules.
    #[arg(skip)]
    pub ignored_dirs: Vec<String>,
    /// Restrict results to this directory subtree, without accessing the filesystem.
    #[arg(long, value_name = "DIR")]
    pub path: Option<PathBuf>,
    /// Skip this many matches after filtering.
    #[arg(short = 'o', long, default_value_t = 0, value_name = "N")]
    pub offset: usize,
    /// Stop after this many matches after filtering and offset.
    #[arg(short = 'n', long = "nlimit", value_name = "N")]
    pub limit: Option<usize>,
    /// Write a JSON array of paths.
    #[arg(long, conflicts_with = "null")]
    pub json: bool,
    /// Separate raw paths with NUL bytes, for piping to other tools.
    #[arg(short = '0', long = "null")]
    pub null: bool,
}
