use super::*;

const CONFIG: &str = r#"
outdir = "D:/fs"

[[index]]
name = "files"
root = "C:/data"
"#;

#[test]
fn database_defaults_to_outdir_and_name() {
    let cfg = Config::parse(CONFIG).unwrap();
    assert_eq!(cfg.index[0].database, Path::new("D:/fs/files.db"));
    assert_eq!(
        cfg.index[0].update_database(),
        cfg.index[0].search_database()
    );
    let cfg = Config::parse(&CONFIG.replace("files", "archive")).unwrap();
    assert_eq!(cfg.index[0].database, Path::new("D:/fs/archive.db"));
}

#[test]
fn explicit_database_and_role_paths_take_precedence() {
    let cfg = Config::parse(&format!(
        "{CONFIG}\ndatabase = \"E:/explicit.db\"\nupdate_database = \"E:/build.db\"\nsearch_database = \"E:/search.db\""
    ))
    .unwrap();
    let idx = &cfg.index[0];
    assert_eq!(idx.database, Path::new("E:/explicit.db"));
    assert_eq!(idx.update_database(), Path::new("E:/build.db"));
    assert_eq!(idx.search_database(), Path::new("E:/search.db"));
}

#[test]
fn rejects_missing_or_relative_output_paths() {
    let missing = CONFIG.replace("outdir = \"D:/fs\"", "");
    let error = Config::parse(&missing).unwrap_err().to_string();
    assert!(error.contains("needs database or global outdir"));
    assert!(Config::parse(&CONFIG.replace("D:/fs", "relative")).is_err());
    assert!(Config::parse(&format!("{CONFIG}\ndatabase = \"relative.db\"")).is_err());
}

#[test]
fn rejects_names_that_could_escape_outdir() {
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "/escape",
        r"bad\\name",
        "C:escape",
        r"\u0000",
    ] {
        assert!(
            Config::parse(&CONFIG.replace("files", name)).is_err(),
            "{name}"
        );
    }
}
