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
fn basename_pushdown_is_conservative_for_paths_and_not() {
    let expressions = [
        "*.csv",
        "ext:py",
        "soil md",
        "soil | ext:py",
        "!ext:py",
        "path:soil ext:py",
        "!path:soil",
        "!<path:soil ext:py>",
        "!<soil | path:rain>",
        "!<soil ext:py>",
        "regex:soil",
        "path:soil | rain",
    ];
    let paths: &[&[u8]] = &[
        b"/soil/other.py",
        b"/rain/soil.csv",
        b"/data/SOIL.md",
        b"/data/bad_\xff.py",
        b"/data/.py",
        b"/data/other.txt",
    ];
    for expression in expressions {
        let q = expr(expression);
        for path in paths {
            let name = path.rsplit(|&b| b == b'/').next().unwrap();
            assert!(
                !q.matches(path) || q.may_match_basename(name),
                "{expression} {path:?}"
            );
            if q.basename_only() {
                assert_eq!(q.matches(path), q.may_match_basename(name));
            }
        }
    }
    assert!(!expr("*.csv").may_match_basename(b"report.csv.bak"));
    assert!(!expr("ext:py").may_match_basename(b"report.py.bak"));
    assert!(!expr("!ext:py").may_match_basename(b"report.py"));
    assert!(expr("!path:soil").may_match_basename(b"soil.py"));
}

#[test]
fn extension_candidates_are_sound() {
    let q = expr("ext:py;CSV;[ab];x");
    let branches = q.branches().unwrap();
    assert_eq!(branches.len(), 4);
    for path in [
        b"/data/bad_\xff.PY".as_slice(),
        b"/data/report.csv",
        b"/data/report.[ab]",
        b"/data/report.x",
    ] {
        assert!(q.matches(path));
        assert!(
            branches
                .iter()
                .any(|branch| branch.iter().all(|(pattern, basename)| {
                    Query::new(&SearchOptions {
                        patterns: vec![pattern.clone()],
                        basename: *basename,
                        ignore_case: true,
                        ..Default::default()
                    })
                    .unwrap()
                    .matches(path)
                }))
        );
    }
    assert!(expr("!ext:py").branches().unwrap()[0].is_none());
    let list = std::iter::repeat_n("py", 65).collect::<Vec<_>>().join(";");
    assert!(expr(&format!("ext:{list}")).branches().unwrap()[0].is_none());
    let list = (0..9)
        .map(|n| format!("e{n}"))
        .collect::<Vec<_>>()
        .join(";");
    assert!(expr(&format!("ext:{list} ext:{list}")).branches().unwrap()[0].is_none());
}

#[test]
fn required_grams_merge_and_without_crossing_or_or_not() {
    use std::collections::BTreeSet;
    let required = |text| {
        expr(text)
            .required_grams()
            .into_iter()
            .collect::<BTreeSet<_>>()
    };
    let soil: BTreeSet<_> = grams(b"soil").into_iter().collect();
    let mut both = soil.clone();
    both.extend(grams(b"rain"));
    assert_eq!(required("soil rain"), both);
    assert_eq!(required("soil !rain"), soil);
    assert!(required("soil | soil_rain").is_empty());
    assert!(expr("soil | rain").required_grams().is_empty());
    assert!(expr("soil | !rain").required_grams().is_empty());
    assert!(expr("!<soil rain>").required_grams().is_empty());
    assert_eq!(expr("ext:py").required_grams(), grams(b".py"));
    assert!(expr("ext:py;x;[ab]").required_grams().is_empty());

    for expression in [
        "soil rain",
        "path:soil ext:nc",
        "<soil | rain> ext:nc",
        "soil | soil_rain",
        "ext:py;py",
        "ext:[ab]",
        "!<path:soil ext:py>",
    ] {
        let q = expr(expression);
        let required: BTreeSet<_> = q.required_grams().into_iter().collect();
        for path in [
            b"/soil/rain.nc".as_slice(),
            b"/data/soil_rain.nc",
            b"/data/SOIL.txt",
            b"/data/bad_\xff.py",
            b"/data/literal.[ab]",
            b"/data/.py",
        ] {
            if q.matches(path) {
                let actual: BTreeSet<_> = grams(path).into_iter().collect();
                assert!(required.is_subset(&actual), "{expression}: {path:?}");
            }
        }
    }
}

#[test]
fn anchors_are_sound_and_bad_syntax_rejected() {
    for (text, anchor, basename) in [
        ("soil md", "soil", true),
        ("soil readme", "readme", true),
        ("soil !rain", "soil", true),
        ("soil rain", "rain", true),
        ("path:soil md", "soil", false),
    ] {
        assert_eq!(
            expr(text).branches().unwrap(),
            vec![Some((anchor.into(), basename))]
        );
    }
    assert_eq!(anchor(b"[abc]*soil?.nc"), Some(b"soil".to_vec()));
    assert!(expr("soil | !rain").branches().unwrap()[1].is_none());
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
