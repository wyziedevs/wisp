//! The edge build's answers for paths that never change (a baked page, a
//! trailing-slash redirect) come from the bridge's table after the first
//! one, without entering the wasm: the same status, headers and body as the
//! native server sends, for GET, HEAD and every `if-none-match` that names the
//! ETag or not. Skipped, with a note, when Node or Rust's wasm32 target is not
//! there.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The native server, killed when dropped.
struct Native(Child);

impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn have(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The wasm build of the app, in a folder with the bridge and the harness.
fn edge_app(package: &str) -> Option<PathBuf> {
    let target = "wasm32-unknown-unknown";
    if !have(Command::new("node").arg("--version")) {
        eprintln!("fast: no node, skipped");
        return None;
    }
    if !have(Command::new("rustc").args(["--print", "target-libdir", "--target", target])) {
        eprintln!("fast: no {target}, skipped");
        return None;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // This test's own `target/debug/deps/fast-*`: three up is the target folder.
    let exe = std::env::current_exe().ok()?;
    let dir = exe.ancestors().nth(3)?.to_path_buf();
    let built = Command::new(cargo)
        .args(["build", "-p", package, "--target", target])
        .env("CARGO_TARGET_DIR", &dir)
        .status()
        .ok()?;
    if !built.success() {
        eprintln!("fast: no wasm build, skipped");
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::env::temp_dir().join(format!("wisp-fast-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).ok()?;
    let put = |from: PathBuf, to: &str| std::fs::copy(from, out.join(to)).expect(to);
    put(
        dir.join(target).join(format!("debug/{package}.wasm")),
        "app.wasm",
    );
    put(
        root.join("../../crates/wisp-cli/src/targets/bridge.js"),
        "bridge.mjs",
    );
    put(root.join("tests/fast.mjs"), "fast.mjs");
    Some(out)
}

#[test]
fn constant_paths_are_answered_by_the_table_as_native_answers_them() {
    let Some(dir) = edge_app("wisp-test-platform") else {
        return;
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_wisp-test-platform"))
        .env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .env("WISP_THREADS", "2")
        .env("WISP_SECRET", "0123456789abcdef0123456789abcdef")
        .env("WISP_DATA", "off")
        .env("WISP_DEV", "off")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the app");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let _native = Native(child);
    let port = line.trim().rsplit(':').next().unwrap().to_string();
    let run = Command::new("node")
        .arg(dir.join("fast.mjs"))
        .arg(format!("http://127.0.0.1:{port}"))
        .output()
        .expect("run node");
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&run.stdout);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "{err}
{text}"
    );
    assert!(text.lines().count() > 20, "{text}");
}

/// An app with hooks (the test app's `before` can answer any request) has no
/// table: its baked pages still enter the wasm every time.
#[test]
fn an_app_with_hooks_has_no_table() {
    let Some(dir) = edge_app("wisp-test-app") else {
        return;
    };
    let run = Command::new("node")
        .arg(dir.join("fast.mjs"))
        .arg("-")
        .output()
        .expect("run node");
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&run.stdout);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{err}\n{text}");
    assert!(text.lines().count() >= 3, "{text}");
}
