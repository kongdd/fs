use super::*;

#[test]
fn ignore_rules_persist_and_are_idempotent() {
    let dir = std::env::temp_dir().join(format!("nasfind-ignore-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    fs::write(&config, "# untouched config\n").unwrap();
    assert!(load(&config).unwrap().is_empty());
    for _ in 0..2 {
        run(
            &config,
            IgnoreAction::Add {
                dirs: vec!["cache with spaces".into()],
            },
        )
        .unwrap();
    }
    assert_eq!(load(&config).unwrap(), vec!["cache with spaces"]);
    assert_eq!(fs::read_to_string(&config).unwrap(), "# untouched config\n");
    assert!(load(&dir.join("other.toml")).unwrap().is_empty());
    for _ in 0..2 {
        run(
            &config,
            IgnoreAction::Rm {
                dirs: vec!["cache with spaces".into()],
            },
        )
        .unwrap();
        assert!(load(&config).unwrap().is_empty());
    }
    for name in ["", ".", "..", "/missing/cache", "./cache", "cache/"] {
        assert!(
            run(
                &config,
                IgnoreAction::Add {
                    dirs: vec![name.into()]
                }
            )
            .is_err()
        );
        assert!(
            run(
                &config,
                IgnoreAction::Rm {
                    dirs: vec![name.into()]
                }
            )
            .is_err()
        );
    }
    run(
        &config,
        IgnoreAction::Add {
            dirs: vec![
                "FY4B".into(),
                "node_modules".into(),
                "FY4B".into(),
                "cache with spaces".into(),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        load(&config).unwrap(),
        vec!["FY4B", "cache with spaces", "node_modules"]
    );
    let before = fs::read(store_path(&config)).unwrap();
    assert!(
        run(
            &config,
            IgnoreAction::Add {
                dirs: vec!["new-cache".into(), "/invalid/path".into()],
            }
        )
        .is_err()
    );
    assert!(run(&config, IgnoreAction::Add { dirs: vec![] }).is_err());
    assert_eq!(fs::read(store_path(&config)).unwrap(), before);
    assert!(
        run(
            &config,
            IgnoreAction::Rm {
                dirs: vec!["FY4B".into(), "/invalid/path".into()],
            }
        )
        .is_err()
    );
    assert!(run(&config, IgnoreAction::Rm { dirs: vec![] }).is_err());
    assert_eq!(fs::read(store_path(&config)).unwrap(), before);
    for _ in 0..2 {
        run(
            &config,
            IgnoreAction::Rm {
                dirs: vec![
                    "FY4B".into(),
                    "node_modules".into(),
                    "FY4B".into(),
                    "not-present".into(),
                ],
            },
        )
        .unwrap();
        assert_eq!(load(&config).unwrap(), vec!["cache with spaces"]);
    }
    fs::write(store_path(&config), "broken").unwrap();
    assert!(load(&config).is_err());
    fs::write(store_path(&config), "[\"/missing/cache\"]").unwrap();
    assert!(load(&config).is_err());
    fs::write(store_path(&config), "[\"node_modules\"]").unwrap();
    assert_eq!(load(&config).unwrap(), vec!["node_modules"]);
    fs::remove_dir_all(dir).unwrap();
}
