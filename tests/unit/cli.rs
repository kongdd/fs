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

#[test]
fn index_actions_accept_shared_options() {
    for args in [
        vec!["nasfind", "index", "update", "research", "--no-progress"],
        vec!["nasfind", "index", "--no-progress", "update", "research"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Commands::Index {
            action: Some(IndexAction::Update { names }), no_progress: true, ..
        } if names == ["research"]));
    }
    let cli = Cli::try_parse_from(["nasfind", "index", "init"]).unwrap();
    assert!(matches!(cli.command, Commands::Index {
        action: Some(IndexAction::Init { names }), ..
    } if names.is_empty()));
    assert!(
        Cli::try_parse_from([
            "nasfind",
            "index",
            "update",
            "research",
            "--folder",
            "/research"
        ])
        .is_err()
    );
}

#[test]
fn stats_is_a_command_with_explicit_boolean_and_positive_top() {
    let args = [
        "nasfind",
        "stats",
        "--recursive",
        "false",
        "-n20",
        "-d",
        "cmip6",
    ];
    let normalized = normalize_implicit_search(args.into_iter().map(OsString::from).collect());
    let cli = Cli::try_parse_from(normalized).unwrap();
    assert!(matches!(cli.command, Commands::Stats(options)
        if !options.recursive && options.top.get() == 20 && options.indexes == ["cmip6"]));
    assert!(Cli::try_parse_from(["nasfind", "stats", "-n0"]).is_err());
    assert!(Cli::try_parse_from(["nasfind", "stats", "--recursive", "yes"]).is_err());
}

#[test]
fn implicit_search_accepts_options() {
    for options in [
        vec!["-i", "soil"],
        vec!["--json", "-d", "research", "soil"],
        vec!["--", "--version"],
    ] {
        let mut args = vec![OsString::from("nasfind")];
        args.extend(options.iter().map(|arg| OsString::from(*arg)));
        let normalized = normalize_implicit_search(args);
        assert_eq!(normalized[1], "search");
        assert!(Cli::try_parse_from(normalized).is_ok());
    }
}
