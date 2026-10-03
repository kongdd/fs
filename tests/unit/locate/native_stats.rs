use super::*;
use crate::config::{Filters, IndexConfig, Tools};
use fs_updatedb::native as writer;
use std::{ffi::OsString, fs, os::unix::ffi::OsStringExt};

struct Fixture {
    base: PathBuf,
    cfg: Config,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = env::temp_dir().join(format!("native-stats-{}-{nonce}", std::process::id()));
        let root = base.join("files");
        fs::create_dir_all(root.join("nested/deep")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        fs::create_dir(root.join("hidden")).unwrap();
        for name in [
            "a.nc",
            "b.tmp",
            "nested/c.csv",
            "nested/deep/line\nname",
            "hidden/skip.nc",
        ] {
            fs::write(root.join(name), b"").unwrap();
        }
        let raw = if cfg!(target_os = "macos") {
            OsString::from("bad.nc")
        } else {
            OsString::from_vec(b"bad_\xff.nc".to_vec())
        };
        fs::write(root.join(raw), b"").unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let cfg = Config {
            tools: Tools::default(),
            index: vec![IndexConfig {
                name: "native".into(),
                root,
                database: base.join("index.db"),
                filters: Filters {
                    exclude_dirs: vec!["hidden".into()],
                    ..Default::default()
                },
            }],
        };
        writer::update(&cfg.index[0], None, false).unwrap();
        Self { base, cfg }
    }

    fn reference(&self) -> Counts {
        let mut counts = Counts::new(self.cfg.index.len() > 1);
        search::visit_paths(
            &self.cfg,
            &SearchOptions {
                patterns: vec!["/".into()],
                ..Default::default()
            },
            |path| {
                counts.add(path);
                Ok(())
            },
        )
        .unwrap();
        counts.aggregate();
        counts
    }

    fn check(&self) {
        let actual = collect(&self.cfg, &[]).unwrap();
        let expected = self.reference();
        assert_eq!(actual.total, expected.total);
        assert_eq!(actual.directories.len(), expected.directories.len());
        for (path, &id) in &expected.directories {
            let node = &actual.nodes[actual.directories[path]];
            assert_eq!(
                (node.direct, node.recursive),
                (expected.nodes[id].direct, expected.nodes[id].recursive),
                "{path:?}"
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).ok();
    }
}

#[test]
fn metadata_counts_match_paths_across_updates_and_offline_roots() {
    let fixture = Fixture::new();
    fixture.check();
    let root = &fixture.cfg.index[0].root;
    fs::rename(root.join("a.nc"), root.join("nested/new.nc")).unwrap();
    fs::remove_dir_all(root.join("nested/deep")).unwrap();
    writer::update(&fixture.cfg.index[0], None, false).unwrap();
    fixture.check();
    fs::rename(root, fixture.base.join("offline")).unwrap();
    fixture.check();
}

#[test]
fn metadata_counts_cover_empty_and_symbolic_roots() {
    let mut fixture = Fixture::new();
    let root = fixture.cfg.index[0].root.clone();
    fs::remove_dir_all(&root).unwrap();
    fs::create_dir(&root).unwrap();
    writer::update(&fixture.cfg.index[0], None, false).unwrap();
    fixture.check();
    assert_eq!(collect(&fixture.cfg, &[]).unwrap().total, 1);
    let alias = fixture.base.join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    fs::write(root.join("file.nc"), b"").unwrap();
    fixture.cfg.index[0].root = alias;
    fixture.cfg.index[0].database = fixture.base.join("alias.db");
    writer::update(&fixture.cfg.index[0], None, false).unwrap();
    fixture.check();
    assert_eq!(collect(&fixture.cfg, &[]).unwrap().total, 2);
}

#[test]
fn file_filters_and_overlapping_indexes_keep_per_path_fallback() {
    let mut fixture = Fixture::new();
    fixture.cfg.index[0].filters.exclude_extensions = vec!["tmp".into()];
    fixture.check();
    fixture.cfg.index[0].filters.exclude_files = vec!["C.CSV".into()];
    fixture.check();
    fixture.cfg.index[0].filters.exclude_extensions.clear();
    fixture.cfg.index[0].filters.exclude_files.clear();
    let mut duplicate = fixture.cfg.index[0].clone();
    duplicate.name = "duplicate".into();
    duplicate.database = fixture.base.join("duplicate.db");
    fs::copy(&fixture.cfg.index[0].database, &duplicate.database).unwrap();
    fixture.cfg.index.push(duplicate);
    fixture.check();
}

#[test]
fn counted_children_preserve_root_and_trailing_slash_semantics() {
    let mut actual = Counts::new(false);
    let mut expected = Counts::new(false);
    for directory in [
        b"/".as_slice(),
        b"//",
        b"/tree",
        b"/tree/",
        b"/tree///",
        b"/tree/empty",
    ] {
        let n = if directory.ends_with(b"empty") { 0 } else { 3 };
        actual.add_children(directory, n);
        for i in 0..n {
            let mut path = directory.to_vec();
            if !path.ends_with(b"/") {
                path.push(b'/');
            }
            path.extend_from_slice(format!("name{i}").as_bytes());
            expected.add(&path);
        }
    }
    actual.aggregate();
    expected.aggregate();
    assert_eq!(actual.total, expected.total);
    assert_eq!(actual.directories.len(), expected.directories.len());
    for (path, &id) in &expected.directories {
        let node = &actual.nodes[actual.directories[path]];
        assert_eq!(
            (node.direct, node.recursive),
            (expected.nodes[id].direct, expected.nodes[id].recursive)
        );
    }
}

#[test]
fn invalid_count_metadata_does_not_publish_stats_cache() {
    let fixture = Fixture::new();
    let backup = fixture.base.join("valid.db");
    fs::copy(&fixture.cfg.index[0].database, &backup).unwrap();
    for sql in [
        "UPDATE blocks SET n=0 WHERE directory<>0",
        "UPDATE blocks SET n=33 WHERE directory<>0",
        "UPDATE blocks SET size=-1 WHERE directory<>0",
        "UPDATE blocks SET directory=999999 WHERE directory<>0",
        "DELETE FROM blocks WHERE directory=0",
        "UPDATE blocks SET data=X'00' WHERE directory=0",
    ] {
        fs::copy(&backup, &fixture.cfg.index[0].database).unwrap();
        let connection = rusqlite::Connection::open(&fixture.cfg.index[0].database).unwrap();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        assert!(collect(&fixture.cfg, &[]).is_err(), "{sql}");
        assert!(refresh(&fixture.cfg, &[]).is_err(), "{sql}");
        let cache = cache::open(&fixture.cfg).unwrap();
        assert_eq!(
            cache
                .query_row("SELECT count(*) FROM selections", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(cache);
    }
}
