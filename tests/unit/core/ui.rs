use super::*;

#[test]
fn color_respects_terminal_and_environment() {
    assert!(color_enabled(true, false, Some("xterm")));
    assert!(!color_enabled(false, false, Some("xterm")));
    assert!(!color_enabled(true, true, Some("xterm")));
    assert!(!color_enabled(true, false, Some("dumb")));
    assert_eq!(paint("done", Tone::Success, false), "done");
    assert_eq!(paint("error", Tone::Error, true), "\x1b[1;31merror\x1b[0m");
}

#[test]
fn progress_fits_one_row_including_wide_and_combining_characters() {
    assert_eq!(fit_progress("ETA --", 20), "ETA --");
    assert_eq!(fit_progress("abcdef", 4), "abc…");
    assert_eq!(fit_progress("目录目录", 6), "目录…");
    assert_eq!(fit_progress("e\u{301}xyz", 3), "e\u{301}x…");
    assert_eq!(fit_progress("long", 1), "…");
    assert_eq!(fit_progress("long", 0), "");
    for width in 0..80 {
        let line = fit_progress(
            "progress: 目录 · ETA -- · 👩‍💻 · e\u{301}"
                .repeat(10)
                .as_str(),
            width,
        );
        assert!(line.width() <= width, "{width}: {line:?}");
    }
}

#[test]
fn progress_sanitizes_filename_control_characters() {
    let line = fit_progress("dir\nname\r\t\x1b[2J", 80);
    assert_eq!(line, "dir name   [2J");
    assert!(!line.chars().any(char::is_control));
}
