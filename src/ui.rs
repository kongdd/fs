use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

pub fn stderr_color() -> bool {
    color_enabled(
        std::io::stderr().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn color_enabled(terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    terminal && !no_color && term != Some("dumb")
}

pub fn paint(text: &str, tone: Tone, color: bool) -> String {
    if !color {
        return text.into();
    }
    let code = match tone {
        Tone::Info => "1;36",
        Tone::Success => "1;32",
        Tone::Warning => "1;33",
        Tone::Error => "1;31",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

pub fn log(tone: Tone, message: std::fmt::Arguments<'_>) {
    eprintln!("{}", paint(&message.to_string(), tone, stderr_color()));
}

#[cfg(test)]
mod tests {
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
}
