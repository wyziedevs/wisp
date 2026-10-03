//! HTTP by curl, which ships with Windows 10+, macOS and every Linux
//! distribution: the CLI needs no HTTP client of its own.

use std::path::Path;
use std::process::{Command, Stdio};

/// Why a fetch failed: the server answered with an error (a 404, say), or
/// there was no answer (no network, no curl).
pub enum Fail {
    Status,
    Other(String),
}

impl Fail {
    pub fn text(&self) -> &str {
        match self {
            Fail::Status => "the server answered with an error",
            Fail::Other(e) => e,
        }
    }
}

/// `url`'s body, into `to` (with a progress bar) or returned.
pub fn fetch(url: &str, to: Option<&Path>) -> Result<Vec<u8>, Fail> {
    let mut cmd = Command::new("curl");
    cmd.args(["-fL", "--connect-timeout", "30", "--retry", "2"]);
    match to {
        Some(file) => {
            cmd.args(["--progress-bar", "-o"]).arg(file);
        }
        None => {
            cmd.arg("-sS").stdout(Stdio::piped());
        }
    }
    let out = cmd
        .arg(url)
        .stderr(if to.is_some() {
            Stdio::inherit()
        } else {
            Stdio::piped()
        })
        .output()
        .map_err(|e| Fail::Other(format!("could not run curl: {e}; install curl")))?;
    match out.status.code() {
        Some(0) => Ok(out.stdout),
        // curl's code for an HTTP error, with -f.
        Some(22) => Err(Fail::Status),
        _ => {
            let why = String::from_utf8_lossy(&out.stderr);
            let why = why.trim().trim_start_matches("curl: ");
            Err(Fail::Other(if why.is_empty() {
                "curl failed".into()
            } else {
                why.into()
            }))
        }
    }
}
