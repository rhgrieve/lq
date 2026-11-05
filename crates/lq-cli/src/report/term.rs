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

#[cfg(not(unix))]
fn terminal_width() -> Option<usize> {
    None
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
