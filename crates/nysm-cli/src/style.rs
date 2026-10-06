//! Minimal ANSI styling that honours NO_COLOR, --color and non-TTY stdout.

use std::io::IsTerminal;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    pub fn new(choice: ColorChoice) -> Self {
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                    && std::io::stdout().is_terminal()
                    && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
            }
        };
        Style { color }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn warn(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn bad(&self, s: &str) -> String {
        self.wrap("31", s)
    }
}
