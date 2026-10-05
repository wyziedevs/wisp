//! The warning that the CLI is older than the app's wisp crate: on stderr,
//! never waiting without a terminal, and silenced by WISP_NO_UPDATE_CHECK.

use crate::{Dir, command, wisp, write};

/// An app whose Cargo.lock says `wisp` is at a version no CLI has reached.
fn ahead_app(cwd: &Dir) -> std::path::PathBuf {
    let app = cwd.join("app");
    write(&app, "Cargo.toml", "[package]\nname = \"app\"\n");
    write(&app, "build.rs", "fn main() {}\n");
    write(
        &app,
        "Cargo.lock",
        "[[package]]\nname = \"wisp\"\nversion = \"999.0.0\"\n",
    );
    app
}

#[test]
fn an_older_cli_warns_on_stderr_and_goes_on() {
    let cwd = Dir::new("update-check");
    let app = ahead_app(&cwd);
    // The route scan fails (no routes), and the warning comes first.
    let o = wisp(&app, &["routes"]);
    for said in [
        "older than this app's wisp crate",
        "app 999.0.0",
        "cargo install wisp-web --force",
        "WISP_NO_UPDATE_CHECK=1",
    ] {
        assert!(o.err.contains(said), "wanted {said:?} in {}", o.err);
    }
    assert!(!o.out.contains("999.0.0"), "{}", o.out);
    let warned = o.err.find("older than").unwrap();
    assert!(
        o.err.find("src/routes").is_none_or(|at| warned < at),
        "{}",
        o.err
    );
    assert!(!o.err.contains("Continue anyway"), "{}", o.err);

    let quiet = command(&app)
        .arg("routes")
        .env("WISP_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&quiet.stderr);
    assert!(!err.contains("older than"), "{err}");

    // Not for commands outside an app, nor for `new`.
    let o = wisp(&cwd, &["routes"]);
    assert!(!o.err.contains("older than"), "{}", o.err);
    let o = wisp(&app, &["--help"]);
    assert!(!o.err.contains("older than"), "{}", o.err);
}
