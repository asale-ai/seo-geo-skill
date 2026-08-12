//! Headless Chrome bridge.
//!
//! Rendering, screenshots, and PDF printing all need a real browser engine.
//! Rather than bundle one (which would multiply the binary size by an order
//! of magnitude), we drive whatever Chrome/Chromium/Edge the machine already
//! has. Callers get a clear, actionable error when none is installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::output::{err, CmdResult, Error};

const ENV_OVERRIDE: &str = "SEOGEO_CHROME";

#[cfg(target_os = "macos")]
const CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
];

#[cfg(target_os = "linux")]
const CANDIDATES: &[&str] = &[
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/microsoft-edge",
    "/snap/bin/chromium",
];

#[cfg(target_os = "windows")]
const CANDIDATES: &[&str] = &[
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
];

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
const CANDIDATES: &[&str] = &[];

/// Locate a usable Chrome binary, honouring `$SEOGEO_CHROME` first.
pub fn find() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(ENV_OVERRIDE) {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    for c in CANDIDATES {
        let p = Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    // Fall back to anything on PATH.
    for name in ["google-chrome", "chromium", "chrome", "msedge"] {
        if let Ok(out) = Command::new("which").arg(name).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() && Path::new(&s).exists() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }
    None
}

pub fn require() -> CmdResult<PathBuf> {
    find().ok_or_else(|| {
        Error(
            "headless Chrome not found. Install Google Chrome, Chromium, or Edge, \
             or point SEOGEO_CHROME at the executable."
                .into(),
        )
    })
}

fn base_args(timeout_ms: u64) -> Vec<String> {
    vec![
        "--headless=new".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--disable-dev-shm-usage".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-extensions".into(),
        "--disable-background-networking".into(),
        format!("--virtual-time-budget={timeout_ms}"),
    ]
}

/// Render a URL and return the post-JavaScript DOM.
pub fn dump_dom(url: &str, timeout_ms: u64, user_agent: Option<&str>) -> CmdResult<String> {
    let chrome = require()?;
    let mut args = base_args(timeout_ms);
    args.push("--dump-dom".into());
    if let Some(ua) = user_agent {
        args.push(format!("--user-agent={ua}"));
    }
    args.push(url.to_string());

    let out = Command::new(&chrome)
        .args(&args)
        .output()
        .map_err(|e| Error(format!("could not launch {}: {e}", chrome.display())))?;

    if !out.status.success() && out.stdout.is_empty() {
        return err(format!(
            "chrome render failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Screenshot a URL to a PNG path.
pub fn screenshot(
    url: &str,
    output: &Path,
    width: u32,
    height: u32,
    full_page: bool,
    timeout_ms: u64,
) -> CmdResult<()> {
    let chrome = require()?;
    let mut args = base_args(timeout_ms);
    args.push(format!("--screenshot={}", output.display()));
    args.push(format!("--window-size={width},{height}"));
    if full_page {
        args.push("--screenshot-format=png".into());
        args.push("--hide-scrollbars".into());
    }
    args.push(url.to_string());

    let out = Command::new(&chrome)
        .args(&args)
        .output()
        .map_err(|e| Error(format!("could not launch {}: {e}", chrome.display())))?;
    if !output.exists() {
        return err(format!(
            "chrome screenshot produced no file: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Print a local HTML file to PDF.
pub fn print_pdf(html_path: &Path, pdf_path: &Path, timeout_ms: u64) -> CmdResult<()> {
    let chrome = require()?;
    let abs_html = std::fs::canonicalize(html_path)
        .map_err(|e| Error(format!("cannot resolve {}: {e}", html_path.display())))?;
    let mut args = base_args(timeout_ms);
    args.push(format!("--print-to-pdf={}", pdf_path.display()));
    args.push("--no-pdf-header-footer".into());
    args.push(format!("file://{}", abs_html.display()));

    let out = Command::new(&chrome)
        .args(&args)
        .output()
        .map_err(|e| Error(format!("could not launch {}: {e}", chrome.display())))?;
    if !pdf_path.exists() {
        return err(format!(
            "chrome produced no PDF: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}
