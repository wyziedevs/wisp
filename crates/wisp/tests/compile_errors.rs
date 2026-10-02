//! What the derives and `#[action]` refuse to compile, and where they say
//! it: a small crate of one bad item per line is checked with `cargo check`
//! (offline, in a target folder that later runs reuse), and each error must
//! be on its line, in the words the docs promise. This is the one test that
//! builds a crate; the first run compiles `wisp` for it, later ones do not.

use std::path::Path;
use std::process::Command;

/// One item per line, with the words its error must have.
const CASES: &[(&str, &str)] = &[
    (
        "#[derive(wisp::Json)] struct G<T>(T);",
        "#[derive(Json)] does not take generic types",
    ),
    (
        "#[derive(wisp::FromJson)] struct G<T>(T);",
        "#[derive(FromJson)] does not take generic types",
    ),
    (
        "#[derive(wisp::Cookie)] struct G<T>(T);",
        "#[derive(Cookie)] does not take generic types",
    ),
    (
        "#[derive(wisp::Json)] struct W where u8: Copy { a: u8 }",
        "#[derive(Json)] does not take `where` clauses",
    ),
    (
        "#[derive(wisp::Json)] enum E { A(u8) }",
        "#[derive(Json)] works on enums whose variants have no fields",
    ),
    (
        "#[derive(wisp::FromJson)] enum E { A { b: u8 } }",
        "#[derive(FromJson)] works on enums whose variants have no fields",
    ),
    (
        "#[derive(wisp::Json)] enum E {}",
        "#[derive(Json)] needs an enum with at least one variant",
    ),
    (
        "#[derive(wisp::Cookie)] enum E {}",
        "#[derive(Cookie)] needs an enum with at least one variant",
    ),
    (
        "#[derive(wisp::Json)] union U { a: u8 }",
        "#[derive(Json)] works on a struct or an enum",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(email = 1)] a: String }",
        "`email` takes no value",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(min)] a: u8 }",
        "`min` needs a value: `min = 1`",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(max_len)] a: String }",
        "`max_len` needs a value: `max_len = 1`",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(bogus = 1)] a: u8 }",
        "#[validate] has no `bogus`: it takes len, min, max, min_len, max_len, email and max_size",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(min 1)] a: u8 }",
        "expected `= value`",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(len = 5)] a: String }",
        "`len = 5` needs a range, such as `len = 1..=100`",
    ),
    (
        "#[derive(wisp::FromJson)] struct S { #[validate(len = ..)] a: String }",
        "`len = ..` needs a bound, such as `len = 1..=100`",
    ),
    (
        "#[wisp::action(now)] fn add() {}",
        "#[action] takes no arguments",
    ),
];

/// Where the crate is built: reused by every run, so only the first compiles `wisp`.
const TARGET: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/compile-errors");

#[test]
fn a_bad_derive_is_refused_where_it_is_written() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("compile-errors-crate");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let wisp = Path::new(env!("CARGO_MANIFEST_DIR"))
        .to_str()
        .unwrap()
        .replace('\\', "/");
    let manifest = format!(
        "[package]\nname = \"compile-errors\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nwisp = {{ path = \"{wisp}\" }}\n\n[workspace]\n"
    );
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    // The same versions as the workspace, so nothing is looked up.
    let lock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    std::fs::copy(lock, dir.join("Cargo.lock")).unwrap();
    // Line 1 is a comment, so each case is on line `k + 2`.
    let source: String = CASES.iter().map(|(code, _)| format!("{code}\n")).collect();
    std::fs::write(
        dir.join("src/lib.rs"),
        format!("// one bad item to a line\n{source}"),
    )
    .unwrap();

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "check",
            "--offline",
            "--quiet",
            "--message-format",
            "short",
            "--color",
            "never",
        ])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", TARGET)
        // Not the flags of the run that is testing (coverage, say): the crate is checked as it is.
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&out.stderr).replace('\\', "/");
    assert!(!out.status.success(), "{shown}");

    let errors: Vec<&str> = shown
        .lines()
        .filter(|l| l.starts_with("src/lib.rs:"))
        .collect();
    for (k, (code, said)) in CASES.iter().enumerate() {
        let at = format!("src/lib.rs:{}:", k + 2);
        assert!(
            errors.iter().any(|e| e.contains(&at) && e.contains(said)),
            "`{code}` should say {said:?} on line {}:\n{shown}",
            k + 2
        );
    }
    // Nothing else is wrong with the crate: every error is on one of the lines above.
    let lines: Vec<String> = (0..CASES.len())
        .map(|k| format!("src/lib.rs:{}:", k + 2))
        .collect();
    for e in &errors {
        assert!(
            lines.iter().any(|l| e.starts_with(l)),
            "an error away from its case: {e}"
        );
    }
}
