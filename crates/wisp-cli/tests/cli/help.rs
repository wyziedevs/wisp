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
                "wisp dev [--port|-p <n>]",
                "wisp build ",
                "wisp build --static [--out dist]",
                "wisp build --docker [--force]",
                "wisp build --target|-t <host> [--edge] [--out|-o dist/<host>]",
                "cloudflare, pages, deno, vercel, netlify, node, bun or lambda",
                "wisp build --client ts [--out client.ts]",
                "wisp openapi [-o|--out openapi.json]",
                "wisp openapi --check",
                "wisp deploy init <host> [--force]",
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
    for bad in ["bogus", "--verbose", "New"] {
        let o = fail(&cwd, &[bad], &format!("There is no command {bad}."));
        assert!(o.err.contains("Run wisp --help"));
        assert!(o.out.is_empty());
    }
}

#[test]
fn version_prints_the_version() {
    let cwd = Dir::new("version");
    for flag in ["--version", "-V"] {
        let o = wisp(&cwd, &[flag]);
        assert!(
            o.ok && o.out
                == format!(
                    "wisp {}
",
                    env!("CARGO_PKG_VERSION")
                ),
            "{}",
            o.out
        );
    }
}
