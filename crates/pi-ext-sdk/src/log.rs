//! Logging to stderr, which pi shows with the extension path as prefix.
//! Warnings and errors always print; `PI_EXT_LOG=info` or `PI_EXT_LOG=debug` adds more.

use std::fmt;
use std::io::Write;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

fn threshold() -> Level {
    static THRESHOLD: OnceLock<Level> = OnceLock::new();
    *THRESHOLD.get_or_init(|| match std::env::var("PI_EXT_LOG").as_deref() {
        Ok("debug") | Ok("trace") => Level::Debug,
        Ok(value) if !value.is_empty() => Level::Info,
        _ => Level::Warn,
    })
}

pub fn enabled(level: Level) -> bool {
    level >= threshold()
}

pub fn write(level: Level, args: fmt::Arguments<'_>) {
    if enabled(level) {
        let _ = writeln!(std::io::stderr().lock(), "{}: {args}", level.label());
    }
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Debug, ::std::format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Info, ::std::format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Warn, ::std::format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Error, ::std::format_args!($($arg)*)) };
}
