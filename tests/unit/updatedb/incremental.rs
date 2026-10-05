use super::super::schema;
use super::*;

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    schema(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO blocks(directory,data,size,n) VALUES (0,x'00',1,1)",
            [],
        )
        .unwrap();
    connection
}

#[test]
fn optional_count_cache_is_invalidated_by_other_block_writers() {
    for sql in [
        "INSERT INTO blocks(directory,data,size,n) VALUES (1,x'00',1,7)",
        "UPDATE blocks SET n=9",
        "DELETE FROM blocks",
    ] {
        let connection = fixture();
        assert_eq!(cached_entry_count(&connection).unwrap(), None);
        store_entry_count(&connection, 1).unwrap();
        assert_eq!(cached_entry_count(&connection).unwrap(), Some(1));
        connection.execute(sql, []).unwrap();
        assert_eq!(cached_entry_count(&connection).unwrap(), None, "{sql}");
        let count = count_entries(&connection).unwrap();
        store_entry_count(&connection, count).unwrap();
        assert_eq!(cached_entry_count(&connection).unwrap(), Some(count));
        // Changes to compressed bytes do not affect the number of entries.
        connection
            .execute("UPDATE blocks SET data=x'01'", [])
            .unwrap();
        assert_eq!(cached_entry_count(&connection).unwrap(), Some(count));
    }
}

#[test]
fn unprotected_or_malformed_count_metadata_is_not_trusted() {
    let connection = fixture();
    connection
        .execute(
            "INSERT INTO meta(key,value) VALUES ('entry_count',?)",
            [999u64.to_le_bytes().as_slice()],
        )
        .unwrap();
    assert_eq!(cached_entry_count(&connection).unwrap(), None);
    store_entry_count(&connection, 1).unwrap();
    connection
        .execute("UPDATE meta SET value=x'00' WHERE key='entry_count'", [])
        .unwrap();
    assert!(cached_entry_count(&connection).is_err());
}

#[cfg(unix)]
#[test]
fn snapshot_keeps_raw_sorted_children_even_without_directory_rows() {
    let connection = fixture();
    for (id, path) in [
        (1, b"".as_slice()),
        (2, b"a"),
        (3, b"a/child"),
        (4, b"a-other"),
    ] {
        connection
            .execute(
                "INSERT INTO directories(id,path,stamp) VALUES (?,?,?)",
                params![id, path, path],
            )
            .unwrap();
    }
    for (id, name) in [
        (1, b"a".as_slice()),
        (1, b"a-other"),
        (2, b"z_unreadable"),
        (2, b"bad_\xff"),
        (2, b"child"),
    ] {
        connection
            .execute(
                "INSERT INTO children(directory,name) VALUES (?,?)",
                params![id, name],
            )
            .unwrap();
    }
    let snapshot = DirectorySnapshot::load(&connection, b"a/").unwrap();
    assert_eq!(snapshot.directories.len(), 2);
    let parent = &snapshot.directories[b"a".as_slice()];
    assert_eq!(parent.id, 2);
    assert_eq!(parent.stamp, b"a");
    let expected = [b"bad_\xff".as_slice(), b"child", b"z_unreadable"]
        .into_iter()
        .map(|name| os_string(name.to_vec()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(parent.children, expected);
    assert!(
        snapshot.directories[b"a/child".as_slice()]
            .children
            .is_empty()
    );
    assert_eq!(
        DirectorySnapshot::load(&connection, b"")
            .unwrap()
            .directories
            .len(),
        4
    );
}
