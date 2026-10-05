use super::*;

fn parse_without_engine_env(args: &[&str]) -> Cli {
    let command = <Cli as clap::CommandFactory>::command().mut_subcommand("updatedb", |cmd| {
        cmd.mut_arg("engine", |arg| arg.env(None::<&str>))
    });
    let matches = command.try_get_matches_from(args).unwrap();
    <Cli as clap::FromArgMatches>::from_arg_matches(&matches).unwrap()
}

#[test]
fn implicit_search_is_inserted() {
    let args = vec!["fs".into(), "soil".into(), "moisture".into()];
    let args = normalize_implicit_search(args);
    assert_eq!(args[1], "locate");
    assert_eq!(args[2], "soil");
}

#[test]
fn explicit_command_is_untouched() {
    let args = vec!["fs".into(), "index".into(), "research".into()];
    assert_eq!(normalize_implicit_search(args.clone()), args);
}

#[test]
fn implicit_search_after_config_is_inserted() {
    let args = vec![
        "fs".into(),
        "--config".into(),
        "cfg.toml".into(),
        "soil".into(),
    ];
    let args = normalize_implicit_search(args);
    assert_eq!(args[3], "locate");
    assert_eq!(args[4], "soil");
}

#[test]
fn updatedb_and_locate_are_canonical_commands() {
    let cli = parse_without_engine_env(&["fs", "updatedb"]);
    assert!(matches!(cli.command, Commands::Index { engine: None, .. }));
    let args = ["fs", "-c", "config.toml", "locate", "soil"];
    let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
    let cli = Cli::try_parse_from(normalized).unwrap();
    assert!(matches!(cli.command, Commands::Search(_)));
}

#[test]
fn index_engine_defaults_to_plocate_and_rust_is_explicit() {
    for args in [
        vec!["fs", "updatedb"],
        vec!["fs", "index", "update"],
        vec!["fs", "index", "init"],
    ] {
        let cli = parse_without_engine_env(&args);
        assert!(matches!(cli.command, Commands::Index { engine: None, .. }));
    }
    for args in [
        vec!["fs", "updatedb", "--engine", "rust"],
        vec!["fs", "index", "update", "--engine", "rust"],
    ] {
        let cli = parse_without_engine_env(&args);
        assert!(matches!(
            cli.command,
            Commands::Index {
                engine: Some(IndexEngine::Rust),
                ..
            }
        ));
    }
}

#[test]
fn config_engine_command_parses_and_rewrites_without_dropping_comments() {
    let cli = Cli::try_parse_from(["fs", "config", "engine", "rust"]).unwrap();
    assert!(matches!(
        cli.command,
        Commands::Config {
            action: ConfigAction::Engine {
                engine: Some(IndexEngine::Rust)
            }
        }
    ));
    let cli = Cli::try_parse_from(["fs", "config", "engine"]).unwrap();
    assert!(matches!(
        cli.command,
        Commands::Config {
            action: ConfigAction::Engine { engine: None }
        }
    ));
    let text = "# keep me\n[filters]\nexclude_dirs = [\".git\"]\n\n[[index]]\nname = \"files\"\n";
    let updated = set_config_engine(text, "rust").unwrap();
    assert!(updated.starts_with("# keep me\nengine = \"rust\"\n[filters]\n"));
    let replaced = set_config_engine("engine = \"plocate\"\n[filters]\n", "rust").unwrap();
    assert_eq!(replaced, "engine = \"rust\"\n[filters]\n");
    assert_eq!(
        resolve_engine(None, Some("rust")).unwrap(),
        IndexEngine::Rust
    );
    assert_eq!(resolve_engine(None, None).unwrap(), IndexEngine::Rust);
    if cfg!(target_os = "linux") {
        assert_eq!(
            resolve_engine(Some(IndexEngine::Plocate), Some("rust")).unwrap(),
            IndexEngine::Plocate
        );
    } else {
        assert!(resolve_engine(None, Some("plocate")).is_err());
        assert!(Cli::try_parse_from(["fs", "config", "engine", "plocate"]).is_err());
    }
}

#[test]
fn config_engine_rewrites_quoted_keys_without_changing_other_text() {
    for key in ["engine", r#""engine""#, "'engine'", r#""eng\u0069ne""#] {
        let text = format!("# 引擎\r\n{key} = 'plocate' # keep\r\n[tools]\nplocate = 'custom'\n");
        let updated = set_config_engine(&text, "rust").unwrap();
        assert_eq!(updated, text.replace("'plocate'", "\"rust\""));
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(parsed["engine"].as_str(), Some("rust"));
    }
    for text in ["", "# comment", "# comment\n", "unused = [\n[1, 2]\n]\n"] {
        let updated = set_config_engine(text, "rust").unwrap();
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(parsed["engine"].as_str(), Some("rust"));
    }
    assert!(set_config_engine("invalid = [", "rust").is_err());
}

#[test]
fn nas_updatedb_accepts_config_and_compact_jobs() {
    let args = ["fs", "-c", "config/updatedb_nas.toml", "updatedb", "-j4"];
    let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
    assert_eq!(normalized, args.map(OsString::from));
    let cli = parse_without_engine_env(&args);
    assert_eq!(cli.config, Some(PathBuf::from("config/updatedb_nas.toml")));
    assert!(matches!(cli.command, Commands::Index { jobs: 4, .. }));
}

#[test]
fn updatedb_jobs_controls_scan_threads() {
    let cli = parse_without_engine_env(&["fs", "updatedb", "-j", "4"]);
    assert!(matches!(cli.command, Commands::Index { jobs: 4, .. }));
    let cli = parse_without_engine_env(&["fs", "updatedb", "-j8"]);
    assert!(matches!(cli.command, Commands::Index { jobs: 8, .. }));
    let cli = parse_without_engine_env(&["fs", "updatedb", "--jobs=8"]);
    assert!(matches!(cli.command, Commands::Index { jobs: 8, .. }));
    let cli = parse_without_engine_env(&["fs", "index", "update", "--jobs", "2"]);
    assert!(matches!(cli.command, Commands::Index { jobs: 2, .. }));
    assert!(Cli::try_parse_from(["fs", "updatedb", "-j", "0"]).is_err());
}

#[test]
fn index_actions_accept_shared_options() {
    for args in [
        vec!["fs", "index", "update", "research", "--no-progress"],
        vec!["fs", "index", "--no-progress", "update", "research"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Commands::Index {
            action: Some(IndexAction::Update { names }), no_progress: true, ..
        } if names == ["research"]));
    }
    let cli = Cli::try_parse_from(["fs", "index", "init"]).unwrap();
    assert!(matches!(cli.command, Commands::Index {
        action: Some(IndexAction::Init { names }), ..
    } if names.is_empty()));
    assert!(
        Cli::try_parse_from(["fs", "index", "update", "research", "--folder", "/research"])
            .is_err()
    );
}

#[test]
fn stats_is_a_command_with_explicit_boolean_and_positive_top() {
    let args = ["fs", "stats", "--recursive", "false", "-n20", "-d", "cmip6"];
    let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
    let cli = Cli::try_parse_from(normalized).unwrap();
    assert!(matches!(cli.command, Commands::Stats(options)
        if !options.recursive && options.top.get() == 20 && options.indexes == ["cmip6"]));
    assert!(Cli::try_parse_from(["fs", "stats", "-n0"]).is_err());
    assert!(Cli::try_parse_from(["fs", "stats", "--recursive", "yes"]).is_err());
}

#[test]
fn ignore_commands_are_not_implicit_searches() {
    for args in [
        vec![
            "fs",
            "ignore",
            "add",
            "cache",
            "node_modules",
            "cache with spaces",
        ],
        vec!["fs", "--config", "cfg.toml", "ignore", "list"],
        vec!["fs", "ignore", "rm", "cache"],
    ] {
        let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
        let cli = Cli::try_parse_from(normalized).unwrap();
        assert!(matches!(cli.command, Commands::Ignore { .. }));
    }
    let cli = Cli::try_parse_from(["fs", "ignore", "add", "cache", "node_modules"]).unwrap();
    assert!(matches!(cli.command, Commands::Ignore {
        action: ignore::IgnoreAction::Add { dirs }
    } if dirs == ["cache", "node_modules"]));
    assert!(Cli::try_parse_from(["fs", "ignore", "add"]).is_err());
    let cli = Cli::try_parse_from(["fs", "ignore", "rm", "cache", "node_modules"]).unwrap();
    assert!(matches!(cli.command, Commands::Ignore {
        action: ignore::IgnoreAction::Rm { dirs }
    } if dirs == ["cache", "node_modules"]));
    assert!(Cli::try_parse_from(["fs", "ignore", "rm"]).is_err());
}

#[test]
fn implicit_search_accepts_mount_mapping() {
    let cli = Cli::try_parse_from(normalize_implicit_search(
        ["fs", "--mnt", "--files", "soil"]
            .into_iter()
            .map(OsString::from)
            .collect(),
    ))
    .unwrap();
    assert!(matches!(cli.command, Commands::Search(options) if options.mnt && options.files));
}

#[test]
fn search_kind_flags_are_mutually_exclusive() {
    for flag in ["--dirs", "--files"] {
        let args = ["fs", flag, "soil"];
        let cli = Cli::try_parse_from(normalize_implicit_search(
            args.into_iter().map(OsString::from).collect(),
        ))
        .unwrap();
        assert!(matches!(cli.command, Commands::Search(options)
            if options.dirs == (flag == "--dirs") && options.files == (flag == "--files")));
    }
    assert!(Cli::try_parse_from(["fs", "search", "--dirs", "--files", "soil"]).is_err());
}

#[test]
fn implicit_search_accepts_options() {
    for options in [
        vec!["-i", "soil"],
        vec!["--json", "-d", "research", "soil"],
        vec!["--", "--version"],
    ] {
        let mut args = vec![OsString::from("fs")];
        args.extend(options.iter().map(|arg| OsString::from(*arg)));
        let normalized = normalize_implicit_search(args);
        assert_eq!(normalized[1], "locate");
        assert!(Cli::try_parse_from(normalized).is_ok());
    }
}
