use super::*;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

fn query(pattern: OsString, basename: bool, ignore_case: bool) -> Query {
    Query::new(&SearchOptions {
        patterns: vec![pattern],
        basename,
        ignore_case,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn literal_case_and_path_scope() {
    assert!(query("soil".into(), true, true).matches(b"/data/SOIL.nc"));
    assert!(!query("soil".into(), true, false).matches(b"/data/SOIL.nc"));
    assert!(!query("soil".into(), true, true).matches(b"/soil/other.nc"));
    assert!(query("soil".into(), false, true).matches(b"/soil/other.nc"));
}

#[test]
fn globs_match_full_names_and_preserve_raw_bytes() {
    assert!(query("*.nc".into(), true, true).matches(b"/data/bad_\xff.NC"));
    assert!(query("*.txt".into(), true, false).matches(b"/data/line\nfile.txt"));
    assert!(query("[sr]oil?.nc".into(), true, false).matches(b"/data/soil1.nc"));
    assert!(!query("soil*.nc".into(), true, false).matches(b"/data/xsoil.nc"));
    #[cfg(unix)]
    assert!(
        query(OsString::from_vec(b"bad_\xff".to_vec()), true, false).matches(b"/data/bad_\xff.nc")
    );
}

#[test]
fn literal_fast_path_matches_bytewise_oracle() {
    for first in 0..=255u8 {
        let value = [b'A', b'a', first, b'B', b'\n', 0xff, first, b'b'];
        for needle in [
            vec![],
            vec![first],
            vec![first, b'b'],
            vec![first, b'B'],
            vec![0xff, first],
            vec![first; 9],
        ] {
            for ignore_case in [false, true] {
                let expected = needle.is_empty()
                    || value.windows(needle.len()).any(|window| {
                        if ignore_case {
                            window.eq_ignore_ascii_case(&needle)
                        } else {
                            window == needle
                        }
                    });
                assert_eq!(
                    contains(&value, &needle, ignore_case),
                    expected,
                    "{needle:?} {ignore_case}"
                );
                assert!(!contains(b"", &[first], ignore_case));
            }
        }
    }
}

#[test]
fn simple_globs_match_regex_oracle() {
    let patterns = [
        "*", "**", "*.csv", "**.CSV", "soil*", "soil**", "*soil*", "**soil**", "a*b", "a?", "[ab]*",
    ];
    let values: &[&[u8]] = &[
        b"",
        b"soil",
        b"SOIL.csv",
        b"soil\n.csv",
        b"bad_\xff.csv",
        b".csv",
        b"a",
        b"ab",
        b"a\nb",
        b"xsoil.csv",
        b"soil.csv\n",
    ];
    for pattern in patterns {
        for ignore_case in [false, true] {
            let q = query(pattern.into(), false, ignore_case);
            let oracle = RegexBuilder::new(&glob_regex(pattern.as_bytes()).unwrap())
                .unicode(false)
                .case_insensitive(ignore_case)
                .build()
                .unwrap();
            for value in values {
                assert_eq!(
                    q.matches(value),
                    oracle.is_match(value),
                    "{pattern} {ignore_case} {value:?}"
                );
            }
        }
    }
    assert!(matches!(
        query("*.csv".into(), true, true).matchers[0],
        Matcher::Suffix(_)
    ));
    assert!(matches!(
        query("soil*".into(), true, true).matchers[0],
        Matcher::Prefix(_)
    ));
    assert!(matches!(
        query("a*b".into(), true, true).matchers[0],
        Matcher::Regex(_)
    ));
}

#[test]
fn regex_and_invalid_patterns() {
    let q = Query::new(&SearchOptions {
        patterns: vec!["^soil[0-9]+[.]nc$".into()],
        basename: true,
        ignore_case: true,
        regex: true,
        ..Default::default()
    })
    .unwrap();
    assert!(q.matches(b"/data/SOIL12.nc"));
    assert!(!q.matches(b"/data/soil.nc"));
    for pattern in ["[", "[abc", "[]"] {
        assert!(
            Query::new(&SearchOptions {
                patterns: vec![pattern.into()],
                ..Default::default()
            })
            .is_err()
        );
    }
    assert!(
        Query::new(&SearchOptions {
            patterns: vec!["(".into()],
            regex: true,
            ..Default::default()
        })
        .is_err()
    );
}
