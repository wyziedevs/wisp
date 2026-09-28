//! The `wisp` command.

mod ask;
mod cargo;
mod css;
mod dev;
mod events;
mod new;
mod sha256;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "wisp: fast, fun web apps in Rust

usage:
  wisp new [name]         create an app (asks a few questions; see below)
  wisp dev [--port <n>]   run with hot reload (default port 3000)
  wisp build              release binary with CSS and static files inside
  wisp check              check routes and templates without compiling

wisp new options, to answer its questions up front:
  --template demo|minimal   an app to learn from, or one empty page
  --tailwind, --no-tailwind
  --git, --no-git           create a git repository
  --install, --no-install   download and compile dependencies now
  -y, --yes                 take the defaults for anything not given
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let result = match args.first().map(String::as_str) {
        Some("new") => new::run(&args[1..]),
        Some("dev") => {
            let port = arg("--port").map_or(Ok(3000), |p| p.parse().map_err(|_| format!("bad port: {p}")));
            port.and_then(|port| project().and_then(|root| dev::run(root, port)))
        }
        Some("build") => project().and_then(build),
        Some("check") => project().and_then(|root| wisp_build::check(root).map(|()| println!("ok"))),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wisp: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The current directory, if it looks like a Wisp app.
fn project() -> Result<&'static Path, String> {
    let root = Path::new(".");
    if !root.join("Cargo.toml").exists() || !root.join("build.rs").exists() {
        return Err("run this in a Wisp app (a directory with Cargo.toml and build.rs)".into());
    }
    Ok(root)
}

fn build(root: &Path) -> Result<(), String> {
    wisp_build::check(root)?;
    css::build(root)?;
    let b = cargo::build(root, true);
    let exe = b.exe.filter(|_| b.ok).ok_or("build failed")?;
    let size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
    println!("\nbuilt {} ({:.1} MB) — one file, ready to deploy", exe.display(), size as f64 / 1e6);
    Ok(())
}
