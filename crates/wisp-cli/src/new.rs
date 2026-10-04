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
    Api,
}

const TEMPLATES: [(&str, &str); 3] = [
    (
        "Demo",
        "A home page with a counter, an about page and a word game to learn from.",
    ),
    ("Minimal", "One empty page, a layout and an error page."),
    (
        "API",
        "A JSON API: notes with validation, an API key, live events and docs.",
    ),
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

const REPO: &str = "https://wisp.ar0.eu";

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
        None if asking => [Template::Demo, Template::Minimal, Template::Api]
            [ask::choose("Which template?", &TEMPLATES, 0)?],
        None => Template::Demo,
    };
    // An API has no pages to style.
    let tailwind =
        template != Template::Api && answer(a.tailwind, asking, "Add Tailwind CSS?", false)?;
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
    let label = match template {
        Template::Demo => "demo",
        Template::Minimal => "minimal",
        Template::Api => "API",
    };
    term::done(&format!(
        "Created {} from the {label} template.",
        term::bold(&name)
    ));

    if use_git {
        // The app is written: a repository that did not start is no reason
        // to end in failure.
        match git(root, &["init", "--quiet"]) {
            Ok(()) => term::done("Created a git repository."),
            Err(e) => term::warn(&format!(
                "Could not create a git repository ({e}). Run git init in the app's folder."
            )),
        }
    }
    if install {
        if tailwind && let Err(e) = css::install() {
            term::warn(&format!(
                "{}\n    wisp dev tries again.",
                e.replace('\n', "\n    ")
            ));
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
    let open = match template {
        Template::Api => "http://127.0.0.1:3000/_wisp/docs",
        _ => "http://127.0.0.1:3000",
    };
    println!(
        "\n{}\n\n  {}\n  {}\n\nThen open {}.\n",
        term::bold("Next Steps"),
        term::accent(&cd),
        term::accent("wisp dev"),
        term::bold(open)
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
                let value = inline
                    .or_else(|| args.next().cloned())
                    .ok_or(format!("{flag} needs a template: demo, minimal or api."))?;
                a.template = Some(match value.as_str() {
                    "demo" => Template::Demo,
                    "minimal" => Template::Minimal,
                    "api" => Template::Api,
                    _ => {
                        return Err(format!(
                            "There is no template called {value}.\nPick demo, minimal or api."
                        ));
                    }
                });
            }
            "--api" => a.template = Some(Template::Api),
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
        "con" | "prn" | "aux" | "nul" => "Windows does not allow a file with that name.",
        n if n.len() == 4
            && (n.starts_with("com") || n.starts_with("lpt"))
            && matches!(n.as_bytes()[3], b'1'..=b'9') =>
        {
            "Windows does not allow a file with that name."
        }
        n if KEYWORDS.contains(&n) => "It is a Rust keyword.",
        "wisp" | "wisp-build" | "wisp-macros" | "wisp-shared" | "wisp-cli" => {
            "Wisp's own crates are called that."
        }
        _ => return Ok(name),
    };
    Err(format!(
        "An app cannot be called {name}.\n{why} Pick another name, like my-{name}."
    ))
}

/// Words Cargo refuses as a package name. Names are lowercase here, so the
/// ones that need a capital (`Self`) cannot come up.
const KEYWORDS: [&str; 50] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv",
    "try", "typeof", "unsized", "virtual", "yield",
];

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

    let files = match template {
        Template::Demo => DEMO,
        Template::Minimal => MINIMAL,
        Template::Api => API,
    };
    let common: [(&str, &[u8]); 2] = [
        ("Cargo.toml", cargo_toml.as_bytes()),
        (".gitignore", b"/target\n/.wisp\n/data\n"),
    ];
    let agents = AGENT_FILES
        .iter()
        .map(|(rel, text)| (*rel, text.as_bytes()));
    for (rel, bytes) in common
        .into_iter()
        .chain(agents)
        .chain(files.iter().copied())
    {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("files are inside the app"))
            .map_err(|e| format!("Could not create {}: {e}.", root.display()))?;
        let css;
        let bytes = match std::str::from_utf8(bytes) {
            Ok(text) if tailwind && rel == "src/app.css" => {
                css = with_tailwind(text);
                css.as_bytes()
            }
            _ => bytes,
        };
        // `create_new`: a file that appeared since the folder was found empty
        // is someone's, and stays as it is.
        fs::File::create_new(&path)
            .and_then(|mut f| f.write_all(bytes))
            .map_err(|e| format!("Could not write {}: {e}.", path.display()))?;
    }
    Ok(())
}

/// The Wisp reference for AI agents (the repository's AGENTS.md, less its
/// part for work on Wisp itself), and a file for each agent that reads
/// its own, pointing to it.
pub const AGENTS_MD: &str = include_str!("../templates/vendor/AGENTS.md");
const AGENT_FILES: [(&str, &str); 4] = [
    ("AGENTS.md", AGENTS_MD),
    ("CLAUDE.md", "Read @AGENTS.md: the whole Wisp reference.\n"),
    (
        ".github/copilot-instructions.md",
        "Read AGENTS.md, at the app's root: the whole Wisp reference.\n",
    ),
    (
        ".cursor/rules/wisp.mdc",
        "---\ndescription: The Wisp reference\nalwaysApply: true\n---\nRead @AGENTS.md: the whole Wisp reference.\n",
    ),
];

/// The last line of the reference in an app's AGENTS.md: what follows it
/// is the app's own, and `wisp update-docs` keeps it.
const END: &str = "<!-- End of the Wisp reference. Notes for this app go below; wisp update-docs keeps them. -->\n";

/// `wisp update-docs`: AGENTS.md becomes this Wisp's reference, keeping
/// the app's notes after it, and a pointer file that is missing is
/// written. One that is there is the app's.
pub fn update_docs(root: &Path) -> Result<(), String> {
    let path = root.join("AGENTS.md");
    let notes = match fs::read_to_string(&path) {
        Ok(old) => match old.split_once(END.trim_end()) {
            Some((_, notes)) => notes.trim_start_matches(['\r', '\n']).to_string(),
            None => {
                return Err(
                    "AGENTS.md has no end-of-reference line, so its own notes could be lost.\nMove them to another file, delete AGENTS.md, and run wisp update-docs again."
                        .into(),
                );
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Could not read AGENTS.md: {e}.")),
    };
    let mut wrote = Vec::new();
    for (rel, text) in AGENT_FILES {
        let path = root.join(rel);
        let text = if rel == "AGENTS.md" {
            format!("{AGENTS_MD}{notes}")
        } else if path.exists() {
            continue;
        } else {
            text.to_string()
        };
        if fs::read_to_string(&path).is_ok_and(|old| old == text) {
            continue;
        }
        fs::create_dir_all(path.parent().expect("files are inside the app"))
            .and_then(|()| fs::write(&path, text))
            .map_err(|e| format!("Could not write {rel}: {e}."))?;
        wrote.push(rel);
    }
    if wrote.is_empty() {
        term::done("The agent files are up to date.");
    } else {
        term::done(&format!("Wrote {}.", wrote.join(", ")));
    }
    Ok(())
}

/// Where new apps get Wisp from, until it is on crates.io: the `wisp` and
/// `wisp-build` dependency lines. A `wisp` built from a clone (`cargo install
/// --path`) points apps at that clone, so changes to Wisp reach them at once.
/// One installed with `cargo install --git` points them at the repository, since the
/// checkout Cargo built it from is Cargo's to delete.
fn wisp_source() -> (String, String) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let repo = repo
        .canonicalize()
        .unwrap_or(repo)
        .to_string_lossy()
        .replace('\\', "/");
    // Windows' extended form: `//?/C:/x` is `C:/x`, `//?/UNC/host/x` is `//host/x`.
    let repo = match repo.strip_prefix("//?/UNC/") {
        Some(share) => format!("//{share}"),
        None => repo.strip_prefix("//?/").unwrap_or(&repo).to_string(),
    };
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
// DEMO, API and MINIMAL: each template's files, written by build.rs from the
// folders in template_files.rs.

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

const CARGO_TOML: &str = r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
{wisp}

[build-dependencies]
{wisp-build}

# `cargo test --features browser` runs the tests in a headless Chrome or Edge.
[features]
browser = ["wisp/browser"]

# Dev builds near release speed, and rebuilds as fast as unoptimized ones.
[profile.dev]
debug = "line-tables-only"
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
        assert!(parse(&args("app --api")).unwrap().template == Some(Template::Api));
        assert!(parse(&args("app -t api")).unwrap().template == Some(Template::Api));
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
            "fn",
            "Self",
            "type",
            "NUL",
            "com1",
            "LPT9",
            "con",
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

    /// The copy a published crate builds from is the examples, and `refresh`
    /// brings one folder to another's files, and leaves it alone when equal.
    #[test]
    fn vendored_templates_are_the_examples() {
        use crate::template_files::{EXAMPLES, examples, read_all, refresh, vendor};
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in EXAMPLES {
            let from = examples(base).join(name);
            if from.is_dir() {
                let copy = read_all(&vendor(base).join(name)).unwrap();
                assert!(copy == read_all(&from).unwrap(), "{name}: vendor is stale");
            }
        }
        let tmp = std::env::temp_dir().join(format!("wisp-refresh-{}", std::process::id()));
        let (from, to) = (tmp.join("from"), tmp.join("to"));
        fs::create_dir_all(from.join("src")).unwrap();
        fs::create_dir_all(to.join("old")).unwrap();
        fs::write(from.join("src/a.txt"), "a").unwrap();
        fs::write(to.join("old/b.txt"), "b").unwrap();
        refresh(&from, &to).unwrap();
        assert_eq!(read_all(&to).unwrap(), read_all(&from).unwrap());
        fs::remove_dir_all(tmp).unwrap();
    }

    /// The AI reference apps get and `wisp mcp` serves is the repository's
    /// AGENTS.md and docs as they are now, and so is llms-full.txt: a stale
    /// one is written again, and the test fails until it is committed.
    #[test]
    fn ai_reference_is_the_repositorys() {
        use crate::template_files::{
            app_agents, llms_full, read_text, repo, vendor, write_if_changed,
        };
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(AGENTS_MD.ends_with(END));
        let Ok(agents) = read_text(&repo(base).join("llms/AGENTS.md")) else {
            return;
        };
        assert!(
            AGENTS_MD == app_agents(&agents),
            "templates/vendor/AGENTS.md is stale"
        );
        assert!(!AGENTS_MD.contains("<!-- repo") && !AGENTS_MD.contains("Carmack"));
        // Without the docs site checkout (WISP_DOCS_DIR) the committed copy stays.
        let Some(full) = llms_full(&repo(base)).unwrap() else {
            return;
        };
        assert!(full == read_text(&vendor(base).join("llms-full.txt")).unwrap());
        let path = repo(base).join("llms/llms-full.txt");
        if read_text(&path).ok().as_deref() != Some(full.as_str()) {
            write_if_changed(&path, &full).unwrap();
            panic!("llms-full.txt was stale; it is written again now: commit it");
        }
    }

    /// A template is exactly its folders' files: one added there reaches new
    /// apps, and nothing built or generated does.
    #[test]
    fn templates_are_their_folders() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tables = [("DEMO", DEMO), ("API", API), ("MINIMAL", MINIMAL)];
        for ((name, layers), (table_name, table)) in
            crate::template_files::TEMPLATES.iter().zip(tables)
        {
            assert_eq!(*name, table_name);
            let found = crate::template_files::files(layers, base).unwrap();
            let embedded: Vec<&str> = table.iter().map(|(rel, _)| *rel).collect();
            let wanted: Vec<&str> = found.keys().map(String::as_str).collect();
            assert_eq!(embedded, wanted, "{name}");
            for (rel, bytes) in table {
                assert_eq!(*bytes, fs::read(&found[*rel]).unwrap(), "{name} {rel}");
            }
            for (rel, _) in table {
                assert!(
                    !rel.starts_with("tests/") && !rel.contains("target/") && *rel != "Cargo.toml",
                    "{name} {rel}"
                );
            }
        }
        let paths = |t: &[(&'static str, &'static [u8])]| t.iter().map(|f| f.0).collect::<Vec<_>>();
        assert!(paths(DEMO).contains(&"src/routes/wisple/words.txt"));
        assert!(paths(API).contains(&"build.rs") && paths(API).contains(&"src/tests.rs"));
        assert_eq!(
            paths(MINIMAL),
            [
                "build.rs",
                "src/app.css",
                "src/app.html",
                "src/main.rs",
                "src/routes/+error.wisp",
                "src/routes/+layout.wisp",
                "src/routes/+page.wisp",
                "static/favicon.svg",
            ]
        );
    }
}
