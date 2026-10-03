use super::*;

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
    let cli = Cli::try_parse_from(["fs", "updatedb"]).unwrap();
    assert!(matches!(
        cli.command,
        Commands::Index {
            engine: IndexEngine::Rust,
            ..
        }
    ));
    let args = ["fs", "-c", "config.toml", "locate", "soil"];
    let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
    let cli = Cli::try_parse_from(normalized).unwrap();
    assert!(matches!(cli.command, Commands::Search(_)));
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
