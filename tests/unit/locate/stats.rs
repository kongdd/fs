use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use super::*;

fn options(recursive: bool, top: usize) -> StatsOptions {
    StatsOptions {
        path: None,
        top: NonZeroUsize::new(top).unwrap(),
        indexes: Vec::new(),
        recursive,
    }
}

fn ranked(mut counts: Counts, recursive: bool, top: usize, root: Option<&Path>) -> Vec<Rank> {
    counts.aggregate();
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&connection).unwrap();
    let cached = cache::store(&mut connection, "test", b"snapshot", counts).unwrap();
    cache::ranked(&connection, &cached, &options(recursive, top), root).unwrap()
}

fn reference(paths: &[Vec<u8>], recursive: bool) -> HashMap<Vec<u8>, i64> {
    let mut counts = HashMap::new();
    let mut seen = HashSet::new();
    for path in paths {
        if !seen.insert(path) {
            continue;
        }
        let mut parent = parent_path(path);
        while let Some(directory) = parent {
            if directory == path || (recursive && is_root(directory)) {
                break;
            }
            *counts.entry(directory.to_vec()).or_default() += 1;
            if !recursive {
                break;
            }
            parent = parent_path(directory);
        }
    }
    counts
}

#[test]
fn matches_reference_with_duplicate_and_byte_safe_paths() {
    let mut paths = vec![
        b"/".to_vec(),
        b"/a".to_vec(),
        b"//a".to_vec(),
        b"/a/new\nline".to_vec(),
        b"/a/invalid\xff/file".to_vec(),
        b"relative/file".to_vec(),
    ];
    let mut seed = 42_u64;
    for _ in 0..5000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let depth = seed % 9 + 1;
        let mut path = String::new();
        for _ in 0..depth {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            path.push_str(&format!("/{}", seed % 20));
        }
        paths.push(path.into_bytes());
    }
    paths.extend_from_within(..500);
    for recursive in [true, false] {
        let mut actual = Counts::new(true);
        for path in &paths {
            actual.add(path);
        }
        assert_eq!(
            actual.total,
            paths.iter().collect::<HashSet<_>>().len() as i64
        );
        let rows = ranked(actual, recursive, usize::MAX, Some(Path::new("/")));
        let actual: HashMap<_, _> = rows.into_iter().map(|row| (row.path, row.count)).collect();
        let mut expected = reference(&paths, recursive);
        expected.retain(|path, _| path.starts_with(b"/"));
        assert_eq!(actual, expected);
    }
}

#[test]
fn direct_counts_include_empty_directory_records_and_root_children() {
    let paths: &[&[u8]] = &[
        b"/",
        b"/tree",
        b"/tree/left",
        b"/tree/a",
        b"/tree/left/file",
        b"/tree/left/file",
        b"/tree/empty",
    ];
    let mut counts = Counts::new(true);
    for path in paths {
        counts.add(path);
    }
    assert_eq!(counts.total, 6);
    let rows = ranked(counts, false, 10, None);
    assert_eq!(
        rows.iter()
            .map(|row| (row.path.as_slice(), row.count))
            .collect::<Vec<_>>(),
        vec![
            (b"/tree".as_slice(), 3),
            (b"/".as_slice(), 1),
            (b"/tree/left".as_slice(), 1)
        ]
    );
}

#[test]
fn single_database_retains_only_directory_keys() {
    let mut counts = Counts::new(false);
    for number in 0..10000 {
        counts.add(format!("/missing/tree/file{number}").as_bytes());
    }
    assert!(counts.seen.is_none());
    assert_eq!(counts.total, 10000);
    assert_eq!(counts.directories.len(), 2);
    let rows = ranked(counts, true, 1, Some(Path::new("/missing/tree")));
    assert_eq!(rows[0].count, 10000);
}

#[test]
fn ranking_limits_results_and_sorts_equal_counts_by_path() {
    let mut counts = Counts::new(true);
    for path in [
        b"/tree/b/file".as_slice(),
        b"/tree/a/file",
        b"/tree/c/file",
        b"/tree-other/file",
    ] {
        counts.add(path);
    }
    let rows = ranked(counts, true, 2, Some(Path::new("/tree")));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, b"/tree");
    assert_eq!(rows[0].count, 3);
    assert_eq!(rows[1].path, b"/tree/a");
}

#[test]
fn subtree_queries_treat_special_and_invalid_utf8_bytes_literally() {
    let root = b"/data/a[*?]\\back\xff";
    let mut counts = Counts::new(true);
    for suffix in [b"/first".as_slice(), b"/next/file\nname"] {
        counts.add(&[root.as_slice(), suffix].concat());
    }
    counts.add(b"/data/another/file");
    let rows = ranked(counts, true, 10, Some(Path::new(OsStr::from_bytes(root))));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, root);
    assert_eq!(rows[0].count, 2);
}

#[test]
fn path_normalization_does_not_require_existing_directories() {
    assert_eq!(
        absolute_path(Path::new("/missing/tree/../other/./")).unwrap(),
        Path::new("/missing/other")
    );
    assert_eq!(
        absolute_path(Path::new("/../../missing")).unwrap(),
        Path::new("/missing")
    );
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("nasfind-stats-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn cache_hits_do_not_modify_sqlite_data() {
    let directory = TestDirectory::new();
    let path = directory.0.join("stats.db");
    let writer = rusqlite::Connection::open(&path).unwrap();
    cache::schema(&writer).unwrap();
    let observer = rusqlite::Connection::open(&path).unwrap();
    let before: i64 = observer
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    cache::schema(&writer).unwrap();
    let after: i64 = observer
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn source_database_and_filter_changes_invalidate_fingerprint() {
    use crate::config::{Filters, IndexConfig, Tools};
    let directory = TestDirectory::new();
    let database = directory.0.join("index.db");
    std::fs::write(&database, b"old source").unwrap();
    let mut cfg = Config {
        tools: Tools::default(),
        index: vec![IndexConfig {
            name: "test".into(),
            root: "/missing/index-root".into(),
            database: database.clone(),
            filters: Filters::default(),
        }],
    };
    let original = cache::fingerprint(&cfg, &[]).unwrap();
    std::fs::write(&database, b"changed source bytes").unwrap();
    let changed = cache::fingerprint(&cfg, &[]).unwrap();
    assert_eq!(original.0, changed.0);
    assert_ne!(original.1, changed.1);
    cfg.index[0].filters.exclude_extensions.push("tmp".into());
    assert_ne!(changed.1, cache::fingerprint(&cfg, &[]).unwrap().1);
}

#[test]
fn formats_counts_with_thousands_separators() {
    assert_eq!(separated(0), "0");
    assert_eq!(separated(999), "999");
    assert_eq!(separated(1234567), "1,234,567");
}

#[test]
fn failed_cache_replacement_leaves_previous_counts_intact() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&connection).unwrap();
    let mut first = Counts::new(false);
    first.add(b"/tree/file");
    first.aggregate();
    let cached = cache::store(&mut connection, "test", b"old", first).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_insert BEFORE INSERT ON counts
        BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    let mut replacement = Counts::new(false);
    replacement.add(b"/tree/a");
    replacement.add(b"/tree/b");
    replacement.aggregate();
    assert!(cache::store(&mut connection, "test", b"new", replacement).is_err());
    assert_eq!(
        cache::total(
            &connection,
            &cache::metadata(&connection, cached.id).unwrap(),
            None
        )
        .unwrap(),
        1
    );
    let rows = cache::ranked(
        &connection,
        &cached,
        &options(true, 10),
        Some(Path::new("/tree")),
    )
    .unwrap();
    assert_eq!(rows[0].count, 1);
}

#[test]
fn migrates_legacy_cache_without_requerying_source_databases() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("
        CREATE TABLE selections(id INTEGER PRIMARY KEY, key TEXT UNIQUE, snapshot BLOB, total INTEGER, largest INTEGER);
        CREATE TABLE counts(selection INTEGER, path BLOB, direct_count INTEGER, recursive_count INTEGER,
            PRIMARY KEY(selection, path)) WITHOUT ROWID;
        CREATE INDEX recursive_rank ON counts(selection, recursive_count DESC, path);
        CREATE INDEX direct_rank ON counts(selection, direct_count DESC, path);
        INSERT INTO selections VALUES(1, 'one', x'01', 3, 3), (2, 'two', x'01', 3, 3);
        INSERT INTO counts VALUES(1, x'2f74726565', 1, 3),
            (1, x'2f747265652f6c656674', 2, 2), (2, x'2f74726565', 1, 3);
        PRAGMA user_version=1;
    ").unwrap();
    cache::schema(&connection).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let copies: i64 = connection
        .query_row(
            "SELECT count(*) FROM directories WHERE path=?",
            [b"/tree".as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(copies, 1);
    let cached = cache::metadata(&connection, 1).unwrap();
    let rows = cache::ranked(
        &connection,
        &cached,
        &options(true, 10),
        Some(Path::new("/tree")),
    )
    .unwrap();
    assert_eq!(rows.iter().map(|row| row.count).collect::<Vec<_>>(), [3, 2]);
}

#[test]
fn shared_paths_are_stored_once_and_unused_paths_are_removed() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&connection).unwrap();
    for key in ["one", "two"] {
        let mut counts = Counts::new(false);
        counts.add(b"/tree/left/file");
        counts.aggregate();
        cache::store(&mut connection, key, b"snapshot", counts).unwrap();
    }
    let copies: i64 = connection
        .query_row(
            "SELECT count(*) FROM directories WHERE path=?",
            [b"/tree/left".as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(copies, 1);
    for key in ["one", "two"] {
        cache::store(&mut connection, key, b"empty", Counts::new(false)).unwrap();
    }
    let remaining: i64 = connection
        .query_row("SELECT count(*) FROM directories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn one_cache_stores_both_modes_and_scoped_totals() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&connection).unwrap();
    let mut counts = Counts::new(false);
    counts.add(b"/tree/left");
    counts.add(b"/tree/left/file");
    counts.aggregate();
    let cached = cache::store(&mut connection, "test", b"same", counts).unwrap();
    assert_eq!(
        cache::total(&connection, &cached, Some(Path::new("/tree"))).unwrap(),
        2
    );
    assert_eq!(
        cache::total(&connection, &cached, Some(Path::new("/missing"))).unwrap(),
        0
    );
    let recursive = cache::ranked(
        &connection,
        &cached,
        &options(true, 1),
        Some(Path::new("/tree")),
    )
    .unwrap();
    let direct = cache::ranked(
        &connection,
        &cached,
        &options(false, 1),
        Some(Path::new("/tree")),
    )
    .unwrap();
    assert_eq!(recursive[0].count, 2);
    assert_eq!(direct[0].count, 1);
}
