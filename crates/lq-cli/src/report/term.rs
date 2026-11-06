//! What the terminal on stderr can do.

use std::io::IsTerminal;

const DEFAULT_WIDTH: usize = 100;
const MIN_WIDTH: usize = 40;
const MAX_WIDTH: usize = 140;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// SGR codes. The 16-color palette, so that the terminal's own theme applies.
pub mod style {
    pub const BOLD: &str = "1";
    pub const DIM: &str = "2";
    pub const ERROR: &str = "1;31";
    pub const WARNING: &str = "1;33";
    pub const INFO: &str = "1;34";
    pub const SUCCESS: &str = "1;32";
    pub const RED: &str = "31";
    pub const GREEN: &str = "32";
    pub const CYAN: &str = "36";
    pub const RAIL: &str = "90";
}

#[derive(Clone, Copy, Debug)]
pub struct Term {
    pub color: bool,
    pub links: bool,
    pub width: usize,
}

impl Term {
    /// For a report on stderr.
    pub fn detect(choice: ColorChoice) -> Term {
        Term::for_stream(choice, std::io::stderr().is_terminal())
    }

    /// For text on stdout.
    pub fn detect_stdout(choice: ColorChoice) -> Term {
        Term::for_stream(choice, std::io::stdout().is_terminal())
    }

    fn for_stream(choice: ColorChoice, tty: bool) -> Term {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let tty = enable_ansi() && tty;
        let color = wants_color(choice, tty, env);
        let links = tty && color && supports_links();
        let columns = env("COLUMNS").and_then(|c| c.parse().ok()).or_else(terminal_width).unwrap_or(DEFAULT_WIDTH);
        Term { color, links, width: columns.clamp(MIN_WIDTH, MAX_WIDTH) }
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// An OSC 8 hyperlink, or just the text.
    pub fn link(&self, url: &str, text: &str) -> String {
        if self.links {
            format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
        } else {
            text.to_string()
        }
    }
}

/// The usual conventions. `NO_COLOR` wins, `FORCE_COLOR` means what its value says, and only a
/// value of `CLICOLOR_FORCE` other than 0 does anything.
fn wants_color(choice: ColorChoice, tty: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    match choice {
        ColorChoice::Always => return true,
        ColorChoice::Never => return false,
        ColorChoice::Auto => {}
    }
    if env("NO_COLOR").is_some() {
        return false;
    }
    if let Some(force) = env("FORCE_COLOR") {
        return !matches!(force.as_str(), "0" | "false");
    }
    if env("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
        return true;
    }
    tty && env("CLICOLOR").as_deref() != Some("0") && env("TERM").as_deref() != Some("dumb")
}

fn supports_links() -> bool {
    let var = |name: &str| std::env::var_os(name).is_some();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    matches!(program.as_str(), "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "rio")
        || var("WT_SESSION")
        || var("KITTY_WINDOW_ID")
}

#[cfg(unix)]
fn terminal_width() -> Option<usize> {
    let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ only writes a `winsize` through the pointer, which is valid here.
    let ok = unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_col > 0).then_some(size.ws_col as usize)
}

#[cfg(unix)]
fn enable_ansi() -> bool {
    true
}

#[cfg(windows)]
use console::{enable_ansi, terminal_width};

#[cfg(windows)]
mod console {
    use std::ffi::c_void;

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x4;

    /// `CONSOLE_SCREEN_BUFFER_INFO`. `window` is left, top, right, bottom.
    #[repr(C)]
    #[derive(Default)]
    struct ScreenBufferInfo {
        size: [i16; 2],
        cursor: [i16; 2],
        attributes: u16,
        window: [i16; 4],
        maximum: [i16; 2],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: *mut c_void, info: *mut ScreenBufferInfo) -> i32;
    }

    /// Turns on escape sequences in a console. Without them it prints the codes literally. A
    /// stream that isn't a console, such as a pipe or Git Bash's terminal, needs nothing.
    pub fn enable_ansi() -> bool {
        [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].into_iter().all(|id| {
            let mut mode = 0;
            // SAFETY: both calls take a handle from `GetStdHandle`, which they check, and the
            // first only writes `mode`, which is valid here.
            unsafe {
                let handle = GetStdHandle(id);
                GetConsoleMode(handle, &mut mode) == 0
                    || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
            }
        })
    }

    pub fn terminal_width() -> Option<usize> {
        let mut info = ScreenBufferInfo::default();
        // SAFETY: the call checks the handle and only writes `info`, which is valid here.
        let ok = unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_ERROR_HANDLE), &mut info) } != 0;
        let columns = i32::from(info.window[2]) - i32::from(info.window[0]) + 1;
        (ok && columns > 0).then_some(columns as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::{wants_color, ColorChoice};

    fn auto(tty: bool, vars: &[(&str, &str)]) -> bool {
        wants_color(ColorChoice::Auto, tty, |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| (*v).to_string()))
    }

    #[test]
    fn follows_the_conventions() {
        assert!(auto(true, &[]));
        assert!(!auto(false, &[]));
        assert!(!auto(true, &[("NO_COLOR", "1")]));
        assert!(!auto(true, &[("TERM", "dumb")]));
        assert!(!auto(true, &[("CLICOLOR", "0")]));
        assert!(auto(false, &[("FORCE_COLOR", "1")]));
        assert!(auto(false, &[("CLICOLOR_FORCE", "1")]));
        assert!(!auto(false, &[("CLICOLOR_FORCE", "0")]));
        assert!(!auto(true, &[("FORCE_COLOR", "0")]));
        assert!(!auto(false, &[("NO_COLOR", "1"), ("FORCE_COLOR", "1")]));
    }
}
