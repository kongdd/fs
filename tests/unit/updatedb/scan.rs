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
    };
    started.recv().unwrap();
    drop(scanner);
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
