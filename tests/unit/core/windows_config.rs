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
    assert!(Config::parse(&EXAMPLE_CONFIG.replace(".git", r"cache\\\\nested")).is_err());
}
