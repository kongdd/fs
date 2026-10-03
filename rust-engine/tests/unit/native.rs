use super::*;
use crate::config::Filters;
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    fs::MetadataExt,
};

struct Fixture {
    _workspace: PathBuf,
    idx: IndexConfig,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    // APFS rejects non-UTF-8 names; the codec test below still covers raw bytes.
    let invalid = if cfg!(target_os = "macos") {
        std::ffi::OsString::from("bad_apfs.nc")
    } else {
        std::ffi::OsString::from_vec(b"bad_\xff.nc".to_vec())
    };
    fs::write(fixture.idx.root.join(invalid), b"").unwrap();
    update(&fixture.idx, None, false).unwrap();
    let mut all: Vec<_> = fs::read_dir(&fixture.idx.root)
        .unwrap()
        .map(|entry| entry.unwrap().path().as_os_str().as_bytes().to_vec())
        .collect();
    all.push(fixture.idx.root.as_os_str().as_bytes().to_vec());
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
fn bulk_build_creates_complete_indexes_and_deduplicated_postings() {
    let fixture = Fixture::new();
    for name in ["zzzzzz.nc", "ZZZzzz.txt", "aaaaaaa.dat"] {
        fs::write(fixture.idx.root.join(name), b"").unwrap();
    }
    update(&fixture.idx, None, false).unwrap();
    let zzz = trigram_keys(b"zzz").next().unwrap();
    for removed in [false, true] {
        if removed {
            fs::remove_file(fixture.idx.root.join("zzzzzz.nc")).unwrap();
            update(&fixture.idx, None, false).unwrap();
        }
        let connection = open_read(&fixture.idx.database).unwrap();
        let indexed: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='directory_blocks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        let frequency: i64 = connection
            .query_row("SELECT n FROM postings WHERE gram=?", [zzz], |r| r.get(0))
            .unwrap();
        assert_eq!(frequency, 1); // Repeated/folded grams counted once per block.
        let mut statement = connection.prepare("SELECT data,n FROM postings").unwrap();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            assert_eq!(
                unpack(row.get_ref(0).unwrap().as_blob().unwrap())
                    .unwrap()
                    .len(),
                row.get::<_, i64>(1).unwrap() as usize
            );
        }
    }
    assert_eq!(fixture.query(&["zzz"], true, true, false).len(), 1);
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

#[test]
fn blocks_cover_boundaries_and_never_combine_different_filenames() {
    let fixture = Fixture::new();
    for number in 0..65 {
        fs::write(fixture.idx.root.join(format!("entry_{number:03}.nc")), b"").unwrap();
    }
    fs::write(fixture.idx.root.join("soil.txt"), b"").unwrap();
    fs::write(fixture.idx.root.join("rain.csv"), b"").unwrap();
    assert_eq!(update(&fixture.idx, None, false).unwrap().entries, 68);
    let connection = open_read(&fixture.idx.database).unwrap();
    let blocks: i64 = connection
        .query_row("SELECT count(*) FROM blocks WHERE directory!=0", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(blocks, 3);
    drop(connection);
    assert!(
        fixture
            .query(&["soil", "csv"], true, false, false)
            .is_empty()
    );
    for number in [0, 31, 32, 63, 64] {
        let name = format!("entry_{number:03}.nc");
        assert_eq!(fixture.query(&[&name], true, false, false).len(), 1);
        assert_eq!(
            fixture
                .query(&[&format!("files/{name}")], false, false, false)
                .len(),
            1
        );
    }
    fs::remove_file(fixture.idx.root.join("entry_032.nc")).unwrap();
    fs::rename(
        fixture.idx.root.join("entry_064.nc"),
        fixture.idx.root.join("renamed.nc"),
    )
    .unwrap();
    update(&fixture.idx, None, false).unwrap();
    assert!(fixture.query(&["entry_032"], true, false, false).is_empty());
    assert!(fixture.query(&["entry_064"], true, false, false).is_empty());
    assert_eq!(fixture.query(&["renamed"], true, false, false).len(), 1);
    assert_eq!(fixture.query(&["entry"], true, false, false).len(), 63);
}

#[test]
fn block_codec_preserves_raw_bytes_and_rejects_corruption() {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO directories(path,stamp) VALUES (?,?)",
            params![b"/root", b""],
        )
        .unwrap();
    let mut encoder = BlockEncoder::new().unwrap();
    let mut postings = HashMap::new();
    let data = b"bad_\xff.nc\0line\nfile.txt\0";
    write_block(
        &connection,
        1,
        data,
        2,
        b"/root/",
        &mut postings,
        &mut encoder,
    )
    .unwrap();
    let query = Query::new(&SearchOptions {
        patterns: vec!["*".into()],
        ..Default::default()
    })
    .unwrap();
    let mut reader = BlockReader {
        prefixes: HashMap::new(),
        buffer: Vec::new(),
        decoded: Vec::new(),
        decompressor: zstd::bulk::Decompressor::new().unwrap(),
    };
    let mut read = || {
        let mut found = Vec::new();
        let mut statement = connection
            .prepare("SELECT data,directory,size,n FROM blocks")
            .unwrap();
        let mut rows = statement.query([]).unwrap();
        reader.visit(
            rows.next().unwrap().unwrap(),
            &connection,
            &query,
            &mut |path| {
                found.push(path.to_vec());
                Ok(true)
            },
        )?;
        Ok::<_, anyhow::Error>(found)
    };
    assert_eq!(
        read().unwrap(),
        vec![
            b"/root/bad_\xff.nc".to_vec(),
            b"/root/line\nfile.txt".to_vec()
        ]
    );
    for sql in [
        "UPDATE blocks SET n=3",
        "UPDATE blocks SET n=2,size=-1",
        "UPDATE blocks SET size=4194305",
        "UPDATE blocks SET size=1",
        "UPDATE blocks SET size=25,data=X'ff'",
    ] {
        connection.execute_batch(sql).unwrap();
        assert!(read().is_err(), "{sql}");
    }
}

#[test]
fn posting_batch_flushes_and_merges_sorted_unique_ids() {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    let mut batch = PostingBatch::default();
    batch
        .add(
            &connection,
            1,
            HashMap::from([(123, (1..=262_144).collect())]),
        )
        .unwrap();
    assert_eq!(batch.count, 0);
    batch
        .add(&connection, 2, HashMap::from([(123, vec![262_145])]))
        .unwrap();
    batch.flush(&connection).unwrap();
    let (data, n): (Vec<u8>, i64) = connection
        .query_row("SELECT data,n FROM postings WHERE gram=123", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(n, 262_145);
    assert_eq!(unpack(&data).unwrap(), (1..=262_145).collect::<Vec<_>>());
}

#[test]
fn fresh_posting_table_keeps_slots_stable_across_growth() {
    let mut table = FreshPostingTable::default();
    let keys = [0, 1, 255, 256, 257, 65535, 65536, 0xffff00, 0xffffff];
    for (index, gram) in keys.into_iter().enumerate() {
        assert!(table.get(gram).is_none());
        table.get_or_insert(gram).last = index as u64 + 1;
    }
    // Grow the entry vector without moving any slot's logical index.
    for gram in 0x120000..0x120200 {
        table.get_or_insert(gram).last = u64::from(gram);
    }
    for (index, gram) in keys.into_iter().enumerate() {
        assert_eq!(table.get(gram).unwrap().last, index as u64 + 1);
        table.get_or_insert(gram).count += 1;
        assert_eq!(table.get(gram).unwrap().count, 1);
    }
    for gram in 0x120000..0x120200 {
        assert_eq!(table.get(gram).unwrap().last, u64::from(gram));
    }
    assert_eq!(table.entries.len(), keys.len() + 512);
    assert!(table.get(2).is_none()); // Empty slot in an allocated page.
    assert!(table.get(0x800000).is_none()); // Unallocated page.
    assert!(table.get(1 << 24).is_none());
}

#[test]
fn block_encoder_reuses_buffers_without_retaining_previous_bytes() {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    let mut encoder = BlockEncoder::new().unwrap();
    let mut large = vec![b'A'; 4096];
    large.push(0);
    let mut compressed_capacity = 0;
    let mut suffix_capacity = 0;
    for (index, data) in [large.as_slice(), b"x\0", b"bad_\xff\0line\nname\0"]
        .into_iter()
        .enumerate()
    {
        let mut postings = HashMap::new();
        let n = data.iter().filter(|&&byte| byte == 0).count();
        write_block(&connection, 0, data, n, b"", &mut postings, &mut encoder).unwrap();
        let compressed: Vec<u8> = connection
            .query_row(
                "SELECT data FROM blocks WHERE id=?",
                [connection.last_insert_rowid()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            zstd::bulk::decompress(&compressed, data.len()).unwrap(),
            data
        );
        let expected: HashSet<_> = data[..data.len() - 1]
            .split(|&byte| byte == 0)
            .flat_map(trigram_keys)
            .collect();
        assert_eq!(postings.keys().copied().collect::<HashSet<_>>(), expected);
        if index == 0 {
            compressed_capacity = encoder.compressed.capacity();
            suffix_capacity = encoder.suffix.capacity();
        }
        assert_eq!(encoder.compressed.capacity(), compressed_capacity);
        assert_eq!(encoder.suffix.capacity(), suffix_capacity);
    }
}

#[test]
fn fresh_postings_encode_once_and_publish_only_at_finish() {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    let mut fresh = FreshPostings::default();
    fresh
        .add(&connection, HashMap::from([(123, vec![1, 127, 128])]))
        .unwrap();
    fresh
        .add(
            &connection,
            HashMap::from([(123, vec![99999]), (456, vec![200000])]),
        )
        .unwrap();
    assert_eq!(fresh.segments, 0);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM postings", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    fresh.finish(&connection).unwrap();
    for (gram, ids) in [(123, vec![1, 127, 128, 99999]), (456, vec![200000])] {
        let (data, n): (Vec<u8>, i64) = connection
            .query_row("SELECT data,n FROM postings WHERE gram=?", [gram], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(data, pack(&ids));
        assert_eq!(n as usize, ids.len());
    }
}

#[test]
fn fresh_posting_spills_concatenate_delta_streams_without_reencoding() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA temp_store=FILE;").unwrap();
    schema(&connection).unwrap();
    let mut fresh = FreshPostings {
        byte_limit: 4,
        ..Default::default()
    };
    fresh
        .add(
            &connection,
            HashMap::from([(123, vec![1, 127, 128, 99999])]),
        )
        .unwrap();
    assert_eq!(fresh.segments, 1);
    assert_eq!(fresh.buffered_bytes, 0);
    fresh
        .add(
            &connection,
            HashMap::from([(123, vec![u32::MAX as u64]), (456, vec![10, 20])]),
        )
        .unwrap();
    assert_eq!(fresh.segments, 2);
    fresh
        .add(&connection, HashMap::from([(123, vec![u64::MAX])]))
        .unwrap();
    assert_eq!(fresh.segments, 3);
    // Leave an unspilled tail and a gram that was absent from the middle spill.
    fresh
        .add(&connection, HashMap::from([(456, vec![21])]))
        .unwrap();
    assert_eq!(fresh.buffered_bytes, 1);
    fresh.finish(&connection).unwrap();
    for (gram, ids) in [
        (123, vec![1, 127, 128, 99999, u32::MAX as u64, u64::MAX]),
        (456, vec![10, 20, 21]),
    ] {
        let (data, n): (Vec<u8>, i64) = connection
            .query_row("SELECT data,n FROM postings WHERE gram=?", [gram], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(data, pack(&ids));
        assert_eq!(unpack(&data).unwrap(), ids);
        assert_eq!(n as usize, ids.len());
    }
    let temporary: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_temp_master WHERE name='fresh_posting_segments'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(temporary, 0);
}

#[test]
fn fresh_directory_stream_matches_full_path_grams_across_spills() {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    let mut writer = PostingWriter::Fresh(FreshPostings {
        byte_limit: 1,
        ..Default::default()
    });
    let mut encoder = BlockEncoder::new().unwrap();
    let mut oracle: HashMap<u32, Vec<u64>> = HashMap::new();
    for directory in 1..=3 {
        let prefix = format!("/root/shared_{directory}/").into_bytes();
        let common = grams(&prefix);
        let mut postings = writer.directory(&common);
        let mut directory_keys = HashSet::new();
        // The last directory is empty; it must still have a reverse-gram row.
        if directory != 3 {
            for data in [
                b"shared_shared_abc\0bad_\xff.nc\0".as_slice(),
                b"shared_abc\0xyz\0".as_slice(),
            ] {
                write_block(
                    &connection,
                    directory,
                    data,
                    2,
                    &prefix,
                    &mut postings,
                    &mut encoder,
                )
                .unwrap();
                let id = connection.last_insert_rowid() as u64;
                let mut block_keys = HashSet::new();
                for name in data[..data.len() - 1].split(|&byte| byte == 0) {
                    let mut path = prefix.clone();
                    path.extend_from_slice(name);
                    block_keys.extend(trigram_keys(&path));
                }
                for gram in block_keys {
                    directory_keys.insert(u64::from(gram));
                    oracle.entry(gram).or_default().push(id);
                }
            }
        }
        postings.finish(&connection, directory).unwrap();
        let data: Vec<u8> = connection
            .query_row(
                "SELECT data FROM directory_grams WHERE directory=?",
                [directory],
                |row| row.get(0),
            )
            .unwrap();
        let mut expected: Vec<_> = directory_keys.into_iter().collect();
        expected.sort_unstable();
        assert_eq!(unpack(&data).unwrap(), expected);
    }
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM postings", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    writer.flush(&connection).unwrap();
    let mut select = connection
        .prepare("SELECT gram,data,n FROM postings")
        .unwrap();
    let mut rows = select.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let gram: u32 = row.get(0).unwrap();
        let expected = oracle.remove(&gram).unwrap();
        let data: Vec<u8> = row.get(1).unwrap();
        assert_eq!(data, pack(&expected));
        assert_eq!(row.get::<_, i64>(2).unwrap() as usize, expected.len());
    }
    assert!(oracle.is_empty());
}

#[test]
fn fresh_postings_reject_nonmonotonic_ids_even_after_spilling() {
    for limit in [1, usize::MAX] {
        for invalid in [9, 10] {
            let connection = Connection::open_in_memory().unwrap();
            schema(&connection).unwrap();
            let mut fresh = FreshPostings {
                byte_limit: limit,
                ..Default::default()
            };
            fresh
                .add(&connection, HashMap::from([(123, vec![10])]))
                .unwrap();
            assert!(
                fresh
                    .add(&connection, HashMap::from([(123, vec![invalid])]))
                    .is_err()
            );
        }
    }
}

#[test]
fn fresh_posting_spills_and_final_rows_roll_back_together() {
    let mut connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    {
        let transaction = connection.transaction().unwrap();
        let mut fresh = FreshPostings {
            byte_limit: 1,
            ..Default::default()
        };
        fresh
            .add(&transaction, HashMap::from([(123, vec![1, 2])]))
            .unwrap();
        fresh.finish(&transaction).unwrap();
        // Drop the transaction without commit, as on a scan failure.
    }
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM postings", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM sqlite_temp_master", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn fresh_build_bulk_directory_index_preserves_unique_paths() {
    let fixture = Fixture::new();
    update(&fixture.idx, None, false).unwrap();
    let connection = Connection::open(&fixture.idx.database).unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO directories(path,stamp) VALUES (?,?)",
                params![fixture.idx.root.as_os_str().as_bytes(), b""]
            )
            .is_err()
    );
    let indexed: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='directory_paths'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 1);
}

#[test]
fn fresh_scanner_preserves_order_filters_and_symlink_rules() {
    let mut fixture = Fixture::new();
    for name in ["a", "b", "z", "z/child", "hidden", "blocked"] {
        fs::create_dir_all(fixture.idx.root.join(name)).unwrap();
    }
    std::os::unix::fs::symlink(&fixture.idx.root, fixture.idx.root.join("loop")).unwrap();
    fixture.idx.filters.exclude_dirs = vec!["hidden".into()];
    fixture.idx.filters.exclude_paths = vec![PathBuf::from("blocked")];
    let mut scanner = scan::Scanner::new(
        &fixture.idx.root,
        &fixture.idx.root,
        ScanRules::new(&fixture.idx),
    )
    .unwrap();
    let mut paths = Vec::new();
    while let Some(directory) = scanner.next().unwrap() {
        assert_eq!(
            directory.before,
            scan::read_stamp(&directory.path, &fixture.idx.root).unwrap()
        );
        assert!(
            directory
                .entries
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0)
        );
        paths.push(
            directory
                .path
                .strip_prefix(&fixture.idx.root)
                .unwrap()
                .to_path_buf(),
        );
    }
    assert_eq!(paths, ["", "z", "z/child", "b", "a"].map(PathBuf::from));
    // The writer applies the same filters to stored records as the producer
    // applies to traversal. A symlink remains a record, never a traversal child.
    let report = update(&fixture.idx, None, false).unwrap();
    assert_eq!(report.scanned, 5);
    assert!(fixture.query(&["hidden"], false, false, false).is_empty());
    assert!(fixture.query(&["blocked"], false, false, false).is_empty());
    assert_eq!(fixture.query(&["loop"], true, false, false).len(), 1);
}

#[test]
fn fresh_scanner_batches_large_directories_and_cancels_cleanly() {
    let fixture = Fixture::new();
    let mut expected = Vec::new();
    for i in (0..600).rev() {
        let name = format!("entry_{i:04}_{}", "x".repeat(220));
        fs::write(fixture.idx.root.join(&name), b"").unwrap();
        expected.push(std::ffi::OsString::from(name));
    }
    expected.sort();
    let mut scanner = scan::Scanner::new(
        &fixture.idx.root,
        &fixture.idx.root,
        ScanRules::new(&fixture.idx),
    )
    .unwrap();
    let directory = scanner.next().unwrap().unwrap();
    assert_eq!(
        directory
            .entries
            .into_iter()
            .map(|(name, is_dir)| {
                assert!(!is_dir);
                name
            })
            .collect::<Vec<_>>(),
        expected
    );
    assert!(scanner.next().unwrap().is_none());
    // Error/early-return cleanup must disconnect before joining the worker.
    drop(
        scan::Scanner::new(
            &fixture.idx.root,
            &fixture.idx.root,
            ScanRules::new(&fixture.idx),
        )
        .unwrap(),
    );
    let mut missing = scan::Scanner::new(
        &fixture.idx.root,
        &fixture.idx.root.join("missing"),
        ScanRules::new(&fixture.idx),
    )
    .unwrap();
    assert!(missing.next().is_err());
}

#[test]
fn old_schema_is_rejected_without_overwriting() {
    let fixture = Fixture::new();
    update(&fixture.idx, None, false).unwrap();
    Connection::open(&fixture.idx.database)
        .unwrap()
        .execute_batch("PRAGMA user_version=1")
        .unwrap();
    let before = fs::read(&fixture.idx.database).unwrap();
    assert!(is_native(&fixture.idx.database).is_err());
    assert!(update(&fixture.idx, None, false).is_err());
    assert_eq!(fs::read(&fixture.idx.database).unwrap(), before);
}
