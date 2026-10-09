use fs_locate::{config, ignore, index_search, platform, search, stats, ui};
use fs_updatedb::indexer;

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
    name = "fs",
    version,
    about = "Everything-style byte-safe NAS search with plocate and Rust"
)]
struct Cli {
    /// Config file. Overrides FS_CONFIG, `fs config set`, and the default config.
    #[arg(short = 'c', long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create an example configuration file.
    Init {
        #[arg(default_value = "~/.config/fs/config.toml")]
        path: String,
        #[arg(long)]
        force: bool,
    },

    /// Build or update one or more filename indexes.
    #[command(name = "updatedb", visible_alias = "index")]
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
        /// Parallel filesystem scan threads for the Rust engine. 1 keeps a single walker.
        #[arg(
            short = 'j',
            long,
            global = true,
            default_value_t = 1,
            value_parser = clap::value_parser!(u16).range(1..)
        )]
        jobs: u16,
        /// Index engine. Omit to use `fs config engine`, then the built-in rust default.
        /// plocate is Linux-only and must be selected explicitly.
        #[arg(long, global = true, value_enum, env = "FS_ENGINE")]
        engine: Option<IndexEngine>,
    },

    /// Show or change saved settings.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Search indexes: filenames by default, ignoring ASCII case.
    #[command(
        name = "locate",
        visible_alias = "search",
        after_help = r#"Results are full paths resolved against each index's configured root.
Words containing / and path: terms match full paths. Use -p for all terms.
Quote expressions to prevent shell expansion. --files/--dirs infer types only.

Examples:
  fs locate 'Kong 2024 *.pdf'
  fs locate '"Kong 2024" *.pdf'
  fs locate '<soil | rain> exts:nc !backup'
  fs locate -C -p Kong
  fs locate --path /data --exts docx,pdf Kong"#
    )]
    Search(SearchOptions),

    /// Rank directories by indexed-entry count without scanning the filesystem.
    Stats(stats::StatsOptions),

    /// Hide directory names at query time without rebuilding indexes.
    Ignore {
        #[command(subcommand)]
        action: ignore::IgnoreAction,
    },

    /// Check configuration and external dependencies.
    Doctor,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum IndexEngine {
    Rust,
    #[cfg_attr(not(target_os = "linux"), value(skip))]
    Plocate,
}

impl IndexEngine {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Plocate => "plocate",
        }
    }

    fn from_name(value: &str) -> Result<Self> {
        let engine = match value {
            "rust" => Self::Rust,
            "plocate" => Self::Plocate,
            _ => bail!("engine must be rust or plocate, not {value:?}"),
        };
        engine.ensure_supported()
    }

    fn ensure_supported(self) -> Result<Self> {
        if self == Self::Plocate && !cfg!(target_os = "linux") {
            bail!("plocate is only available on Linux; this system uses rust");
        }
        Ok(self)
    }
}

fn builtin_engine() -> IndexEngine {
    // Rust databases are the same format on every OS. plocate is opt-in on Linux.
    IndexEngine::Rust
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Show or set the default updatedb engine. Does not convert existing databases.
    Engine {
        /// rust, or plocate on Linux. Omit to print the current default.
        engine: Option<IndexEngine>,
    },
    /// Save the config file used by locate or updatedb. The path is stored absolute.
    Set {
        /// locate or updatedb.
        role: ConfigRole,
        /// Config file. Relative paths are resolved from the current directory.
        path: String,
    },
    /// Show the saved locate and updatedb config paths.
    List,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigRole {
    #[value(alias = "search")]
    Locate,
    #[value(alias = "index")]
    Updatedb,
}

impl ConfigRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Locate => "locate",
            Self::Updatedb => "updatedb",
        }
    }
}

fn env_engine() -> Result<Option<IndexEngine>> {
    match env::var("FS_ENGINE") {
        Ok(value) => Ok(Some(
            IndexEngine::from_name(&value).context("invalid FS_ENGINE")?,
        )),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// `--engine` and `FS_ENGINE` are already folded into `explicit` by clap.
fn resolve_engine(explicit: Option<IndexEngine>, configured: Option<&str>) -> Result<IndexEngine> {
    if let Some(engine) = explicit {
        return Ok(engine);
    }
    match configured {
        Some(value) => IndexEngine::from_name(value),
        None => builtin_engine().ensure_supported(),
    }
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
    let mut cli = Cli::parse_from(args);
    cli.config = resolve_cli_config(cli.config, config_role(&cli.command))?;

    match cli.command {
        Commands::Init { path, force } => init_config(&path, force),
        Commands::Index {
            action,
            names,
            folders,
            no_progress,
            jobs,
            engine,
        } => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            let cfg = cfg.for_update();
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
                            format_args!("all selected databases already exist; use fs updatedb"),
                        );
                        return Ok(());
                    }
                    missing
                }
                None => names,
            };
            let engine = resolve_engine(engine, cfg.engine.as_deref())?;
            indexer::build_indexes(
                &cfg,
                &names,
                &folders,
                !no_progress,
                matches!(engine, IndexEngine::Rust),
                jobs,
            )?;
            stats::refresh(&cfg, &names)
                .context("index databases updated, but statistics refresh failed")
        }
        Commands::Search(mut options) => {
            let (cfg, path) = Config::load(cli.config.as_deref())?;
            let cfg = cfg.for_search();
            options.ignored_dirs = ignore::load(&path)?;
            search::search(&cfg, &options)
        }
        Commands::Stats(options) => {
            let (cfg, _) = Config::load(cli.config.as_deref())?;
            stats::stats(&cfg.for_search(), &options)
        }
        Commands::Ignore { action } => {
            let (_, path) = Config::load(cli.config.as_deref())?;
            ignore::run(&path, action)
        }
        Commands::Config { action } => config_command(cli.config.as_deref(), action),
        Commands::Doctor => doctor(cli.config.as_deref()),
    }
}

fn config_role(command: &Commands) -> Option<ConfigRole> {
    match command {
        Commands::Index { .. } => Some(ConfigRole::Updatedb),
        Commands::Search(_) | Commands::Stats(_) | Commands::Ignore { .. } => {
            Some(ConfigRole::Locate)
        }
        Commands::Config {
            action: ConfigAction::Engine { .. },
        } => Some(ConfigRole::Updatedb),
        _ => None,
    }
}

fn pick_config(
    explicit: Option<PathBuf>,
    env_config: bool,
    saved: Option<PathBuf>,
) -> Option<PathBuf> {
    if explicit.is_some() || env_config {
        explicit
    } else {
        saved
    }
}

fn resolve_cli_config(
    explicit: Option<PathBuf>,
    role: Option<ConfigRole>,
) -> Result<Option<PathBuf>> {
    let saved = match role {
        Some(role) if explicit.is_none() && config::env_config_file().is_none() => {
            saved_role(role)?
        }
        _ => None,
    };
    Ok(pick_config(
        explicit,
        config::env_config_file().is_some(),
        saved,
    ))
}

fn config_command(config: Option<&Path>, action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Engine { engine } => config_engine(config, engine),
        ConfigAction::Set { role, path } => config_set(role, &path),
        ConfigAction::List => config_list(),
    }
}

fn config_engine(config: Option<&Path>, engine: Option<IndexEngine>) -> Result<()> {
    let (_, path) = Config::load(config)?;
    if let Some(engine) = engine {
        engine.ensure_supported()?;
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        write_text_atomic(&path, set_config_engine(&text, engine.as_str())?)?;
        if env::var_os("FS_ENGINE").is_some() {
            ui::log(
                ui::Tone::Warning,
                format_args!("FS_ENGINE overrides the config file for this process"),
            );
        }
    }
    let (cfg, path) = Config::load(config)?;
    let from_env = env_engine()?;
    let effective = resolve_engine(from_env, cfg.engine.as_deref())?;
    let source = match (from_env, cfg.engine.as_deref()) {
        (Some(_), Some(saved)) if saved != effective.as_str() => {
            format!("FS_ENGINE; config has {saved}")
        }
        (Some(_), _) => "FS_ENGINE".to_string(),
        (None, Some(_)) => format!("config {}", path.display()),
        (None, None) => "built-in default".to_string(),
    };
    println!("engine: {} ({source})", effective.as_str());
    Ok(())
}

fn set_config_engine(text: &str, engine: &str) -> Result<String> {
    set_toml_key(text, "engine", &format!("\"{engine}\""))
}

fn set_toml_key(text: &str, key: &str, literal: &str) -> Result<String> {
    let values: std::collections::BTreeMap<String, toml::Spanned<toml::Value>> =
        toml::from_str(text)?;
    let mut out = text.to_string();
    if let Some(value) = values.get(key) {
        out.replace_range(value.span(), literal);
    } else {
        let offset = text
            .split_inclusive('\n')
            .take_while(|line| {
                line.ends_with('\n')
                    && (line.trim().is_empty() || line.trim_start().starts_with('#'))
            })
            .map(str::len)
            .sum();
        out.insert_str(offset, &format!("{key} = {literal}\n"));
    }
    Ok(out)
}

fn quote_toml(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn absolute_config_path(raw: &str) -> Result<PathBuf> {
    if raw.is_empty() {
        bail!("config path cannot be empty");
    }
    let path = std::path::absolute(expand_tilde(raw)?).context("failed to resolve config path")?;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

fn roles_file() -> Option<PathBuf> {
    platform::home_dir().map(|home| home.join(".config/fs/roles.toml"))
}

fn roles_path() -> Result<PathBuf> {
    roles_file().context("home directory is not set")
}

fn saved_role(role: ConfigRole) -> Result<Option<PathBuf>> {
    let Some(file) = roles_file() else {
        return Ok(None);
    };
    role_from(&file, role)
}

fn role_from(file: &Path, role: ConfigRole) -> Result<Option<PathBuf>> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", file.display()));
        }
    };
    let value: toml::Value =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", file.display()))?;
    match value.get(role.as_str()) {
        None => Ok(None),
        Some(toml::Value::String(path)) => Ok(Some(PathBuf::from(path))),
        Some(_) => bail!("{} in {} must be a path", role.as_str(), file.display()),
    }
}

fn config_set(role: ConfigRole, raw: &str) -> Result<()> {
    let path = absolute_config_path(raw)?;
    let text_path = path.to_str().context("config path must be valid Unicode")?;
    let file = roles_path()?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let existing = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", file.display()));
        }
    };
    write_text_atomic(
        &file,
        set_toml_key(&existing, role.as_str(), &quote_toml(text_path))?,
    )?;
    println!("{}: {text_path}", role.as_str());
    Ok(())
}

fn config_list() -> Result<()> {
    let file = roles_file();
    for role in [ConfigRole::Locate, ConfigRole::Updatedb] {
        let value = file
            .as_deref()
            .map(|file| role_from(file, role))
            .transpose()?
            .flatten()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not set".to_string());
        println!("{}: {value}", role.as_str());
    }
    Ok(())
}

fn write_text_atomic(path: &Path, text: String) -> Result<()> {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let result = (|| -> Result<()> {
        fs::write(&temporary, text.as_bytes())?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to update {}", path.display()))
}

fn normalize_implicit_search(mut args: Vec<OsString>) -> Vec<OsString> {
    if args.len() < 2 {
        return args;
    }
    const COMMANDS: &[&str] = &[
        "init", "updatedb", "index", "locate", "search", "stats", "ignore", "config", "doctor",
        "help",
    ];

    // Skip global options that may precede the command. This keeps both
    // `fs soil` and `fs --config cfg.toml soil` convenient.
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
            args.insert(pos, "locate".into());
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
    let from_env = env_engine()?;
    let default_engine = resolve_engine(from_env, cfg.engine.as_deref())?;
    println!(
        "default engine: {} ({})",
        default_engine.as_str(),
        match (from_env, cfg.engine.as_deref()) {
            (Some(_), _) => "FS_ENGINE",
            (None, Some(_)) => "config",
            (None, None) => "built-in default",
        }
    );
    let missing_use_plocate = matches!(default_engine, IndexEngine::Plocate);
    let mut legacy = false;
    for idx in &cfg.index {
        // Existing DBs identify their backend; missing DBs use the default engine.
        let mut saw_file = false;
        for path in [idx.update_database(), idx.search_database()] {
            if path.is_file() {
                saw_file = true;
                legacy |= !index_search::is_rust_index(path)?;
            }
        }
        if !saw_file {
            legacy |= missing_use_plocate;
        }
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
        let update = idx.update_database();
        let search = idx.search_database();
        let ready = |path: &Path| {
            if path.is_file() { "ready" } else { "not built" }
        };
        if update == search {
            println!(
                "{}: root={} [{}], db={} [{}]",
                idx.name,
                idx.root.display(),
                if root_ok { "ok" } else { "missing" },
                update.display(),
                ready(update)
            );
        } else {
            println!(
                "{}: root={} [{}], update={} [{}], search={} [{}]",
                idx.name,
                idx.root.display(),
                if root_ok { "ok" } else { "missing" },
                update.display(),
                ready(update),
                search.display(),
                ready(search)
            );
        }
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
    if path == "~" || path.starts_with("~/") || (cfg!(windows) && path.starts_with("~\\")) {
        let home = platform::home_dir().context("home directory is not set")?;
        if path == "~" {
            return Ok(home);
        }
        return Ok(home.join(&path[2..]));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
#[path = "../tests/unit/locate/cli.rs"]
mod tests;
