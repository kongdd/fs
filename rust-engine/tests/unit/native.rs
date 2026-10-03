use super::*;
use crate::config::Filters;

struct Fixture {
    _workspace: PathBuf,
    idx: IndexConfig,
}

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("nasfind-native-{}-{nonce}", std::process::id()));
        let root = workspace.join("files");
        fs::create_dir_all(&root).unwrap();
        Self {
            idx: IndexConfig {
                name: "native".into(),
                root,
                database: workspace.join("native.db"),
                filters: Filters::default(),
            },
            _workspace: workspace,
        }
    }
    fn query(
        &self,
        patterns: &[&str],
        basename: bool,
        ignore_case: bool,
        regex: bool,
    ) -> Vec<Vec<u8>> {
        let options = SearchOptions {
            patterns: patterns.iter().map(Into::into).collect(),
            basename,
            ignore_case,
            regex,
            ..Default::default()
        };
        let mut result = Vec::new();
        visit(&self.idx, &Query::new(&options).unwrap(), |path| {
            result.push(path.to_vec());
            Ok(true)
        })
        .unwrap();
        result.sort();
        result
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self._workspace).ok();
    }
}

#[test]
fn varints_roundtrip_and_reject_corruption() {
    let ids = [1, 127, 128, 99999, u32::MAX as u64, u64::MAX];
    assert_eq!(unpack(&pack(&ids)).unwrap(), ids);
    assert!(unpack(&[128]).is_err());
    assert!(unpack(&[255; 11]).is_err());
    assert!(unpack(&[1, 0]).is_err());
}

#[test]
fn byte_safe_search_matches_reference_for_literals_globs_and_regex() {
    let fixture = Fixture::new();
    for name in [
        "soil_ERA5.nc",
        "SOIL.csv",
        "a.txt",
        "rain_2019.tif",
        "rain_2020.tif",
        "中文.nc",
        "line\nfile.nc",
    ] {
        fs::write(fixture.idx.root.join(name), b"").unwrap();
    }
    let invalid = std::ffi::OsString::from_vec(b"bad_\xff.nc".to_vec());
    fs::write(fixture.idx.root.join(invalid), b"").unwrap();
    update(&fixture.idx, None, false).unwrap();
    let connection = open_read(&fixture.idx.database).unwrap();
    let all = connection
        .prepare("SELECT path FROM paths")
        .unwrap()
        .query_map([], |r| r.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (patterns, regex) in [
        (vec!["soil"], false),
        (vec!["SOIL"], false),
        (vec!["nc"], false),
        (vec!["*.nc"], false),
        (vec!["*rain_20[12]?*.tif"], false),
        (vec!["*rain_20[!0]?*.tif"], false),
        (vec!["nc", "soil"], false),
        (vec!["bad"], false),
        (vec!["中文"], false),
        (vec!["line"], false),
        (vec!["a"], false),
        (vec![""], false),
        (vec!["rain_20(19|20)[.]tif$"], true),
    ] {
        for basename in [false, true] {
            for ignore_case in [false, true] {
                let options = SearchOptions {
                    patterns: patterns.iter().map(Into::into).collect(),
                    basename,
                    ignore_case,
                    regex,
                    ..Default::default()
                };
                let query = Query::new(&options).unwrap();
                let mut expected: Vec<_> =
                    all.iter().filter(|p| query.matches(p)).cloned().collect();
                expected.sort();
                assert_eq!(
                    fixture.query(&patterns, basename, ignore_case, regex),
                    expected,
                    "{patterns:?} basename={basename} ignore_case={ignore_case}"
                );
            }
        }
    }
    assert_eq!(fixture.query(&["bad"], true, false, false).len(), 1);
    assert_eq!(fixture.query(&["SOIL"], true, true, false).len(), 2);
}

#[test]
fn bulk_build_creates_complete_indexes_and_deduplicated_frequencies() {
    let fixture = Fixture::new();
    for name in ["zzzzzz.nc", "ZZZzzz.txt", "aaaaaaa.dat"] {
        fs::write(fixture.idx.root.join(name), b"").unwrap();
    }
    update(&fixture.idx, None, false).unwrap();
    let connection = open_read(&fixture.idx.database).unwrap();
    let indexes: i64 = connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('directory_entries','directory_postings')", [], |r| r.get(0)).unwrap();
    assert_eq!(indexes, 2);
    let zzz = trigram_keys(b"zzz").next().unwrap();
    let frequency: i64 = connection
        .query_row("SELECT n FROM frequencies WHERE gram=?", [zzz], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(frequency, 2); // Repeated/folded grams counted once per entry.
    let missing: i64 = connection.query_row("SELECT count(*) FROM (SELECT gram,SUM(n) FROM postings GROUP BY gram EXCEPT SELECT gram,n FROM frequencies)", [], |r| r.get(0)).unwrap();
    let extra: i64 = connection.query_row("SELECT count(*) FROM (SELECT gram,n FROM frequencies EXCEPT SELECT gram,SUM(n) FROM postings GROUP BY gram)", [], |r| r.get(0)).unwrap();
    assert_eq!((missing, extra), (0, 0));
    drop(connection);
    fs::remove_file(fixture.idx.root.join("zzzzzz.nc")).unwrap();
    update(&fixture.idx, None, false).unwrap();
    assert_eq!(fixture.query(&["zzz"], true, true, false).len(), 1);
    let connection = open_read(&fixture.idx.database).unwrap();
    let frequency: i64 = connection
        .query_row("SELECT n FROM frequencies WHERE gram=?", [zzz], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(frequency, 1);
}

#[test]
fn reuse_checks_descendants_and_handles_deletion_and_rename() {
    let fixture = Fixture::new();
    let nested = fixture.idx.root.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("before.nc"), b"").unwrap();
    let first = update(&fixture.idx, None, false).unwrap();
    assert_eq!(first.scanned, 2);
    let second = update(&fixture.idx, None, false).unwrap();
    assert_eq!(second.scanned, 0);
    assert_eq!(second.reused, 2);
    fs::rename(nested.join("before.nc"), nested.join("after.nc")).unwrap();
    let changed = update(&fixture.idx, None, false).unwrap();
    assert_eq!(changed.scanned, 1);
    assert_eq!(changed.reused, 1);
    assert!(fixture.query(&["before.nc"], true, false, false).is_empty());
    assert_eq!(fixture.query(&["after.nc"], true, false, false).len(), 1);
    fs::remove_dir_all(nested).unwrap();
    update(&fixture.idx, None, false).unwrap();
    assert!(fixture.query(&["after.nc"], true, false, false).is_empty());
    assert_eq!(fixture.query(&["*"], false, false, false).len(), 1);
}

#[test]
fn no_change_update_preserves_database_fingerprint() {
    let fixture = Fixture::new();
    fs::write(fixture.idx.root.join("file.nc"), b"").unwrap();
    update(&fixture.idx, None, false).unwrap();
    let before = fs::metadata(&fixture.idx.database).unwrap();
    update(&fixture.idx, None, false).unwrap();
    let after = fs::metadata(&fixture.idx.database).unwrap();
    assert_eq!(
        (before.len(), before.mtime(), before.mtime_nsec()),
        (after.len(), after.mtime(), after.mtime_nsec())
    );
}

#[test]
fn exclusions_rebuild_when_changed_and_do_not_follow_symlinks() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.idx.root.join("ignored dir")).unwrap();
    fs::write(fixture.idx.root.join("ignored dir/hidden.nc"), b"").unwrap();
    fs::write(fixture.idx.root.join("kept.nc"), b"").unwrap();
    std::os::unix::fs::symlink(&fixture.idx.root, fixture.idx.root.join("loop")).unwrap();
    fixture.idx.filters.exclude_dirs = vec!["ignored dir".into()];
    update(&fixture.idx, None, false).unwrap();
    assert!(fixture.query(&["hidden"], true, false, false).is_empty());
    fixture.idx.filters.exclude_dirs.clear();
    update(&fixture.idx, None, false).unwrap();
    assert_eq!(fixture.query(&["hidden"], true, false, false).len(), 1);
    assert_eq!(fixture.query(&["loop"], true, false, false).len(), 1);
    fs::create_dir(fixture.idx.root.join("ignored dir-other")).unwrap();
    fs::write(fixture.idx.root.join("ignored dir-other/kept.nc"), b"").unwrap();
    fixture.idx.filters.exclude_paths = vec![PathBuf::from("ignored dir")];
    update(&fixture.idx, None, false).unwrap();
    assert!(fixture.query(&["hidden"], true, false, false).is_empty());
    assert_eq!(fixture.query(&["kept.nc"], true, false, false).len(), 2);
}

#[test]
fn partial_update_preserves_other_subtrees_and_rollback_preserves_old_index() {
    let fixture = Fixture::new();
    for name in ["left", "right"] {
        fs::create_dir(fixture.idx.root.join(name)).unwrap();
        fs::write(fixture.idx.root.join(name).join("old.nc"), b"").unwrap();
    }
    update(&fixture.idx, None, false).unwrap();
    fs::write(fixture.idx.root.join("left/new.nc"), b"").unwrap();
    fs::write(fixture.idx.root.join("right/new.nc"), b"").unwrap();
    update(&fixture.idx, Some(&fixture.idx.root.join("left")), false).unwrap();
    assert_eq!(fixture.query(&["new.nc"], true, false, false).len(), 1);
    let before = fs::read(&fixture.idx.database).unwrap();
    assert!(update(&fixture.idx, Some(&fixture.idx.root.join("missing")), false).is_err());
    assert_eq!(fs::read(&fixture.idx.database).unwrap(), before);
    assert_eq!(fixture.query(&["old.nc"], true, false, false).len(), 2);
}

#[test]
fn symbolic_root_preserves_logical_paths_without_following_child_links() {
    let mut fixture = Fixture::new();
    fs::write(fixture.idx.root.join("file.nc"), b"").unwrap();
    let alias = fixture._workspace.join("alias");
    std::os::unix::fs::symlink(&fixture.idx.root, &alias).unwrap();
    fixture.idx.root = alias.clone();
    update(&fixture.idx, None, false).unwrap();
    assert_eq!(
        fixture.query(&["file.nc"], true, false, false),
        vec![alias.join("file.nc").as_os_str().as_bytes().to_vec()]
    );
    assert_eq!(update(&fixture.idx, None, false).unwrap().scanned, 0);
}

#[test]
fn malformed_queries_fail_and_limits_stop_visiting() {
    let fixture = Fixture::new();
    fs::write(fixture.idx.root.join("file.txt"), b"").unwrap();
    update(&fixture.idx, None, false).unwrap();
    assert!(
        Query::new(&SearchOptions {
            patterns: vec!["(".into()],
            regex: true,
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        Query::new(&SearchOptions {
            patterns: vec!["[bad".into()],
            ..Default::default()
        })
        .is_err()
    );
    let query = Query::new(&SearchOptions {
        patterns: vec!["*".into()],
        ..Default::default()
    })
    .unwrap();
    let mut count = 0;
    assert!(
        !visit(&fixture.idx, &query, |_| {
            count += 1;
            Ok(false)
        })
        .unwrap()
    );
    assert_eq!(count, 1);
}
