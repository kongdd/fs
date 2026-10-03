use super::*;
fn expr(text: &str) -> Expr {
    parse(&SearchOptions {
        patterns: vec![text.into()],
        ..Default::default()
    })
    .unwrap()
}
#[test]
fn defaults_and_boolean_precedence() {
    let q = expr("soil | rain ext:nc !backup");
    assert!(q.matches(b"/backup/SOIL.txt"));
    assert!(q.matches(b"/data/RAIN.nc"));
    assert!(!q.matches(b"/rain/other.nc"));
    assert!(!q.matches(b"/data/rain_backup.nc"));
    assert!(!q.matches(b"/data/rain.txt"));
}
#[test]
fn groups_quotes_paths_and_bytes() {
    let q = expr("<soil | rain> ext:nc;tif");
    assert!(q.matches(b"/data/soil.TIF"));
    assert!(!q.matches(b"/data/soil.csv"));
    assert!(expr("\"a | b\"").matches(b"/data/a | b.txt"));
    assert!(expr("path:\"my data\" *.nc").matches(b"/my data/bad_\xff.nc"));
    assert!(expr("data/soil").matches(b"/data/soil.nc"));
    assert!(expr("regex:soil[0-9]").matches(b"/data/SOIL7.nc"));
}
#[test]
fn anchors_are_sound_and_bad_syntax_rejected() {
    assert_eq!(anchor(b"[abc]*soil?.nc"), Some(b"soil".to_vec()));
    assert_eq!(expr("soil | !rain").branches().unwrap()[1].len(), 0);
    for text in [
        "a |", "<>", "<a", "a >", "!", "\"a", "ext:", "folder:", "path:", "[abc",
    ] {
        assert!(
            parse(&SearchOptions {
                patterns: vec![text.into()],
                ..Default::default()
            })
            .is_err(),
            "{text}"
        );
    }
}
