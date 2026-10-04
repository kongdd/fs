use super::*;
use rusqlite::params;
use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    populate(&connection);
    connection
}

fn populate(connection: &Connection) {
    connection.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value BLOB); INSERT INTO meta VALUES('root',X'2f726f6f74'); CREATE TABLE blocks(id INTEGER PRIMARY KEY,data BLOB,directory INTEGER,size INTEGER,n INTEGER); CREATE TABLE directories(id INTEGER PRIMARY KEY,path BLOB); INSERT INTO directories VALUES(1,X'');").unwrap();
    connection
        .execute(
            "INSERT INTO blocks VALUES(0,?,0,1,1)",
            [zstd::bulk::compress(b"\0", 1).unwrap()],
        )
        .unwrap();
    for number in 1..=900 {
        let mut data = format!("py_{number:04}.txt\0").into_bytes();
        data.extend_from_slice(b"bad_\xff.nc\0line\nfile.txt\0");
        connection
            .execute(
                "INSERT INTO blocks VALUES(?,?,1,?,3)",
                params![
                    number,
                    zstd::bulk::compress(&data, 1).unwrap(),
                    data.len() as i64
                ],
            )
            .unwrap();
    }
}

fn is_py(name: &[u8]) -> bool {
    name.starts_with(b"py_")
}

fn run(
    connection: &Connection,
    workers: usize,
    matches: impl Fn(&[u8]) -> bool + Sync,
    mut visitor: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    parallel(
        connection,
        true,
        (&matches, &|_| true),
        &mut visitor,
        workers,
    )
}

fn collect(connection: &Connection, basename: bool, workers: usize, filter: bool) -> Vec<Vec<u8>> {
    let matches = |path: &[u8]| path.windows(3).any(|w| w == b"py_") || !basename;
    let name_matches = |name: &[u8]| !filter || is_py(name);
    let mut found = Vec::new();
    let mut visitor = |path: &[u8]| {
        found.push(path.to_vec());
        Ok(true)
    };
    let complete = parallel(
        connection,
        basename,
        (&matches, &name_matches),
        &mut visitor,
        workers,
    )
    .unwrap();
    assert!(complete);
    found
}

#[test]
fn parallel_matches_serial_in_order_with_raw_bytes_and_path_predicates() {
    let connection = fixture();
    for basename in [false, true] {
        for filter in [false, true] {
            let expected = collect(&connection, basename, 1, filter);
            for workers in [0, 2, 4, 7, usize::MAX] {
                assert_eq!(collect(&connection, basename, workers, filter), expected);
            }
        }
    }
    let paths = collect(&connection, false, 4, false);
    for suffix in [b"bad_\xff.nc".as_slice(), b"line\nfile.txt"] {
        assert!(paths.iter().any(|p| p.ends_with(suffix)));
    }
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for (workers, found) in [(1, &mut expected), (4, &mut actual)] {
        let matches = |p: &[u8]| p.ends_with(b"bad_\xff.nc");
        let mut visitor = |p: &[u8]| {
            found.push(p.to_vec());
            Ok(true)
        };
        parallel(
            &connection,
            false,
            (&matches, &|_| true),
            &mut visitor,
            workers,
        )
        .unwrap();
    }
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 900);
}

#[test]
fn relative_directories_and_root_marker_produce_full_paths() {
    let connection = fixture();
    let found = collect(&connection, false, 4, false);
    assert_eq!(found[0], b"/root");
    assert!(found[1..].iter().all(|path| path.starts_with(b"/root/")));
    for bad in [b"/outside".as_slice(), b"../outside", b"bad\0path"] {
        connection
            .execute("UPDATE directories SET path=?", [bad])
            .unwrap();
        assert!(run(&connection, 4, is_py, |_| Ok(true)).is_err());
    }
}

#[test]
fn invalid_directory_paths_fail_only_when_reached() {
    for prefix in [b"../outside".as_slice(), b"/bad\0prefix"] {
        let connection = fixture();
        connection
            .execute("INSERT INTO directories VALUES(2,?)", [prefix])
            .unwrap();
        connection
            .execute("UPDATE blocks SET directory=2 WHERE id>=600", [])
            .unwrap();
        for workers in [1, 4] {
            let error = run(&connection, workers, is_py, |_| Ok(true)).unwrap_err();
            assert!(error.to_string().contains("invalid directory path"));
            let mut count = 0;
            let complete = run(&connection, workers, is_py, |_| {
                count += 1;
                Ok(count < 513)
            })
            .unwrap();
            assert!(!complete);
            assert_eq!(count, 513);
            assert!(run(&connection, workers, |_| false, |_| unreachable!()).unwrap());
        }
    }
}

#[test]
fn multiple_worker_panics_return_an_error_after_all_joins() {
    let connection = fixture();
    let barrier = std::sync::Barrier::new(2);
    let matches = |name: &[u8]| {
        if is_py(name) && name >= b"py_0512".as_slice() {
            barrier.wait();
            panic!("test worker failure");
        }
        true
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&connection, 2, matches, |_| Ok(true))
    }));
    assert!(result.is_ok(), "worker panic escaped the Result API");
    let error = result.unwrap().unwrap_err();
    assert!(error.to_string().contains("scan worker panicked"));
}

#[test]
fn large_blocks_cross_byte_budget_without_losing_records() {
    let connection = fixture();
    let mut data = b"py_".to_vec();
    data.extend(std::iter::repeat_n(b'x', BATCH_BYTES + 1));
    data.push(0);
    let encoded = zstd::bulk::compress(&data, 1).unwrap();
    for id in [600, 601, 750] {
        connection
            .execute(
                "UPDATE blocks SET data=?,size=?,n=1 WHERE id=?",
                params![encoded, data.len() as i64, id],
            )
            .unwrap();
    }
    assert_eq!(
        collect(&connection, true, 7, true),
        collect(&connection, true, 1, true)
    );
    connection
        .execute("UPDATE blocks SET data=zeroblob(5000000) WHERE id=650", [])
        .unwrap();
    let error = run(&connection, 7, |_| false, |_| unreachable!()).unwrap_err();
    assert!(error.to_string().contains("excessive compressed size"));
}

#[test]
fn slower_first_batch_does_not_reorder_results() {
    let connection = fixture();
    let matches = |name: &[u8]| {
        if name.starts_with(b"py_0512") {
            std::thread::sleep(Duration::from_millis(25));
        }
        is_py(name)
    };
    let mut actual = Vec::new();
    run(&connection, 4, matches, |p| {
        actual.push(p.to_vec());
        Ok(true)
    })
    .unwrap();
    assert_eq!(actual, collect(&connection, true, 1, true));
}

#[test]
fn cancellation_and_visitor_errors_disconnect_before_joining() {
    let connection = fixture();
    for limit in [1, 512, 513, 700, 901] {
        let count = Cell::new(0);
        let complete = run(&connection, 7, is_py, |_| {
            count.set(count.get() + 1);
            Ok(count.get() < limit)
        })
        .unwrap();
        assert_eq!(count.get(), limit.min(900));
        assert_eq!(complete, limit > 900);
    }
    let count = Cell::new(0);
    let error = run(&connection, 4, is_py, |_| {
        count.set(count.get() + 1);
        if count.get() == 513 {
            bail!("output failed");
        }
        Ok(true)
    })
    .unwrap_err();
    assert!(error.to_string().contains("output failed"));
}

#[test]
fn workers_reject_corruption_even_when_predicates_never_match() {
    let connection = fixture();
    let (data, size): (Vec<u8>, i64) = connection
        .query_row("SELECT data,size FROM blocks WHERE id=600", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    for sql in [
        "UPDATE blocks SET n=4 WHERE id=600",
        "UPDATE blocks SET n=3,size=-1 WHERE id=600",
        "UPDATE blocks SET size=4194305 WHERE id=600",
        "UPDATE blocks SET size=40,data=X'ff' WHERE id=600",
        "UPDATE blocks SET directory=0,n=1 WHERE id=600",
    ] {
        connection
            .execute(
                "UPDATE blocks SET directory=1,n=3,size=?,data=? WHERE id=600",
                params![size, data],
            )
            .unwrap();
        connection.execute_batch(sql).unwrap();
        let result = parallel(
            &connection,
            true,
            (&|_| false, &|_| false),
            &mut |_| unreachable!(),
            4,
        );
        assert!(result.is_err(), "{sql}");
    }
    let data = b"illegal/slash\0";
    connection
        .execute(
            "UPDATE blocks SET directory=1,data=?,size=?,n=1 WHERE id=600",
            params![zstd::bulk::compress(data, 1).unwrap(), data.len() as i64],
        )
        .unwrap();
    assert!(run(&connection, 4, |_| false, |_| unreachable!()).is_err());
}

#[test]
fn early_limit_does_not_surface_errors_from_read_ahead() {
    for sql in [
        "UPDATE blocks SET n=-1 WHERE id=550",
        "UPDATE blocks SET n=4 WHERE id=550",
        "UPDATE blocks SET data=X'ff' WHERE id=550",
        "UPDATE blocks SET size=4194305 WHERE id=650",
    ] {
        let connection = fixture();
        connection.execute_batch(sql).unwrap();
        let mut count = 0;
        let complete = run(&connection, 4, is_py, |_| {
            count += 1;
            Ok(count < 513)
        })
        .unwrap();
        assert!(!complete, "{sql}");
        assert_eq!(count, 513);
        assert!(
            run(&connection, 4, |_| true, |_| Ok(true)).is_err(),
            "{sql}"
        );
    }
}

#[test]
fn small_scan_and_empty_scan_stay_serial() {
    let connection = fixture();
    connection
        .execute("DELETE FROM blocks WHERE id>=10", [])
        .unwrap();
    assert_eq!(
        collect(&connection, false, 7, false),
        collect(&connection, false, 1, false)
    );
    connection.execute("DELETE FROM blocks", []).unwrap();
    assert!(collect(&connection, false, 7, false).is_empty());
}

#[test]
fn directory_lookup_and_block_read_share_one_snapshot_during_update() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "fs-scan-snapshot-{}-{}.db",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    populate(&connection);
    connection
        .execute("INSERT INTO directories VALUES(2,X'6c617465')", [])
        .unwrap();
    connection
        .execute("UPDATE blocks SET directory=2 WHERE id>=600", [])
        .unwrap();
    let writer = Connection::open(&path).unwrap();
    {
        let transaction = connection.transaction().unwrap();
        let mut count = 0;
        run(&transaction, 4, is_py, |p| {
            count += 1;
            if count == 1 {
                writer.execute_batch("UPDATE directories SET path=X'6368616e676564'; UPDATE blocks SET n=20 WHERE id>=600;").unwrap();
            }
            assert!(p.starts_with(if count < 600 { b"/root/".as_slice() } else { b"/root/late/" }));
            Ok(true)
        }).unwrap();
        assert_eq!(count, 900);
    }
    drop(writer);
    drop(connection);
    std::fs::remove_file(path).unwrap();
}
