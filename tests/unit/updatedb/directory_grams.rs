use super::*;

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    crate::index_builder::schema(&connection).unwrap();
    connection
}

fn keys(directory: u64) -> Vec<u64> {
    (0..1024).map(|n| directory * 31 + n * 127).collect()
}

#[test]
fn dictionary_roundtrips_compressed_raw_and_empty_lists() {
    let connection = database();
    let mut codec = DirectoryGrams::new(&connection).unwrap();
    codec.stage_fresh(&connection).unwrap();
    for directory in 0..512 {
        codec
            .store(&connection, directory, keys(directory as u64))
            .unwrap();
    }
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM directory_grams", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    codec.finish_fresh(&connection).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM directory_grams WHERE size>0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(count > 0);
    // Reload contexts exactly as a subsequent updatedb invocation does.
    let mut codec = DirectoryGrams::new(&connection).unwrap();
    for directory in 0..512 {
        assert_eq!(
            codec.read(&connection, directory).unwrap(),
            keys(directory as u64)
        );
    }
    for (directory, keys) in [(512, vec![]), (513, vec![0, 1, 0xffffff]), (514, keys(514))] {
        codec.store(&connection, directory, keys.clone()).unwrap();
        assert_eq!(codec.read(&connection, directory).unwrap(), keys);
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT size FROM directory_grams WHERE directory=512",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn small_indexes_stay_raw_without_dictionary() {
    let connection = database();
    let mut codec = DirectoryGrams::new(&connection).unwrap();
    codec.stage_fresh(&connection).unwrap();
    codec
        .store(&connection, 1, vec![0, 127, 128, 0xffffff])
        .unwrap();
    codec.finish_fresh(&connection).unwrap();
    assert!(codec.compressor.is_none());
    assert_eq!(
        codec.read(&connection, 1).unwrap(),
        vec![0, 127, 128, 0xffffff]
    );
    assert_eq!(
        connection
            .query_row("SELECT size FROM directory_grams", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn invalid_sizes_keys_and_frames_are_rejected() {
    let mut codec = DirectoryGrams::with_dictionary(Some(&[127; 256])).unwrap();
    let raw = pack(&keys(1));
    let (compressed, size) = codec.encode(raw.clone()).unwrap();
    assert!(size > 0);
    assert_eq!(codec.decode(&compressed, size as i64).unwrap(), keys(1));
    assert!(codec.decode(&compressed, -1).is_err());
    assert!(codec.decode(&compressed, MAX_BYTES as i64 + 1).is_err());
    assert!(codec.decode(&compressed, size as i64 + 1).is_err());
    assert!(codec.decode(&compressed, size as i64 - 1).is_err());
    assert!(
        codec
            .decode(&compressed[..compressed.len() - 1], size as i64)
            .is_err()
    );
    assert!(codec.decode(b"not a frame", size as i64).is_err());
    assert!(
        DirectoryGrams::with_dictionary(None)
            .unwrap()
            .decode(&compressed, size as i64)
            .is_err()
    );
    assert!(codec.decode(&pack(&[0x1000000]), 0).is_err());
    assert!(codec.decode(&[128], 0).is_err());
    assert!(codec.decode(&[1, 0], 0).is_err());
}
