use nasfind::{
    config::{Config, Filters, IndexConfig, Tools},
    indexer, native,
    platform::path_bytes,
    search::{self, SearchOptions},
    stats,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    base: PathBuf,
    cfg: Config,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "nasfind-portable-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let root = base.join("files");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        for name in ["soil.nc", "SOIL.csv", "中文🌧.nc", "nested/rain.nc"] {
            fs::write(root.join(name), b"").unwrap();
        }
        let cfg = Config {
            tools: Tools::default(),
            index: vec![IndexConfig {
                name: "test".into(),
                root,
                database: base.join("index.db"),
                filters: Filters::default(),
            }],
        };
        indexer::build_indexes(&cfg, &[], &[], false, true).unwrap();
        Self { base, cfg }
    }

    fn paths(&self, options: SearchOptions) -> Vec<Vec<u8>> {
        let mut paths = Vec::new();
        search::visit_paths(&self.cfg, &options, |path| {
            paths.push(path.to_vec());
            Ok(())
        })
        .unwrap();
        paths
    }

    fn query(&self, pattern: &str) -> Vec<Vec<u8>> {
        self.paths(SearchOptions {
            patterns: vec![pattern.into()],
            basename: true,
            ..Default::default()
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn native_unicode_glob_regex_and_reuse() {
    let fixture = Fixture::new();
    assert_eq!(fixture.query("中文🌧.nc").len(), 1);
    assert_eq!(fixture.query("*.nc").len(), 3);
    assert_eq!(
        fixture
            .paths(SearchOptions {
                patterns: vec!["^soil[.]nc$".into()],
                basename: true,
                regex: true,
                ..Default::default()
            })
            .len(),
        1
    );
    let idx = &fixture.cfg.index[0];
    assert!(native::is_native(&idx.database).unwrap());
    assert_eq!(native::update(idx, None, false).unwrap().scanned, 0);
}

#[test]
fn scoped_updates_limits_and_offline_queries() {
    let fixture = Fixture::new();
    let idx = &fixture.cfg.index[0];
    fs::rename(
        idx.root.join("nested/rain.nc"),
        idx.root.join("nested/new.nc"),
    )
    .unwrap();
    fs::write(idx.root.join("later.nc"), b"").unwrap();
    indexer::build_indexes(&fixture.cfg, &[], &[idx.root.join("nested")], false, true).unwrap();
    assert!(fixture.query("rain.nc").is_empty());
    assert_eq!(fixture.query("new.nc").len(), 1);
    assert!(fixture.query("later.nc").is_empty());
    let all = fixture.query("*");
    let limited = fixture.paths(SearchOptions {
        patterns: vec!["*".into()],
        basename: true,
        offset: 1,
        limit: Some(2),
        ..Default::default()
    });
    assert_eq!(limited, all[1..3]);
    fs::rename(&idx.root, fixture.base.join("offline")).unwrap();
    assert_eq!(fixture.query("*"), all);
    assert!(native::update(idx, None, false).is_err());
}

#[test]
fn statistics_match_paths_and_survive_offline_roots() {
    let fixture = Fixture::new();
    let idx = &fixture.cfg.index[0];
    let mut n = 1; // The indexed root itself.
    native::directory_counts(idx, |_, count| {
        n += count;
        Ok(())
    })
    .unwrap();
    assert_eq!(n as usize, fixture.query("*").len());
    stats::refresh(&fixture.cfg, &[]).unwrap();
    let connection = rusqlite::Connection::open(fixture.base.join("stats.db")).unwrap();
    let total: i64 = connection
        .query_row(
            "SELECT total FROM selections WHERE key='[\"test\"]'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, n);
    let root: Vec<u8> = connection
        .query_row(
            "SELECT path FROM directories WHERE path=?",
            [path_bytes(&idx.root).as_ref()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(root, path_bytes(&idx.root).as_ref());
    drop(connection);
    fs::rename(&idx.root, fixture.base.join("offline")).unwrap();
    stats::refresh(&fixture.cfg, &[]).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_path_patterns_and_scopes() {
    let fixture = Fixture::new();
    let idx = &fixture.cfg.index[0];
    let paths = fixture.paths(SearchOptions {
        patterns: vec!["*".into()],
        path: Some(idx.root.join("nested")),
        ..Default::default()
    });
    assert_eq!(
        paths,
        [
            path_bytes(&idx.root.join("nested")).into_owned(),
            path_bytes(&idx.root.join("nested/rain.nc")).into_owned(),
        ]
    );
    let query = native::Query::new(&SearchOptions {
        patterns: vec![idx.root.join("nested").into_os_string()],
        ..Default::default()
    })
    .unwrap();
    let mut count = 0;
    native::visit(idx, &query, |_| {
        count += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(count, 2); // Directory record and its file.
    assert!(indexer::build_indexes(&fixture.cfg, &[], &[], false, false).is_err());
}
