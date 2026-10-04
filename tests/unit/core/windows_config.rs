use super::*;

#[test]
fn windows_example_uses_absolute_paths() {
    let cfg = Config::parse(EXAMPLE_CONFIG).unwrap();
    assert!(
        cfg.index
            .iter()
            .all(|idx| idx.root.is_absolute() && idx.database.is_absolute())
    );
    assert!(Config::parse(&EXAMPLE_CONFIG.replace("C:/Users", "C:Users")).is_err());
    assert!(Config::parse(&EXAMPLE_CONFIG.replace(".git", r"cache\\nested")).is_err());
}

#[test]
fn bare_drive_letter_is_the_volume_root() {
    let cfg = Config::parse(
        r#"
[[index]]
name = "c"
root = "c:"
database = "D:/fs/c.db"
"#,
    )
    .unwrap();
    assert_eq!(cfg.index[0].root, std::path::PathBuf::from("c:/"));
    assert!(cfg.index[0].root.is_absolute());
}
