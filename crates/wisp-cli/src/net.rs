//! HTTP by curl, which ships with Windows 10+, macOS and every Linux
//! distribution: the CLI needs no HTTP client of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Why a fetch failed, in words.
pub struct Fail {
    /// The server answered with an error (a 404, say), rather than not at
    /// all (no network, no curl).
    pub status: bool,
    pub text: String,
}

/// `url`'s body.
pub fn fetch(url: &str) -> Result<Vec<u8>, Fail> {
    curl(url, None)
}

/// Downloads `url` to `dest`, whole or not at all: into a file beside it
/// (named by the process, so two `wisp` commands never write one file),
/// which `check` may read, change or refuse (its error is returned), then
/// moved into place. Another `wisp` may have put it there first; its copy
/// is the same.
pub fn fetch_to(
    url: &str,
    dest: &Path,
    check: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut partial = dest.as_os_str().to_owned();
    partial.push(format!(".{}.download", std::process::id()));
    let partial = PathBuf::from(partial);
    let done = curl(url, Some(&partial))
        .map_err(|e| format!("Could not download {url}: {}.", e.text))
        .and_then(|_| check(&partial))
        .and_then(|()| match fs::rename(&partial, dest) {
            Err(e) if !dest.exists() => Err(format!("Could not write {}: {e}.", dest.display())),
            _ => Ok(()),
        });
    let _ = fs::remove_file(&partial);
    done
}

/// curl, the body into `out` or returned.
fn curl(url: &str, out: Option<&Path>) -> Result<Vec<u8>, Fail> {
    let mut cmd = Command::new("curl");
    cmd.args(["-fsSL", "--connect-timeout", "30", "--retry", "2"]);
    if let Some(file) = out {
        cmd.arg("-o").arg(file);
    }
    let out = cmd
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| Fail {
            status: false,
            text: format!("could not run curl: {e}; install curl"),
        })?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let why = String::from_utf8_lossy(&out.stderr);
    let why = why.trim().trim_start_matches("curl: ");
    Err(Fail {
        // curl's code for an HTTP error, with -f.
        status: out.status.code() == Some(22),
        text: if why.is_empty() {
            "curl failed".into()
        } else {
            why.into()
        },
    })
}
