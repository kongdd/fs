mod config;
mod indexer;
mod search;

use std::{
    env, fs,
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
    about = "Fast multi-database NAS file search powered by plocate"
)]
struct Cli {
    /// Config file. Defaults to $NASFIND_CONFIG, ~/.config/nasfind/config.toml, then /etc/nasfind/config.toml.
    #[arg(long, global = true)]
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

    /// Build or update one or more plocate databases.
    Index {
        /// Index names. Omit to update all configured indexes.
        names: Vec<String>,
        /// Disable per-entry progress tracking for maximum indexing throughput.
        #[arg(long)]
        no_progress: bool,
    },

    /// Search one or more configured databases.
    Search(SearchOptions),

    /// Check configuration and external dependencies.
    Doctor,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = normalize_implicit_search(env::args().collect());
    let cli = Cli::parse_from(args);

    match cli.command {
        Commands::Init { path, force } => init_config(&path, force),
        Commands::Index { names, no_progress } => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            indexer::build_indexes(&cfg, &names, !no_progress)
        }
        Commands::Search(options) => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            search::search(&cfg, &options)
        }
        Commands::Doctor => doctor(cli.config.as_deref()),
    }
}

fn normalize_implicit_search(mut args: Vec<String>) -> Vec<String> {
    if args.len() < 2 {
        return args;
    }
    const COMMANDS: &[&str] = &["init", "index", "search", "doctor", "help"];

    // Skip global options that may precede the command. This keeps both
    // `nasfind soil` and `nasfind --config cfg.toml soil` convenient.
    let mut pos = 1;
    while pos < args.len() {
        let arg = args[pos].as_str();
        if arg == "--config" {
            pos += 2;
            continue;
        }
        if arg.starts_with("--config=") {
            pos += 1;
            continue;
        }
        if matches!(arg, "-h" | "--help" | "-V" | "--version") {
            return args;
        }
        break;
    }

    if pos < args.len() {
        let first = args[pos].as_str();
        if !first.starts_with('-') && !COMMANDS.contains(&first) {
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
    check_command(&cfg.tools.plocate, "--version")?;
    check_command(&cfg.tools.updatedb, "--version")?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_search_is_inserted() {
        let args = vec!["nasfind".into(), "soil".into(), "moisture".into()];
        let args = normalize_implicit_search(args);
        assert_eq!(args[1], "search");
        assert_eq!(args[2], "soil");
    }

    #[test]
    fn explicit_command_is_untouched() {
        let args = vec!["nasfind".into(), "index".into(), "research".into()];
        assert_eq!(normalize_implicit_search(args.clone()), args);
    }

    #[test]
    fn implicit_search_after_config_is_inserted() {
        let args = vec![
            "nasfind".into(),
            "--config".into(),
            "cfg.toml".into(),
            "soil".into(),
        ];
        let args = normalize_implicit_search(args);
        assert_eq!(args[3], "search");
        assert_eq!(args[4], "soil");
    }
}
