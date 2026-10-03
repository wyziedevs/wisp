//! The `wisp` command.

mod ask;
mod cargo;
mod css;
mod deploy;
mod dev;
mod events;
mod fmt;
mod lsp;
mod mcp;
mod net;
mod new;
mod npm;
mod targets;
#[cfg(test)]
mod template_files;
mod term;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

/// `wisp --help`: each command or option, and what it does.
const COMMANDS: [(&str, &str); 15] = [
    (
        "wisp new [name]",
        "Create an app. It asks a few questions; the options below answer them.",
    ),
    (
        "wisp dev [--port <n>]",
        "Run the app, rebuilding and reloading on every save. Port 3000 by default.",
    ),
    (
        "wisp build",
        "Build one release binary with the CSS and static files inside.",
    ),
    (
        "wisp build --static [--out dist]",
        "Write the pages as plain files, for any static host.",
    ),
    (
        "wisp build --docker [--force]",
        "Write a Dockerfile and .dockerignore.",
    ),
    (
        "wisp build --target <host> [--out dist/<host>]",
        "Write a folder ready to deploy to cloudflare, deno, vercel, netlify or node.",
    ),
    (
        "wisp build --client ts [--out client.ts]",
        "Write a typed TypeScript client of the app's +server.rs endpoints.",
    ),
    (
        "wisp check",
        "Check routes and templates without compiling.",
    ),
    (
        "wisp fmt [paths]",
        "Format .wisp files: markup, the --- block (rustfmt), scripts and styles.",
    ),
    (
        "wisp fmt --check",
        "Name the .wisp files that are not formatted, and fail if any are.",
    ),
    (
        "wisp add <pkg>[@version]",
        "Add an npm package to package.json, for import x from 'pkg'. No Node needed.",
    ),
    (
        "wisp remove <pkg>",
        "Take an npm package out of package.json.",
    ),
    (
        "wisp lsp",
        "Run the language server for editors, over stdio (editors/vscode starts it).",
    ),
    (
        "wisp update-docs",
        "Bring AGENTS.md, the reference for AI agents, up to this Wisp.",
    ),
    (
        "wisp mcp",
        "Serve docs, routes, components and checks to AI agents (MCP, stdio).",
    ),
];

const NEW_OPTIONS: [(&str, &str); 5] = [
    (
        "--template demo|minimal|api",
        "An app to learn from, one empty page, or a JSON API (also --api).",
    ),
    ("--[no-]tailwind", "Add Tailwind CSS, or leave it out."),
    ("--[no-]git", "Create a git repository, or leave it out."),
    (
        "--[no-]install",
        "Download and compile dependencies now, or later.",
    ),
    ("-y, --yes", "Take the defaults for anything not given."),
];

fn usage() -> String {
    let width = COMMANDS
        .iter()
        .chain(&NEW_OPTIONS)
        .map(|(c, _)| c.len())
        .max()
        .unwrap_or(0);
    let rows = |rows: &[(&str, &str)]| -> String {
        rows.iter()
            .map(|(c, about)| format!("  {}  {about}\n", term::accent(&format!("{c:width$}"))))
            .collect()
    };
    format!(
        "{}\n\n{}\n{}\n{}\n{}",
        term::banner(),
        term::bold("Usage"),
        rows(&COMMANDS),
        term::bold("Options for wisp new"),
        rows(&NEW_OPTIONS)
    )
}

fn main() -> ExitCode {
    // `std::env::args` panics on an argument that is not UTF-8.
    let args: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string())
        .collect();
    let Ok(args) = args else {
        term::failed("An argument is not valid UTF-8.\nUse names made of ordinary text.");
        return ExitCode::FAILURE;
    };
    let result = match args.first().map(String::as_str) {
        Some("new") => new::run(&args[1..]),
        Some("dev") => {
            dev_port(&args[1..]).and_then(|port| project().and_then(|root| dev::run(root, port)))
        }
        Some("build") => {
            build_options(&args[1..]).and_then(|o| project().and_then(|root| build(root, &o)))
        }
        Some("check") => no_options("check", &args[1..])
            .and_then(|()| project())
            .and_then(|root| {
                wisp_build::check(root)?;
                term::done("Routes and templates are valid.");
                fmt::warn_unformatted(root);
                Ok(())
            }),
        Some("fmt") => fmt::run(&args[1..]),
        Some("add") => project().and_then(|root| npm::add(root, &args[1..])),
        Some("remove") => project().and_then(|root| npm::remove(root, &args[1..])),
        Some("lsp") => no_options("lsp", &args[1..]).and_then(|()| lsp::run()),
        Some("update-docs") => no_options("update-docs", &args[1..])
            .and_then(|()| project())
            .and_then(new::update_docs),
        Some("mcp") => no_options("mcp", &args[1..]).and_then(|()| mcp::run()),
        // Not in --help: how `wisp dev` runs a tool that must end with it.
        Some("__child") => css::child(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            print!("{}", usage());
            Ok(())
        }
        Some(other) => Err(format!(
            "There is no command {other}.\nRun wisp --help to see the commands."
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            term::failed(&e);
            ExitCode::FAILURE
        }
    }
}

/// `wisp dev`'s one option: `--port <n>`, `--port=<n>` or `-p <n>`.
fn dev_port(args: &[String]) -> Result<u16, String> {
    let usage = "wisp dev takes --port <n>, like wisp dev --port 3001.";
    let mut port = 3000;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = match arg.split_once('=') {
            Some(("--port" | "-p", v)) => v,
            _ if arg == "--port" || arg == "-p" => args
                .next()
                .ok_or_else(|| format!("{arg} needs a port number.\n{usage}"))?,
            _ if arg.starts_with('-') => return Err(format!("There is no option {arg}.\n{usage}")),
            _ => return Err(format!("Unexpected {arg}.\n{usage}")),
        };
        port = value.parse().map_err(|_| {
            format!(
                "{value} is not a port number.\nUse one from 1 to 65535, like wisp dev --port 3001."
            )
        })?;
    }
    Ok(port)
}

/// What `wisp build` was asked for.
#[derive(Debug, Default, PartialEq)]
struct BuildOptions {
    static_site: bool,
    docker: bool,
    force: bool,
    out: Option<String>,
    /// `--target <host>`, an edge or Node host (`static` and `docker` set
    /// the flags above).
    target: Option<String>,
    /// `--client ts`: write the TypeScript client of the app's endpoints.
    client: bool,
}

fn build_options(args: &[String]) -> Result<BuildOptions, String> {
    let usage = "wisp build takes --static [--out <folder>], --docker [--force], --target <host> [--out <folder>] and --client ts [--out <file>].";
    let mut o = BuildOptions::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--static" => o.static_site = true,
            "--target" | "-t" => {
                let host = args
                    .next()
                    .ok_or_else(|| format!("{arg} needs a host.\n{usage}"))?;
                target(&mut o, host)?;
            }
            _ if arg.starts_with("--target=") => target(&mut o, &arg["--target=".len()..])?,
            "--docker" => o.docker = true,
            "--force" => o.force = true,
            "--client" | "--client=ts" => {
                if arg == "--client" && args.next().map(String::as_str) != Some("ts") {
                    return Err(format!(
                        "--client takes ts: wisp build --client ts.\n{usage}"
                    ));
                }
                o.client = true;
            }
            "--out" | "-o" => {
                let out = args
                    .next()
                    .ok_or_else(|| format!("{arg} needs a folder.\n{usage}"))?;
                o.out = Some(out.clone());
            }
            _ if arg.starts_with("--out=") => o.out = Some(arg["--out=".len()..].to_string()),
            _ if arg.starts_with('-') => {
                return Err(format!("There is no option {arg}.\n{usage}"));
            }
            _ => return Err(format!("Unexpected {arg}.\n{usage}")),
        }
    }
    let wrong = if o.out.as_deref() == Some("") {
        Some("--out needs a folder.")
    } else if o.target.is_some() && (o.static_site || o.docker) {
        Some("--target <host> goes alone, without --static or --docker.")
    } else if o.client && (o.static_site || o.docker || o.target.is_some()) {
        Some("--client ts goes alone, with --out <file> if you like.")
    } else if o.out.is_some() && !o.static_site && o.target.is_none() && !o.client {
        Some("--out goes with --static, --target or --client.")
    } else if o.force && !o.docker {
        Some("--force goes with --docker.")
    } else {
        None
    };
    if let Some(wrong) = wrong {
        return Err(format!("{wrong}\n{usage}"));
    }
    Ok(o)
}

/// `--target <host>`: `static` and `docker` are the options of those names.
fn target(o: &mut BuildOptions, host: &str) -> Result<(), String> {
    match host {
        "static" => o.static_site = true,
        "docker" => o.docker = true,
        _ if targets::HOSTS.contains(&host) => o.target = Some(host.to_string()),
        _ => {
            return Err(format!(
                "There is no target {host}.\nThe targets are {}, static and docker.",
                targets::HOSTS.join(", ")
            ));
        }
    }
    Ok(())
}

fn no_options(command: &str, args: &[String]) -> Result<(), String> {
    match args.first() {
        Some(arg) => Err(format!(
            "Unexpected {arg}.\nwisp {command} takes no options."
        )),
        None => Ok(()),
    }
}

/// `dir` and the folders above it, made if missing.
fn make_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}.", dir.display()))
}

/// The current directory, if it looks like a Wisp app.
fn project() -> Result<&'static Path, String> {
    let root = Path::new(".");
    if !root.join("Cargo.toml").exists() || !root.join("build.rs").exists() {
        return Err("There is no Wisp app here.\nRun this in an app's folder, the one with Cargo.toml and build.rs, or create one with wisp new.".into());
    }
    Ok(root)
}

fn build(root: &Path, o: &BuildOptions) -> Result<(), String> {
    if o.client {
        let ts = wisp_build::client_ts(root)?;
        if ts.is_empty() {
            return Err(
                "The app has no endpoints (+server.rs files) to write a client for.".into(),
            );
        }
        let out = o.out.as_deref().unwrap_or("client.ts");
        if let Some(dir) = Path::new(out).parent() {
            make_dir(dir)?;
        }
        std::fs::write(out, ts).map_err(|e| format!("Could not write {out}: {e}"))?;
        term::done(&format!("Wrote {out}"));
        let module = Path::new(out)
            .file_stem()
            .map_or("client".into(), |s| s.to_string_lossy());
        println!(
            "    import {{ client }} from './{module}'; const api = client({{ base, token }});"
        );
        return Ok(());
    }
    // `--out`, else `dist` (`dist/<host>` for a host's build).
    let out = |host: Option<&str>| match (&o.out, host) {
        (Some(out), _) => PathBuf::from(out),
        (None, Some(host)) => PathBuf::from(format!("{}/{host}", deploy::DEFAULT_OUT)),
        (None, None) => PathBuf::from(deploy::DEFAULT_OUT),
    };
    if let Some(host) = &o.target {
        return targets::build(root, host, &out(Some(host)));
    }
    let imports = wisp_build::check(root)?;
    css::build(root)?;
    npm::vendor(root, &imports)?;
    if o.docker {
        deploy::docker(
            root,
            &cargo::package_name(root).ok_or("Cargo.toml has no package name.")?,
            o.force,
        )?;
        if !o.static_site {
            return Ok(());
        }
    }
    let started = Instant::now();
    term::step("Building for release");
    let b = cargo::build(root, true, false);
    let exe = b.exe.filter(|_| b.ok).ok_or(
        "The build failed.
The compiler's errors are above.",
    )?;
    let size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
    term::done(&format!(
        "Built {} {}",
        cargo::relative(&exe.to_string_lossy()),
        term::dim(&format!(
            "{:.1} MB in {:.1}s",
            size as f64 / 1e6,
            started.elapsed().as_secs_f64()
        ))
    ));
    if o.static_site {
        return deploy::static_site(root, &exe, &out(None));
    }
    println!("    One file with the CSS and static files inside. Copy it to a server and run it.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(s: &str) -> Result<u16, String> {
        dev_port(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn build_flags() {
        let opts =
            |s: &str| build_options(&s.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(opts(""), Ok(BuildOptions::default()));
        assert_eq!(
            opts("--static --out site").unwrap(),
            BuildOptions {
                static_site: true,
                out: Some("site".into()),
                ..Default::default()
            }
        );
        assert_eq!(opts("--static --out=site"), opts("--static -o site"));
        assert!(opts("--docker --force").unwrap().force);
        assert!(opts("--client ts --out web/api.ts").unwrap().client);
        assert!(opts("--client=ts").unwrap().client);
        for bad in ["--client", "--client js", "--client ts --static"] {
            assert!(opts(bad).is_err(), "{bad}");
        }
        for bad in ["--out site", "--force", "--static --out", "--x", "dist"] {
            assert!(opts(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn target_flags() {
        let opts =
            |s: &str| build_options(&s.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(
            opts("--target cloudflare").unwrap().target.as_deref(),
            Some("cloudflare")
        );
        assert_eq!(
            opts("--target=node --out app").unwrap().out.as_deref(),
            Some("app")
        );
        assert!(opts("-t static").unwrap().static_site);
        assert!(opts("--target docker").unwrap().docker);
        for bad in [
            "--target",
            "--target heroku",
            "--target=",
            "--target node --static",
            "--docker -t deno",
        ] {
            assert!(opts(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dev_flags() {
        assert_eq!(port(""), Ok(3000));
        assert_eq!(port("--port 3001"), Ok(3001));
        assert_eq!(port("--port=3002"), Ok(3002));
        assert_eq!(port("-p 3003"), Ok(3003));
        assert_eq!(port("-p 0"), Ok(0));
        for bad in [
            "--port",
            "--port x",
            "--port 70000",
            "--prot 3000",
            "-x",
            "3000",
            "--port= 3000",
        ] {
            assert!(port(bad).is_err(), "{bad}");
        }
    }
}
