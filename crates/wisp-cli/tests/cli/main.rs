//! End-to-end tests of the `wisp` command: the built binary, run in
//! throwaway folders with no terminal and no network.
//!
//! Builds share one target folder under the system's temp folder, at the
//! lowest optimization, so the dependencies compile once and every later
//! test (and run) only compiles its own small app.

mod bridge;
mod build;
mod check;
mod dev;
mod errors;
mod help;
mod mcp;
mod new;
mod openapi;
mod update_check;

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A folder for one test, removed when it is dropped.
pub struct Dir(PathBuf);

impl Dir {
    pub fn new(name: &str) -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("wisp-cli-test-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Deref for Dir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        // A dev server's app may still be exiting, and holds its file until
        // it has.
        for _ in 0..40 {
            if fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

/// What a finished `wisp` printed.
pub struct Out {
    pub ok: bool,
    pub out: String,
    pub err: String,
}

/// Wisp's own folder, where the demo and API templates come from.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `wisp` in `cwd`, as a script would run it: stdin closed, no colors, cargo
/// offline and cheap, no Tailwind to download.
pub fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wisp"));
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .env(
            "CARGO_TARGET_DIR",
            std::env::temp_dir().join("wisp-cli-tests-target"),
        )
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_PROFILE_DEV_OPT_LEVEL", "0")
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "0")
        .env("CARGO_PROFILE_RELEASE_LTO", "off")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16")
        .env("NO_COLOR", "1")
        .env("WISP_TAILWIND", cwd.join("no-such-tailwind"))
        // Coverage flags would rebuild every app in the shared folder.
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("HOST")
        .env_remove("PORT");
    cmd
}

pub fn wisp(cwd: &Path, args: &[&str]) -> Out {
    wisp_env(cwd, args, &[])
}

pub fn wisp_env(cwd: &Path, args: &[&str], env: &[(&str, &Path)]) -> Out {
    let out = command(cwd)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    Out {
        ok: out.status.success(),
        out: String::from_utf8_lossy(&out.stdout).into_owned(),
        err: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Runs `wisp`, which must fail with `said` on stderr.
pub fn fail(cwd: &Path, args: &[&str], said: &str) -> Out {
    let o = wisp(cwd, args);
    assert!(!o.ok, "{args:?} should fail; it printed {}", o.out);
    assert!(
        o.err.contains(said),
        "{args:?}: wanted {said:?} in {}",
        o.err
    );
    o
}

/// `wisp new NAME` in `cwd` with nothing to ask or install.
pub fn new_app(cwd: &Path, name: &str, more: &[&str]) -> PathBuf {
    let mut args = vec!["new", name, "-y", "--no-git", "--no-install"];
    args.extend_from_slice(more);
    let o = wisp(cwd, &args);
    assert!(o.ok, "{args:?}: {}", o.err);
    cwd.join(name)
}

/// [`new_app`] with the repository's Cargo.lock, so a build uses the
/// dependency versions already downloaded instead of looking for newer ones.
pub fn pinned_app(cwd: &Path, name: &str, more: &[&str]) -> PathBuf {
    let app = new_app(cwd, name, more);
    fs::copy(repo().join("Cargo.lock"), app.join("Cargo.lock")).unwrap();
    app
}

pub fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

pub fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Every file under `root`, `/`-separated and sorted.
pub fn tree(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// A stand-in for the Tailwind program that writes `.wisp/app.css`, says a
/// few things on stderr as Tailwind does, and exits with `code`.
pub fn fake_tailwind(dir: &Path, code: i32) -> PathBuf {
    let (name, script) = if cfg!(windows) {
        (
            "tailwind.cmd",
            format!(
                "@echo off\r\nmkdir .wisp 2>nul\r\necho fake>.wisp\\app.css\r\necho Done in 5ms 1>&2\r\necho warning: other 1>&2\r\necho Error: boom 1>&2\r\nexit /b {code}\r\n"
            ),
        )
    } else {
        (
            "tailwind.sh",
            format!(
                "#!/bin/sh\nmkdir -p .wisp\necho fake > .wisp/app.css\necho Done in 5ms >&2\necho warning: other >&2\necho Error: boom >&2\nexit {code}\n"
            ),
        )
    };
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

pub fn has(haystack: &str, needles: &[&str]) {
    for n in needles {
        assert!(haystack.contains(n), "wanted {n:?} in {haystack}");
    }
}
