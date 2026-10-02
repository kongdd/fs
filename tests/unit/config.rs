use super::*;

// Behavioral tests must not depend on the user's editable example config.
const TEST_CONFIG: &str = r#"
[filters]
exclude_dirs = ["node_modules"]
exclude_paths = ["shared/cache"]
exclude_extensions = ["tmp"]
exclude_files = [".DS_Store"]

[[index]]
name = "research"
root = "/research"
database = "/research.db"
exclude_paths = ["project/cache"]

[[index]]
name = "archive"
root = "/archive"
database = "/archive.db"
"#;

#[test]
fn parses_example_config() {
    let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
    assert!(!cfg.index.is_empty());
}

#[test]
fn selects_named_indexes() {
    let cfg = Config::parse(TEST_CONFIG).unwrap();
    let selected = cfg.select(&["archive".into()]).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "archive");
}

#[test]
fn rejects_unknown_indexes() {
    let cfg = Config::parse(TEST_CONFIG).unwrap();
    assert!(cfg.select(&["missing".into()]).is_err());
}

#[test]
fn local_rules_override_and_missing_rules_inherit() {
    let text = TEST_CONFIG.replace(
        "name = \"research\"",
        "exclude_extensions = [\"local\"]\nexclude_files = []\nname = \"research\"",
    );
    let cfg = Config::parse(&text).unwrap();
    assert_eq!(cfg.index[0].filters.exclude_extensions, ["local"]);
    assert!(cfg.index[0].filters.exclude_files.is_empty());
    assert_eq!(
        cfg.index[0].filters.exclude_dirs,
        cfg.index[1].filters.exclude_dirs
    );
    assert_ne!(
        cfg.index[0].filters.exclude_paths,
        cfg.index[1].filters.exclude_paths
    );
    assert_eq!(
        cfg.index[0].filters.exclude_paths,
        [PathBuf::from("project/cache")]
    );
    assert_eq!(
        cfg.index[1].filters.exclude_paths,
        [PathBuf::from("shared/cache")]
    );
    assert!(
        cfg.index[1]
            .filters
            .exclude_extensions
            .contains(&"tmp".into())
    );
    assert!(
        cfg.index[1]
            .filters
            .exclude_files
            .contains(&".DS_Store".into())
    );
}

#[test]
fn rejects_invalid_global_directory_filter() {
    let text = TEST_CONFIG.replace("node_modules", "cache/subdir");
    assert!(Config::parse(&text).is_err());
}

#[test]
fn accepts_directory_names_with_spaces() {
    let text = TEST_CONFIG.replace("node_modules", "System Volume Information");
    assert!(Config::parse(&text).is_ok());
}
