//! `wisp new [name]`: asks a few questions, then writes an app to start from.
//!
//! Every question has a flag, so scripts and CI can answer up front; `--yes`
//! (or no terminal to ask on) takes the defaults for the rest.

use crate::{ask, css};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq)]
enum Template {
    Demo,
    Minimal,
}

const TEMPLATES: [(&str, &str); 2] = [
    ("Demo", "a home page with a counter, an about page and a word game to learn from"),
    ("Minimal", "one empty page, a layout and an error page"),
];

#[derive(Default)]
struct Answers {
    name: Option<String>,
    template: Option<Template>,
    tailwind: Option<bool>,
    git: Option<bool>,
    install: Option<bool>,
    yes: bool,
}

const REPO: &str = "https://github.com/wyziedevs/wisp";

const USAGE: &str = "wisp new [name] [--template demo|minimal] [--[no-]tailwind] [--[no-]git] [--[no-]install] [--yes]";

pub fn run(args: &[String]) -> Result<(), String> {
    let mut a = parse(args)?;
    let asking = !a.yes && ask::interactive();
    if asking {
        println!("\n{}  {}\n", ask::accent("wisp new"), ask::dim("fast, fun web apps in Rust"));
    }

    let name = match a.name.take() {
        Some(name) => name,
        None if asking => ask::text("Where should the app go?", "my-app")?,
        None => return Err(format!("which directory? usage: {USAGE}")),
    };
    let root = Path::new(&name);
    if fs::read_dir(root).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!("{name} already exists and is not empty"));
    }
    let crate_name = crate_name(root)?;

    let template = match a.template {
        Some(t) => t,
        None if asking => [Template::Demo, Template::Minimal][ask::choose("Which template?", &TEMPLATES, 0)?],
        None => Template::Demo,
    };
    let tailwind = answer(a.tailwind, asking, "Add Tailwind CSS?", false)?;
    // Like `cargo new`: a repository by default, unless the app is going
    // inside one already.
    let parent = root.parent().filter(|p| p.is_dir()).unwrap_or(Path::new("."));
    let has_git = git(parent, &["--version"]).is_ok();
    let git_default = has_git && git(parent, &["rev-parse", "--is-inside-work-tree"]).is_err();
    let use_git = has_git && answer(a.git, asking, "Initialize a git repository?", git_default)?;
    let install = answer(a.install, asking, "Download and compile dependencies now?", true)?;
    if asking {
        println!();
    }

    write(root, &crate_name, template, tailwind)?;
    let label = if template == Template::Demo { "demo" } else { "minimal" };
    println!("Created {} with the {label} template.", ask::bold(&name));

    if use_git {
        git(root, &["init", "--quiet"]).map_err(|e| format!("git init: {e}"))?;
        println!("Initialized a git repository.");
    }
    if install {
        if tailwind {
            css::install()?;
        }
        println!("Compiling dependencies (the first build takes a minute; later ones take seconds)...\n");
        let ok = Command::new("cargo").arg("build").current_dir(root).status().is_ok_and(|s| s.success());
        if !ok {
            println!("\n{}", ask::dim("cargo build failed; `wisp dev` will show the errors."));
        }
    }

    let cd = if name.contains(' ') { format!("cd \"{name}\"") } else { format!("cd {name}") };
    println!("\nNext:\n\n  {}\n  {}\n\nThen open {}.\n", ask::accent(&cd), ask::accent("wisp dev"), ask::bold("http://127.0.0.1:3000"));
    Ok(())
}

fn parse(args: &[String]) -> Result<Answers, String> {
    let mut a = Answers::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if arg.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--template" | "-t" => {
                let value = inline.or_else(|| args.next().cloned()).unwrap_or_default();
                a.template = Some(match value.as_str() {
                    "demo" => Template::Demo,
                    "minimal" => Template::Minimal,
                    _ => return Err(format!("unknown template `{value}` (demo or minimal)")),
                });
            }
            "--tailwind" => a.tailwind = Some(true),
            "--no-tailwind" => a.tailwind = Some(false),
            "--git" => a.git = Some(true),
            "--no-git" => a.git = Some(false),
            "--install" => a.install = Some(true),
            "--no-install" => a.install = Some(false),
            "--yes" | "-y" => a.yes = true,
            f if f.starts_with('-') => return Err(format!("unknown option `{f}`\nusage: {USAGE}")),
            _ if a.name.is_none() => a.name = Some(arg.clone()),
            _ => return Err(format!("unexpected `{arg}`\nusage: {USAGE}")),
        }
    }
    Ok(a)
}

fn answer(flag: Option<bool>, asking: bool, question: &str, default: bool) -> Result<bool, String> {
    match flag {
        Some(v) => Ok(v),
        None if asking => ask::yes(question, default),
        None => Ok(default),
    }
}

/// The package name Cargo will accept for the app's directory.
fn crate_name(root: &Path) -> Result<String, String> {
    // `.` and `..` have no name of their own; the directory they mean does.
    let dir = if root.file_name().is_some() { root.to_path_buf() } else { root.canonicalize().unwrap_or_default() };
    let base = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let name: String = base.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let name = name.trim_matches('-');
    match name.chars().next() {
        None => Err(format!("cannot make a package name from `{}`", root.display())),
        Some(c) if c.is_ascii_digit() => Ok(format!("app-{name}")),
        Some(_) => Ok(name.to_string()),
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("exited with {status}")) }
}

fn write(root: &Path, crate_name: &str, template: Template, tailwind: bool) -> Result<(), String> {
    let (wisp, wisp_build) = wisp_source();
    let cargo_toml = CARGO_TOML.replace("{name}", crate_name).replace("{wisp}", &wisp).replace("{wisp-build}", &wisp_build);

    let (files, css): (&[(&str, &str)], &str) = match template {
        Template::Demo => (&DEMO, DEMO_CSS),
        Template::Minimal => (&MINIMAL, MINIMAL_CSS),
    };
    let css = if tailwind { with_tailwind(css) } else { css.to_string() };
    let common = [
        ("Cargo.toml", cargo_toml.as_str()),
        (".gitignore", "/target\n/.wisp\n"),
        ("build.rs", BUILD_RS),
        ("src/main.rs", MAIN_RS),
        ("src/app.html", APP_HTML),
        ("src/app.css", &css),
        ("static/favicon.svg", FAVICON),
    ];
    for (rel, text) in common.iter().chain(files) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("files are inside the app")).map_err(|e| format!("{}: {e}", root.display()))?;
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Where new apps get Wisp from, until it is on crates.io: the `wisp` and
/// `wisp-build` dependency lines. A `wisp` built from a clone (`cargo install
/// --path`) points apps at that clone, so changes to Wisp reach them at once.
/// One installed with `cargo install --git` points them at GitHub, since the
/// checkout Cargo built it from is Cargo's to delete.
fn wisp_source() -> (String, String) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let repo = repo.canonicalize().unwrap_or(repo).to_string_lossy().replace('\\', "/");
    let repo = repo.strip_prefix("//?/").unwrap_or(&repo);
    let from_git = repo.contains("/git/checkouts/");
    let dep = |name: &str| {
        if from_git { format!("{name} = {{ git = \"{REPO}\" }}") } else { format!("{name} = {{ path = \"{repo}/crates/{name}\" }}") }
    };
    (dep("wisp"), dep("wisp-build"))
}

/// Tailwind first, then the template's own styles in its base layer, so a
/// utility class on an element still beats them.
fn with_tailwind(css: &str) -> String {
    let mut out = String::from("@import \"tailwindcss\";\n\n@layer base {\n");
    for line in css.lines() {
        if !line.is_empty() {
            out.push_str("  ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

// ---- files --------------------------------------------------------------------
//
// The demo template is examples/demo itself, so the two can never drift.

macro_rules! demo {
    ($path:literal) => {
        ($path, include_str!(concat!("../../../examples/demo/", $path)))
    };
}

const BUILD_RS: &str = include_str!("../../../examples/demo/build.rs");
const MAIN_RS: &str = include_str!("../../../examples/demo/src/main.rs");
const APP_HTML: &str = include_str!("../../../examples/demo/src/app.html");
const FAVICON: &str = include_str!("../../../examples/demo/static/favicon.svg");
const DEMO_CSS: &str = include_str!("../../../examples/demo/src/app.css");

const DEMO: [(&str, &str); 10] = [
    demo!("src/routes/+layout.wisp"),
    demo!("src/routes/+layout.rs"),
    demo!("src/routes/+page.wisp"),
    demo!("src/routes/+page.rs"),
    demo!("src/routes/+error.wisp"),
    demo!("src/routes/about/+page.wisp"),
    demo!("src/routes/wisple/+page.wisp"),
    demo!("src/routes/wisple/+page.rs"),
    demo!("src/routes/wisple/words.txt"),
    demo!("src/routes/wisple/how-to-play/+page.wisp"),
];

const MINIMAL: [(&str, &str); 3] = [
    ("src/routes/+layout.wisp", "<main>\n  {@render children()}\n</main>\n"),
    (
        "src/routes/+page.wisp",
        r#"<wisp:head><title>Home</title></wisp:head>

<h1>Welcome to Wisp</h1>
<p>Edit <code>src/routes/+page.wisp</code> and save to see it change.</p>
"#,
    ),
    ("src/routes/+error.wisp", "<wisp:head><title>{status}</title></wisp:head>\n\n<h1>{status}</h1>\n<p>{message}</p>\n"),
];

const MINIMAL_CSS: &str = r#":root {
  color-scheme: dark;
}

body {
  margin: 0;
  background: #141414;
  color: #fafafa;
  font: 1rem/1.5 system-ui, sans-serif;
}

main {
  max-width: 42rem;
  margin: 0 auto;
  padding: 4rem 1rem;
}

a {
  color: #9f8ce7;
}
"#;

const CARGO_TOML: &str = r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
{wisp}

[build-dependencies]
{wisp-build}

# Fast rebuilds: your crate stays unoptimized, dependencies are optimized once.
[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
opt-level = 1

[profile.release]
codegen-units = 1
lto = "fat"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn flags() {
        let a = parse(&args("app --template=minimal --no-tailwind --git -y")).unwrap();
        assert_eq!(a.name.as_deref(), Some("app"));
        assert!(a.template == Some(Template::Minimal) && a.tailwind == Some(false) && a.git == Some(true) && a.yes);
        assert!(parse(&args("app -t demo")).unwrap().template == Some(Template::Demo));
        assert!(parse(&args("app --template vue")).is_err());
        assert!(parse(&args("app --nope")).is_err());
        assert!(parse(&args("app other")).is_err());
    }

    #[test]
    fn package_names() {
        assert_eq!(crate_name(Path::new("My App")).unwrap(), "my-app");
        assert_eq!(crate_name(Path::new("apps/2048")).unwrap(), "app-2048");
        assert!(crate_name(Path::new("___")).is_err());
    }

    #[test]
    fn tailwind_wraps_template_styles() {
        let css = with_tailwind("a {\n  color: red;\n}\n\nb {}\n");
        assert_eq!(css, "@import \"tailwindcss\";\n\n@layer base {\n  a {\n    color: red;\n  }\n\n  b {}\n}\n");
    }
}
