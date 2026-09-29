//! `wisp new [name]`: asks a few questions, then writes an app to start from.
//!
//! Every question has a flag, so scripts and CI can answer up front; `--yes`
//! (or no terminal to ask on) takes the defaults for the rest.

use crate::{ask, css, term};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq)]
enum Template {
    Demo,
    Minimal,
}

const TEMPLATES: [(&str, &str); 2] = [
    (
        "Demo",
        "A home page with a counter, an about page and a word game to learn from.",
    ),
    ("Minimal", "One empty page, a layout and an error page."),
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

pub fn run(args: &[String]) -> Result<(), String> {
    let mut a = parse(args)?;
    let asking = !a.yes && ask::interactive();
    if asking {
        println!("\n{}\n", term::banner());
    }

    let name = match a.name.take() {
        Some(name) => name,
        None if asking => ask::text("Where should the app go?", "my-app")?,
        None => return Err("Which folder should the app go in?\nName it: wisp new my-app".into()),
    };
    let root = Path::new(&name);
    match fs::read_dir(root).map(|mut d| d.next().is_none()) {
        Ok(true) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Ok(false) => {
            return Err(format!(
                "{name} already exists and is not empty.\nPick another name, or empty the folder first."
            ));
        }
        Err(e) => return Err(format!("Could not use {name}: {e}.\nPick another name.")),
    }
    let crate_name = crate_name(root)?;

    let template = match a.template {
        Some(t) => t,
        None if asking => {
            [Template::Demo, Template::Minimal][ask::choose("Which template?", &TEMPLATES, 0)?]
        }
        None => Template::Demo,
    };
    let tailwind = answer(a.tailwind, asking, "Add Tailwind CSS?", false)?;
    // Like `cargo new`: a repository by default, unless the app is going
    // inside one already.
    let parent = root
        .parent()
        .filter(|p| p.is_dir())
        .unwrap_or(Path::new("."));
    let has_git = git(parent, &["--version"]).is_ok();
    let git_default = has_git && git(parent, &["rev-parse", "--is-inside-work-tree"]).is_err();
    let use_git = has_git && answer(a.git, asking, "Create a git repository?", git_default)?;
    let install = answer(
        a.install,
        asking,
        "Download and compile dependencies now?",
        true,
    )?;
    if asking {
        println!();
    }

    write(root, &crate_name, template, tailwind)?;
    let label = if template == Template::Demo {
        "demo"
    } else {
        "minimal"
    };
    term::done(&format!(
        "Created {} from the {label} template.",
        term::bold(&name)
    ));

    if use_git {
        git(root, &["init", "--quiet"])
            .map_err(|e| format!("Could not create a git repository: {e}."))?;
        term::done("Created a git repository.");
    }
    if install {
        if tailwind {
            css::install()?;
        }
        term::step(
            "Compiling dependencies. The first build takes a minute; later ones take seconds.",
        );
        let ok = Command::new("cargo")
            .arg("build")
            .current_dir(root)
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            term::done("Compiled the dependencies.");
        } else {
            term::warn("The dependencies did not compile. wisp dev will show the errors.");
        }
    }

    let cd = if name.contains(' ') {
        format!("cd \"{name}\"")
    } else {
        format!("cd {name}")
    };
    println!(
        "\n{}\n\n  {}\n  {}\n\nThen open {}.\n",
        term::bold("Next Steps"),
        term::accent(&cd),
        term::accent("wisp dev"),
        term::bold("http://127.0.0.1:3000")
    );
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
                    _ => {
                        return Err(format!(
                            "There is no template called {value}.\nPick demo or minimal."
                        ));
                    }
                });
            }
            "--tailwind" => a.tailwind = Some(true),
            "--no-tailwind" => a.tailwind = Some(false),
            "--git" => a.git = Some(true),
            "--no-git" => a.git = Some(false),
            "--install" => a.install = Some(true),
            "--no-install" => a.install = Some(false),
            "--yes" | "-y" => a.yes = true,
            f if f.starts_with('-') => {
                return Err(format!(
                    "There is no option {f}.\nRun wisp --help to see the options."
                ));
            }
            _ if a.name.is_none() => a.name = Some(arg.clone()),
            _ => {
                return Err(format!(
                    "Unexpected {arg}.\nwisp new takes one name, then options."
                ));
            }
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

/// The package name Cargo will accept for the app's directory: its letters
/// and digits, lowercase, with one `-` for each run of anything else.
fn crate_name(root: &Path) -> Result<String, String> {
    // `.` and `..` have no name of their own; the directory they mean does.
    let dir = if root.file_name().is_some() {
        root.to_path_buf()
    } else {
        root.canonicalize().unwrap_or_default()
    };
    let base = dir
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let mut name = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-');
    let name = match name.chars().next() {
        None => {
            return Err(format!(
                "Could not make a package name from {}.\nUse a name with a letter or a digit in it.",
                root.display()
            ));
        }
        Some(c) if c.is_ascii_digit() => format!("app-{name}"),
        Some(_) => name.to_string(),
    };
    let why = match name.as_str() {
        "build" | "deps" | "examples" | "incremental" => {
            "Cargo uses that name for a folder of its own."
        }
        "test" => "It is the name of Rust's built-in test library.",
        "wisp" | "wisp-build" | "wisp-macros" | "wisp-cli" => "Wisp's own crates are called that.",
        _ => return Ok(name),
    };
    Err(format!(
        "An app cannot be called {name}.\n{why} Pick another name, like my-{name}."
    ))
}

/// Whether `dir` is inside a Cargo workspace: a Cargo.toml above it with a
/// `[workspace]` table. An app there builds as part of the workspace, or
/// not at all, unless its own Cargo.toml says it is a workspace of its own.
fn in_workspace(dir: &Path) -> bool {
    let Ok(dir) = std::path::absolute(dir) else {
        return false;
    };
    dir.ancestors().skip(1).any(|d| {
        let toml = fs::read_to_string(d.join("Cargo.toml")).unwrap_or_default();
        toml.lines()
            .any(|l| l.trim_start().starts_with("[workspace"))
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("exited with {status}"))
    }
}

fn write(root: &Path, crate_name: &str, template: Template, tailwind: bool) -> Result<(), String> {
    let (wisp, wisp_build) = wisp_source();
    let mut cargo_toml = CARGO_TOML
        .replace("{name}", crate_name)
        .replace("{wisp}", &wisp)
        .replace("{wisp-build}", &wisp_build);
    if in_workspace(root) {
        cargo_toml.push_str("\n# Not part of the workspace this folder is in.\n[workspace]\n");
    }

    let (files, css): (&[(&str, &str)], &str) = match template {
        Template::Demo => (&DEMO, DEMO_CSS),
        Template::Minimal => (&MINIMAL, MINIMAL_CSS),
    };
    let css = if tailwind {
        with_tailwind(css)
    } else {
        css.to_string()
    };
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
        fs::create_dir_all(path.parent().expect("files are inside the app"))
            .map_err(|e| format!("Could not create {}: {e}.", root.display()))?;
        // `create_new`: a file that appeared since the folder was found empty
        // is someone's, and stays as it is.
        fs::File::create_new(&path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| format!("Could not write {}: {e}.", path.display()))?;
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
    let repo = repo
        .canonicalize()
        .unwrap_or(repo)
        .to_string_lossy()
        .replace('\\', "/");
    let repo = repo.strip_prefix("//?/").unwrap_or(&repo);
    let from_git = repo.contains("/git/checkouts/");
    let dep = |name: &str| {
        if from_git {
            format!("{name} = {{ git = \"{REPO}\" }}")
        } else {
            format!("{name} = {{ path = \"{repo}/crates/{name}\" }}")
        }
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
        (
            $path,
            include_str!(concat!("../../../examples/demo/", $path)),
        )
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
    (
        "src/routes/+layout.wisp",
        "<main>\n  {@render children()}\n</main>\n",
    ),
    (
        "src/routes/+page.wisp",
        r#"<wisp:head><title>Home</title></wisp:head>

<h1>Welcome to Wisp</h1>
<p>Edit <code>src/routes/+page.wisp</code> and save to see it change.</p>
"#,
    ),
    (
        "src/routes/+error.wisp",
        r#"<wisp:head><title>{status}</title></wisp:head>

<h1>{status}</h1>
<p>{message}</p>
<p><a href="/">Go to the Home Page</a></p>
"#,
    ),
];

const MINIMAL_CSS: &str = r#"/*
 * The app's styles. Every color and font is a token here; the rules below
 * only name them. Light by default, dark when the system is.
 */

:root {
  --paper: #f4f4f4;
  --ink: #141414;
  --ink-muted: #565656;
  --accent: #7456d6;

  --font-sans: "Open Sans Variable", "Open Sans", "Segoe UI Variable", "Segoe UI", -apple-system,
    BlinkMacSystemFont, system-ui, sans-serif;
  --font-mono: "Cascadia Code", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace;

  color-scheme: light;
}

@media (prefers-color-scheme: dark) {
  :root {
    --paper: #141414;
    --ink: #fafafa;
    --ink-muted: #a8a8a8;
    --accent: #9f8ce7;

    color-scheme: dark;
  }
}

body {
  margin: 0;
  background: var(--paper);
  color: var(--ink);
  font: 400 1rem/1.5 var(--font-sans);
}

main {
  max-width: 42rem;
  margin: 0 auto;
  padding: 4rem 1rem;
}

h1 {
  margin: 0 0 0.5rem;
  font-size: 1.5rem;
  font-weight: 600;
  line-height: 2rem;
  letter-spacing: -0.025em;
}

p {
  margin: 0 0 1rem;
  color: var(--ink-muted);
}

a {
  color: var(--accent);
  text-underline-offset: 0.2em;
}

code {
  font: 0.875em var(--font-mono);
}

:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
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
        assert!(
            a.template == Some(Template::Minimal)
                && a.tailwind == Some(false)
                && a.git == Some(true)
                && a.yes
        );
        assert!(parse(&args("app -t demo")).unwrap().template == Some(Template::Demo));
        assert!(parse(&args("app --template vue")).is_err());
        assert!(parse(&args("app --nope")).is_err());
        assert!(parse(&args("app other")).is_err());
    }

    #[test]
    fn package_names() {
        assert_eq!(crate_name(Path::new("My App")).unwrap(), "my-app");
        assert_eq!(crate_name(Path::new("apps/2048")).unwrap(), "app-2048");
        assert_eq!(crate_name(Path::new("café app")).unwrap(), "caf-app");
        assert_eq!(crate_name(Path::new("_my__app_")).unwrap(), "my-app");
        assert!(crate_name(Path::new("___")).is_err());
        for taken in [
            "build",
            "Deps",
            "examples",
            "incremental",
            "test",
            "wisp",
            "wisp_build",
            "wisp-macros",
            "wisp cli",
        ] {
            assert!(crate_name(Path::new(taken)).is_err(), "{taken}");
        }
    }

    #[test]
    fn standalone_inside_a_workspace() {
        let root = std::env::temp_dir().join(format!("wisp-new-{}", std::process::id()));
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("outside/Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::create_dir_all(root.join("inside")).unwrap();
        fs::write(
            root.join("inside/Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();
        assert!(!in_workspace(&root.join("outside/app")));
        assert!(in_workspace(&root.join("inside/apps/app")));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tailwind_wraps_template_styles() {
        let css = with_tailwind("a {\n  color: red;\n}\n\nb {}\n");
        assert_eq!(
            css,
            "@import \"tailwindcss\";\n\n@layer base {\n  a {\n    color: red;\n  }\n\n  b {}\n}\n"
        );
    }
}
