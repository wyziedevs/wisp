//! `wisp --help`, and what it says about anything it does not know.

use crate::{Dir, fail, has, wisp};

#[test]
fn help_lists_every_command_and_new_option() {
    let cwd = Dir::new("help");
    for args in [&["--help"][..], &["-h"], &["help"], &[]] {
        let o = wisp(&cwd, args);
        assert!(o.ok, "{args:?}: {}", o.err);
        has(
            &o.out,
            &[
                "A fast, fun web framework for Rust",
                "Usage",
                "wisp new [name]",
                "wisp dev [--port <n>]",
                "wisp build ",
                "wisp build --static [--out dist]",
                "wisp build --docker [--force]",
                "wisp build --target <host> [--out dist/<host>]",
                "cloudflare, deno, vercel, netlify or node",
                "wisp build --client ts [--out client.ts]",
                "wisp check",
                "wisp update-docs",
                "wisp mcp",
                "Options for wisp new",
                "--template demo|minimal|api",
                "also --api",
                "--[no-]tailwind",
                "--[no-]git",
                "--[no-]install",
                "-y, --yes",
            ],
        );
    }
}

#[test]
fn an_unknown_command_points_at_help() {
    let cwd = Dir::new("unknown");
    for bad in ["bogus", "--version", "New"] {
        let o = fail(&cwd, &[bad], &format!("There is no command {bad}."));
        assert!(o.err.contains("Run wisp --help"));
        assert!(o.out.is_empty());
    }
}
