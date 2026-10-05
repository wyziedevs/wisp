//! The gates CI runs, one command on Windows, macOS and Linux (no shell):
//! `cargo run -q -p wisp-gate [fast|wasm]`. Stops at the first red step.
//! `fast` skips the edge wasm steps (they need `rustup target add
//! wasm32-unknown-unknown`); `wasm` runs only the wasm size.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

/// The edge wasm of tests/app, built as `wisp build --target cloudflare`
/// builds it (release, stripped, request-only), may not pass BUDGET bytes. A
/// host loads the file on every cold start, and it grew 8% once without
/// anyone choosing it. BUDGET is the size after the last cut plus 2%: raise
/// it on purpose, in the commit that adds the code, never to make a red
/// build green. Cold start is not gated: workerd's is bimodal on one machine.
/// Raised from 1_810_000: 3065761 (`wisp add crud` tests) put three routes
/// and a saved `Memo` table in tests/app, +55 KB of the app's own code
/// (twiggy: `Table<Memo>`, its forms and pages; the runtime unchanged), and
/// the REST fixes after it +7 KB: 1_852_689 bytes, plus 2%.
const BUDGET: u64 = 1_890_000;
const WASM: &str = "wasm32-unknown-unknown";

fn cargo(args: &[&str], env: &[(&str, &str)]) -> bool {
    println!("\n$ cargo {}", args.join(" "));
    let mut c = Command::new("cargo");
    c.args(args).envs(env.iter().copied());
    c.status().is_ok_and(|s| s.success())
}

fn wasm_size() -> bool {
    let env = [
        ("WISP_REQUEST_ONLY", "1"),
        ("CARGO_PROFILE_RELEASE_STRIP", "symbols"),
    ];
    let build = [
        "build",
        "-q",
        "--release",
        "-p",
        "wisp-test-app",
        "--target",
        WASM,
    ];
    if !cargo(&build, &env) {
        return false;
    }
    let dir =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);
    let file = dir.join(WASM).join("release").join("wisp-test-app.wasm");
    let Ok(meta) = std::fs::metadata(&file) else {
        eprintln!("gate: {} is missing", file.display());
        return false;
    };
    println!("wasm: {} bytes (budget {BUDGET})", meta.len());
    meta.len() <= BUDGET || {
        eprintln!("gate: the wasm is over budget");
        false
    }
}

fn main() -> ExitCode {
    let mode = std::env::args().nth(1);
    let (fast, wasm_only) = (
        mode.as_deref() == Some("fast"),
        mode.as_deref() == Some("wasm"),
    );
    let clippy = [
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ];
    let wasm_clippy = [
        "clippy",
        "-p",
        "wisp-test-app",
        "--all-targets",
        "--target",
        WASM,
        "--",
        "-D",
        "warnings",
    ];
    let mut ok = wasm_only
        || cargo(&["fmt", "--check"], &[])
            && cargo(&clippy, &[])
            && cargo(&["test", "--workspace", "-q"], &[]);
    if ok && !fast {
        ok = (wasm_only || cargo(&wasm_clippy, &[])) && wasm_size();
    }
    if ok {
        println!("\ngate: green");
        ExitCode::SUCCESS
    } else {
        eprintln!("\ngate: red");
        ExitCode::FAILURE
    }
}
