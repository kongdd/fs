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
