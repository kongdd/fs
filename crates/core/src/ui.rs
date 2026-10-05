use std::io::{IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

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

/// Replace one terminal row, clipping before coloring to prevent automatic wrapping.
pub fn write_progress(tone: Tone, message: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    #[cfg(any(unix, windows))]
    let width = terminal_size::terminal_size_of(std::io::stderr())
        .map(|(terminal_size::Width(width), _)| usize::from(width))
        .filter(|&width| width > 0);
    #[cfg(not(any(unix, windows)))]
    let width: Option<usize> = None;
    let width = width
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()?
                .parse()
                .ok()
                .filter(|&w| w > 0)
        })
        .unwrap_or(80);
    // Leave the last column unused: some terminals wrap as soon as it is filled.
    let line = fit_progress(&message.to_string(), width.saturating_sub(1));
    let mut stderr = std::io::stderr().lock();
    write!(stderr, "\r\x1b[2K{}", paint(&line, tone, stderr_color()))?;
    stderr.flush()
}

fn fit_progress(message: &str, columns: usize) -> String {
    // Filenames can contain newlines, tabs and terminal control characters.
    let clean: String = message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if clean.width() <= columns {
        return clean;
    }
    if columns == 0 {
        return String::new();
    }
    let mut clipped = String::new();
    for ch in clean.chars() {
        clipped.push(ch);
        if clipped.width() > columns - 1 {
            clipped.pop();
            break;
        }
    }
    clipped.push('…');
    clipped
}

/// Windows consoles ignore `\r` and erase sequences until virtual-terminal mode is on.
pub fn enable_ansi() {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
        }
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        let handle = std::io::stderr().as_raw_handle();
        let mut mode = 0u32;
        unsafe {
            if GetConsoleMode(handle, &mut mode) != 0 {
                let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/core/ui.rs"]
mod tests;
