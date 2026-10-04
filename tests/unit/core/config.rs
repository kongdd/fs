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
fn update_and_search_can_use_different_databases() {
    let cfg = Config::parse(
        r#"
[[index]]
name = "files"
root = "/data"
database = "/data/files.db"
update_database = "/tmp/files-build.db"
search_database = "/var/files.db"
"#,
    )
    .unwrap();
    let idx = &cfg.index[0];
    assert_eq!(
        idx.update_database(),
        std::path::Path::new("/tmp/files-build.db")
    );
    assert_eq!(idx.search_database(), std::path::Path::new("/var/files.db"));
    assert_eq!(
        cfg.for_update().index[0].database,
        std::path::PathBuf::from("/tmp/files-build.db")
    );
    assert_eq!(
        cfg.for_search().index[0].database,
        std::path::PathBuf::from("/var/files.db")
    );
    assert!(
        Config::parse(
            r#"
[[index]]
name = "files"
root = "/data"
database = "/data/files.db"
update_database = "relative.db"
"#,
        )
        .is_err()
    );
}

#[test]
fn windows_and_nas_paths_are_absolute_with_slashes() {
    let cfg = Config::parse(
        r#"
[[index]]
name = "win"
root = "C:\\Users"
database = "C:\\fs\\c.db"
search_database = "//nas/share/c.db"

[[index]]
name = "nas"
root = "/volume1/CMIP6"
database = "/volume1/CMIP6/fs.db"
"#,
    )
    .unwrap();
    assert_eq!(cfg.index[0].root, std::path::PathBuf::from("C:/Users"));
    assert_eq!(
        cfg.index[0].search_database(),
        std::path::Path::new("//nas/share/c.db")
    );
    assert!(cfg.for_search().index.iter().any(|idx| idx.name == "nas"));
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
