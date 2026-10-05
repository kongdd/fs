use super::*;

#[test]
fn drop_disconnects_a_full_queue_before_joining() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (ready, started) = mpsc::channel();
    let worker = thread::spawn(move || {
        sender.send(Err(anyhow::anyhow!("first"))).unwrap();
        ready.send(()).unwrap();
        assert!(sender.send(Err(anyhow::anyhow!("second"))).is_err());
    });
    let scanner = Scanner {
        receiver: Some(receiver),
        worker: Some(worker),
        profile: None,
        skipped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    started.recv().unwrap();
    drop(scanner);
}

#[test]
fn stat_prefetch_bounds_all_outstanding_results_and_falls_back() {
    let root = std::env::temp_dir();
    let mut pool = StatPool::new(&root, 2);
    let paths: Vec<_> = (0..1000)
        .map(|n| root.join(format!("fs-missing-stat-{}-{n}", std::process::id())))
        .collect();
    for path in &paths {
        pool.submit(path.clone());
    }
    assert_eq!(pool.submitted.len(), pool.capacity);
    for path in paths.iter().rev() {
        let expected = stamp_io(path, &root).unwrap_err().kind();
        assert_eq!(pool.take(path).unwrap_err().kind(), expected);
        assert!(pool.ready.len() <= pool.capacity);
        assert!(pool.submitted.len() <= pool.capacity);
    }
    assert!(pool.ready.is_empty());
    assert!(pool.submitted.is_empty());
    assert_eq!(pool.take(&root).unwrap(), stamp_io(&root, &root).unwrap());
    pool.prefetch(&paths);
    assert!(pool.submitted.contains(paths.last().unwrap()));
    // Drop while a full window is still outstanding; workers must be reaped.
    drop(pool);
}

#[test]
fn worker_panic_is_not_treated_as_successful_eof() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        drop(sender);
        panic!("injected scan failure");
    });
    let mut scanner = Scanner {
        receiver: Some(receiver),
        worker: Some(worker),
        profile: None,
        skipped: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    assert!(
        scanner
            .next()
            .err()
            .unwrap()
            .to_string()
            .contains("scanner panicked")
    );
}
