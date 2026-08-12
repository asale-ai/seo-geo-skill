//! Output helpers.
//!
//! Skills consume this binary's stdout, so JSON must be the only thing on
//! stdout when `--json` is set. Human-readable summaries and progress notes
//! go to stderr.

use std::fmt;

pub type CmdResult<T = ()> = Result<T, Error>;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(format!("json: {e}"))
    }
}

impl From<crate::safety::UrlSafetyError> for Error {
    fn from(e: crate::safety::UrlSafetyError) -> Self {
        Error(format!("url_safety: {e}"))
    }
}

impl From<crate::http::HttpError> for Error {
    fn from(e: crate::http::HttpError) -> Self {
        Error(e.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(format!("sqlite: {e}"))
    }
}

pub fn err<T>(msg: impl Into<String>) -> CmdResult<T> {
    Err(Error(msg.into()))
}

/// Print a value as pretty JSON on stdout.
pub fn print_json<T: serde::Serialize>(value: &T) -> CmdResult {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}

/// Read a source argument that may be `-` (stdin) or a file path.
pub fn read_source(source: &str) -> CmdResult<String> {
    if source == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf)
    } else {
        std::fs::read_to_string(source).map_err(|e| Error(format!("could not read {source}: {e}")))
    }
}

/// Current UTC timestamp in RFC 3339, used for every record this tool writes.
pub fn now_utc() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Current UTC date as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    let d = time::OffsetDateTime::now_utc().date();
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

/// `YYYY-MM-DD` for `days` ago, for API date-range defaults.
pub fn days_ago(days: i64) -> String {
    let d = (time::OffsetDateTime::now_utc() - time::Duration::days(days)).date();
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

/// Truncate a string for display without splitting a UTF-8 char.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
}

/// Round a currency amount and collapse negative zero, which `f64` sums
/// produce and which renders as `$-0`.
pub fn money(value: f64) -> f64 {
    let rounded = (value * 100_000.0).round() / 100_000.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}
