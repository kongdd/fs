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
fn stat_batches_are_bounded_ordered_and_keep_individual_errors() {
    let root = std::env::temp_dir();
    for jobs in [1, 4] {
        let mut pool = StatPool::new(&root, jobs);
        let mut pending: Vec<_> = (0..1000)
            .map(|n| root.join(format!("fs-missing-stat-{}-{n}", std::process::id())))
            .collect();
        pending.push(root.clone());
        let mut checked = 0;
        while !pending.is_empty() {
            let before = pending.clone();
            let mut batch = pool.take_batch(&mut pending).unwrap();
            assert!(batch.len() <= jobs * STAT_BATCH_SIZE);
            assert_eq!(pending, before[..before.len() - batch.len()]);
            for expected in before.iter().rev().take(batch.len()) {
                let (path, result) = batch.pop().unwrap();
                assert_eq!(&path, expected);
                match stamp_io(&path, &root) {
                    Ok(stamp) => assert_eq!(result.unwrap(), stamp),
                    Err(error) => assert_eq!(result.unwrap_err().kind(), error.kind()),
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 1001);
        assert!(pool.take_batch(&mut pending).unwrap().is_empty());
    }
}

#[test]
fn stat_pool_drop_disconnects_queued_jobs_and_results() {
    let root = std::env::temp_dir();
    let pool = StatPool::new(&root, 2);
    for offset in 0..2 {
        pool.job_tx
            .as_ref()
            .unwrap()
            .send((offset, vec![root.clone(); 32]))
            .unwrap();
    }
    // Drop while workers can be sending into a full result queue.
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
