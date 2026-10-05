use std::{ffi::OsString, path::PathBuf};

#[derive(clap::Args, Clone, Debug, Default)]
pub struct SearchOptions {
    /// Restrict search to named indexes. May be repeated.
    #[arg(short = 'd', long = "index")]
    pub indexes: Vec<String>,
    /// Everything expression: spaces AND, | OR, ! NOT, <...> groups, ext: and path:.
    #[arg(required = true)]
    pub patterns: Vec<OsString>,
    /// Match without ASCII/locale case sensitivity.
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    /// Match filenames only, ignoring directory names.
    #[arg(short = 'b', long)]
    pub basename: bool,
    /// Match case (Everything searches ignore ASCII case by default).
    #[arg(long, conflicts_with = "ignore_case")]
    pub case_sensitive: bool,
    /// Match terms against the full path instead of just the filename.
    #[arg(short = 'p', long, conflicts_with = "basename")]
    pub match_path: bool,
    /// Use legacy locate semantics: case-sensitive full paths, no expression parser.
    #[arg(long, conflicts_with_all = ["case_sensitive", "match_path"])]
    pub locate: bool,
    /// Check that matches still exist (accesses the filesystem/NAS).
    #[arg(short = 'e', long)]
    pub existing: bool,
    /// Interpret patterns as regular expressions (Rust syntax for native indexes) (slower).
    #[arg(short = 'r', long)]
    pub regex: bool,
    /// Include only these extensions (ASCII case-insensitive). Repeat or use commas.
    #[arg(long = "ext", value_delimiter = ',')]
    pub extensions: Vec<String>,
    /// Only inferred directories: filenames without a nonempty extension (no filesystem access).
    #[arg(long, conflicts_with = "files")]
    pub dirs: bool,
    /// Only inferred files: filenames with a nonempty extension (no filesystem access).
    #[arg(long, conflicts_with = "dirs")]
    pub files: bool,
    /// Directory names from the query-time ignore sidecar, not indexing rules.
    #[arg(skip)]
    pub ignored_dirs: Vec<String>,
    /// Restrict results to this directory subtree, without accessing the filesystem.
    #[arg(long)]
    pub path: Option<PathBuf>,
    /// Skip this many matches after filtering.
    #[arg(short = 'o', long, default_value_t = 0)]
    pub offset: usize,
    /// Stop after this many matches after filtering and offset.
    #[arg(short = 'n', long = "nlimit")]
    pub limit: Option<usize>,
    /// Map NAS output paths to /mnt/{z,x,y,o} (CMIP6, GitHub, Researches, CUG-hydro).
    #[arg(long)]
    pub mnt: bool,
    /// Write a JSON array of paths.
    #[arg(long, conflicts_with = "null")]
    pub json: bool,
    /// Separate raw paths with NUL bytes, for piping to other tools.
    #[arg(short = '0', long = "null")]
    pub null: bool,
}
