use nasfind::{config, indexer, native, search, stats, ui};

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use config::{Config, EXAMPLE_CONFIG};
use search::SearchOptions;

#[derive(Parser, Debug)]
#[command(
    name = "nasfind",
    version,
    about = "Everything-style byte-safe NAS search with plocate and Rust"
)]
struct Cli {
    /// Config file. Defaults to $NASFIND_CONFIG, ~/.config/nasfind/config.toml, then /etc/nasfind/config.toml.
    #[arg(short = 'c', long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create an example configuration file.
    Init {
        #[arg(default_value = "~/.config/nasfind/config.toml")]
        path: String,
        #[arg(long)]
        force: bool,
    },

    /// Build or update one or more filename indexes.
    Index {
        #[command(subcommand)]
        action: Option<IndexAction>,
        /// Index names. Omit to update all configured indexes.
        names: Vec<String>,
        /// Scan only this folder and merge its paths into the containing DB. May be repeated.
        #[arg(long = "folder", global = true, conflicts_with = "names")]
        folders: Vec<PathBuf>,
        /// Disable live progress (and legacy per-entry tracking).
        #[arg(long, global = true)]
        no_progress: bool,
        /// Index engine. plocate uses updatedb; Rust is the experimental native backend.
        #[arg(long, global = true, value_enum, default_value_t = IndexEngine::Plocate)]
        engine: IndexEngine,
    },

    /// Search one or more configured databases.
    Search(SearchOptions),

    /// Rank directories by indexed-entry count without scanning the filesystem.
    Stats(stats::StatsOptions),

    /// Check configuration and external dependencies.
    Doctor,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum IndexEngine {
    Rust,
    Plocate,
}

#[derive(Subcommand, Debug)]
enum IndexAction {
    /// Update existing databases and automatically initialize missing ones.
    Update { names: Vec<String> },
    /// Initialize missing databases only; leave existing databases unchanged.
    Init { names: Vec<String> },
}

fn main() {
    if let Err(err) = run() {
        ui::log(ui::Tone::Error, format_args!("error: {err:#}"));
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = normalize_implicit_search(env::args_os().collect());
    let cli = Cli::parse_from(args);

    match cli.command {
        Commands::Init { path, force } => init_config(&path, force),
        Commands::Index {
            action,
            names,
            folders,
            no_progress,
            engine,
        } => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            if action.is_some() && !names.is_empty() {
                bail!("put index names after update/init, not before it");
            }
            let names = match action {
                Some(IndexAction::Update { names }) => names,
                Some(IndexAction::Init { names }) => {
                    if !folders.is_empty() {
                        bail!("index init cannot be combined with --folder");
                    }
                    let missing: Vec<_> = cfg
                        .select(&names)?
                        .into_iter()
                        .filter(|idx| !idx.database.is_file())
                        .map(|idx| idx.name.clone())
                        .collect();
                    if missing.is_empty() {
                        ui::log(
                            ui::Tone::Warning,
                            format_args!(
                                "all selected databases already exist; use nasfind index update"
                            ),
                        );
                        return Ok(());
                    }
                    missing
                }
                None => names,
            };
            indexer::build_indexes(
                &cfg,
                &names,
                &folders,
                !no_progress,
                matches!(engine, IndexEngine::Rust),
            )?;
            stats::refresh(&cfg, &names)
                .context("index databases updated, but statistics refresh failed")
        }
        Commands::Search(options) => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            search::search(&cfg, &options)
        }
        Commands::Stats(options) => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            stats::stats(&cfg, &options)
        }
        Commands::Doctor => doctor(cli.config.as_deref()),
    }
}

fn normalize_implicit_search(mut args: Vec<OsString>) -> Vec<OsString> {
    if args.len() < 2 {
        return args;
    }
    const COMMANDS: &[&str] = &["init", "index", "search", "stats", "doctor", "help"];

    // Skip global options that may precede the command. This keeps both
    // `nasfind soil` and `nasfind --config cfg.toml soil` convenient.
    let mut pos = 1;
    while pos < args.len() {
        let arg = args[pos].to_str().unwrap_or("");
        if matches!(arg, "--config" | "-c") {
            pos += 2;
            continue;
        }
        if arg.starts_with("--config=") || arg.starts_with("-c=") {
            pos += 1;
            continue;
        }
        if matches!(arg, "-h" | "--help" | "-V" | "--version") {
            return args;
        }
        break;
    }

    if pos < args.len() {
        let first = args[pos].to_str().unwrap_or("");
        if !COMMANDS.contains(&first) {
            args.insert(pos, "search".into());
        }
    }
    args
}

fn init_config(raw_path: &str, force: bool) -> Result<()> {
    let path = expand_tilde(raw_path)?;
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, EXAMPLE_CONFIG)
        .with_context(|| format!("failed to write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn doctor(config_path: Option<&Path>) -> Result<()> {
    let (cfg, path) = Config::load(config_path)?;
    println!("config: {}", path.display());
    println!("indexes: {}", cfg.index.len());
    let mut legacy = false;
    for idx in &cfg.index {
        // Missing indexes use updatedb by default; validate every existing DB.
        legacy |= !idx.database.is_file() || !native::is_native(&idx.database)?;
    }
    if legacy {
        check_command(&cfg.tools.plocate, "--version")?;
        check_command(&cfg.tools.updatedb, "--version")?;
        check_command(&cfg.tools.plocate_build, "--version")?;
        check_command(&cfg.tools.sort, "--version")?;
    } else {
        println!("engine: rust (no external indexing/search tools required)");
    }

    let mut ok = true;
    for idx in &cfg.index {
        let root_ok = idx.root.is_dir();
        println!(
            "{}: root={} [{}], db={} [{}]",
            idx.name,
            idx.root.display(),
            if root_ok { "ok" } else { "missing" },
            idx.database.display(),
            if idx.database.is_file() {
                "ready"
            } else {
                "not built"
            }
        );
        ok &= root_ok;
    }
    if !ok {
        bail!("one or more index paths need attention");
    }
    Ok(())
}

fn check_command(program: &str, version_flag: &str) -> Result<()> {
    let output = Command::new(program)
        .arg(version_flag)
        .output()
        .with_context(|| {
            format!("cannot execute {program}; install plocate or set [tools] paths")
        })?;
    if !output.status.success() {
        bail!("{program} returned {}", output.status);
    }
    let line = String::from_utf8_lossy(&output.stdout);
    println!("{program}: {}", line.lines().next().unwrap_or("ok"));
    Ok(())
}

fn expand_tilde(path: &str) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") {
        let home = env::var("HOME").context("HOME is not set")?;
        if path == "~" {
            return Ok(PathBuf::from(home));
        }
        return Ok(PathBuf::from(home).join(&path[2..]));
    }
    Ok(PathBuf::from(path))
}
