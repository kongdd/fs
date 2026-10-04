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
