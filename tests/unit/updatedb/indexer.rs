use super::*;

#[test]
fn discovers_spaced_names_at_any_depth_without_following_excluded_trees() {
    let workspace = Workspace::new(&std::env::temp_dir().join("nasfind-test.db")).unwrap();
    let root = &workspace.0;
    for path in [
        "nested/System Volume Information",
        "System Volume Information/child/System Volume Information",
        "node_modules/System Volume Information",
        "cache/System Volume Information",
        "System",
        "Volume",
    ] {
        fs::create_dir_all(root.join(path)).unwrap();
    }
    std::os::unix::fs::symlink(root, root.join("loop")).unwrap();
    let idx = IndexConfig {
        name: "test".into(),
        root: root.clone(),
        database: root.join("test.db"),
        filters: crate::config::Filters {
            exclude_dirs: vec!["node_modules".into(), "System Volume Information".into()],
            exclude_paths: vec!["cache".into()],
            ..Default::default()
        },
    };
    let mut paths = spaced_directory_paths(&idx).unwrap();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            root.join("System Volume Information"),
            root.join("nested/System Volume Information")
        ]
    );
}

#[test]
fn progress_estimates_remaining_time_from_previous_count() {
    let status = progress_status(50, Duration::from_secs(10), Some(100), false);
    assert!(status.contains("~50%"));
    assert!(status.contains("ETA ~0m10s"));
}

#[test]
fn progress_never_claims_completion_from_an_estimate() {
    let status = progress_status(150, Duration::from_secs(10), Some(100), false);
    assert!(status.contains("~99%"));
    assert!(status.contains("past estimate; ETA unknown"));
    let first_run = progress_status(0, Duration::ZERO, None, false);
    assert!(!first_run.contains('%'));
    assert!(first_run.contains("ETA unavailable"));
    let finalizing = progress_status(50, Duration::from_secs(10), Some(100), true);
    assert!(finalizing.contains("finishing; ETA unknown"));
}

#[test]
fn progress_is_compact() {
    let status = progress_status(50, Duration::from_secs(10), Some(100), false);
    assert!(status.starts_with("[====----] ~50%"));
    assert!(status.chars().count() <= 60, "{status}");
    let unknown = progress_status(50, Duration::from_secs(10), None, false);
    assert!(!unknown.contains('['));
    assert!(unknown.chars().count() <= 45, "{unknown}");
}

#[test]
fn escapes_database_list_separators() {
    let escaped = database_arg(Path::new(r"/tmp/a:b\c.db"));
    assert_eq!(escaped.as_bytes(), br"/tmp/a\:b\\c.db");
}
