//! `wisp new`: the files each template writes, and when it refuses.

use crate::{Dir, Out, fail, fake_tailwind, has, new_app, read, repo, tree, wisp, wisp_env, write};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MINIMAL: [&str; 14] = [
    ".cursor/rules/wisp.mdc",
    ".github/copilot-instructions.md",
    ".gitignore",
    "AGENTS.md",
    "CLAUDE.md",
    "Cargo.toml",
    "build.rs",
    "src/app.css",
    "src/app.html",
    "src/main.rs",
    "src/routes/+error.wisp",
    "src/routes/+layout.wisp",
    "src/routes/+page.wisp",
    "static/favicon.svg",
];

/// Where a `name = { path = "..." }` dependency in Cargo.toml points.
fn dependency(cargo_toml: &str, name: &str) -> PathBuf {
    let line = cargo_toml
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name} = {{ path = \"")))
        .unwrap_or_else(|| panic!("no {name} dependency in {cargo_toml}"));
    PathBuf::from(line.split('"').next().unwrap())
}

fn is_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[test]
fn minimal_writes_these_files() {
    let cwd = Dir::new("minimal");
    let o = wisp(
        &cwd,
        &[
            "new",
            "site",
            "-y",
            "--no-git",
            "--no-install",
            "--template",
            "minimal",
        ],
    );
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "Created site from the minimal template.",
            "Next Steps",
            "cd site",
            "wisp dev",
            "Then open http://127.0.0.1:3000.",
        ],
    );
    assert!(!o.out.contains("git repository") && !o.out.contains("Compiling"));

    let app = cwd.join("site");
    assert_eq!(tree(&app), MINIMAL);
    assert!(!app.join(".git").exists());
    assert_eq!(
        read(&app, ".gitignore"),
        "/target\n/.wisp\n/data\n.env\n.env.*\n"
    );

    let toml = read(&app, "Cargo.toml");
    has(
        &toml,
        &[
            "name = \"site\"",
            "edition = \"2024\"",
            "[dependencies]",
            "[build-dependencies]",
            "[profile.release]",
        ],
    );
    assert!(!toml.contains("[workspace]"));
    let wisp = dependency(&toml, "wisp");
    let build = dependency(&toml, "wisp-build");
    assert!(wisp.join("Cargo.toml").is_file() && build.join("Cargo.toml").is_file());
    assert!(wisp.ends_with("crates/wisp") && build.ends_with("crates/wisp-build"));

    // The scaffolding is the demo's, so the two cannot drift.
    let demo = repo().join("examples/demo");
    for rel in [
        "build.rs",
        "src/main.rs",
        "src/app.html",
        "static/favicon.svg",
    ] {
        assert_eq!(read(&app, rel), read(&demo, rel), "{rel}");
    }
    has(&read(&app, "build.rs"), &["wisp_build"]);
    has(&read(&app, "src/main.rs"), &["wisp::main!"]);
    has(&read(&app, "src/app.html"), &["%wisp.head%", "%wisp.body%"]);
    has(&read(&app, "src/routes/+layout.wisp"), &["<slot />"]);
    has(
        &read(&app, "src/routes/+page.wisp"),
        &["<h1>Welcome to Wisp</h1>"],
    );
    has(
        &read(&app, "src/routes/+error.wisp"),
        &["{status}", "{message}"],
    );
    assert!(!read(&app, "src/app.css").contains("tailwindcss"));

    // The reference for AI agents: the repository's, less its part for
    // work on Wisp, and a pointer to it for each agent.
    let agents = read(&app, "AGENTS.md");
    has(
        &agents,
        &["## Actions (form posts)", "wisp update-docs keeps them"],
    );
    assert!(!agents.contains("Carmack"));
    for rel in &AGENT_FILES[1..] {
        has(&read(&app, rel), &["AGENTS.md"]);
    }
}

#[test]
fn update_docs_keeps_the_apps_notes() {
    let cwd = Dir::new("update-docs");
    let app = new_app(&cwd, "app", &["--template", "minimal"]);
    let fresh = read(&app, "AGENTS.md");
    write(
        &app,
        "AGENTS.md",
        &fresh.replace("## Actions", "## Old").replace(
            "keeps them. -->
",
            "keeps them. -->
Use tabs.
",
        ),
    );
    write(
        &app,
        "CLAUDE.md",
        "Mine.
",
    );
    fs::remove_file(app.join(".cursor/rules/wisp.mdc")).unwrap();
    let o = wisp(&app, &["update-docs"]);
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["Wrote AGENTS.md, .cursor/rules/wisp.mdc."]);
    assert_eq!(
        read(&app, "AGENTS.md"),
        format!(
            "{fresh}Use tabs.
"
        )
    );
    assert_eq!(
        read(&app, "CLAUDE.md"),
        "Mine.
"
    );
    has(&wisp(&app, &["update-docs"]).out, &["up to date"]);
    // Without the line that ends the reference, its notes could be lost.
    write(
        &app,
        "AGENTS.md",
        "My notes.
",
    );
    let o = wisp(&app, &["update-docs"]);
    assert!(!o.ok && o.err.contains("end-of-reference"), "{}", o.err);
    assert_eq!(
        read(&app, "AGENTS.md"),
        "My notes.
"
    );
}

/// The files for AI agents every app gets.
const AGENT_FILES: [&str; 4] = [
    "AGENTS.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    ".cursor/rules/wisp.mdc",
];

/// Every file except the ones a template generates is the example's own.
fn assert_is_example(app: &Path, example: &str, generated: &[&str]) {
    let example = repo().join("examples").join(example);
    for rel in tree(app) {
        if !generated.contains(&rel.as_str()) && !AGENT_FILES.contains(&rel.as_str()) {
            assert_eq!(read(app, &rel), read(&example, &rel), "{rel}");
        }
    }
}

#[test]
fn demo_is_the_demo_example() {
    let cwd = Dir::new("demo");
    let app = new_app(&cwd, "demo-copy", &["--template=demo"]);
    let files = tree(&app);
    for rel in [
        "src/routes/+page.wisp",
        "src/routes/about/+page.wisp",
        "src/routes/wisple/+page.wisp",
        "src/routes/wisple/+page.rs",
        "src/routes/wisple/words.txt",
        "src/routes/wisple/how-to-play/+page.wisp",
        "src/app.css",
        "static/favicon.svg",
    ] {
        assert!(files.iter().any(|f| f == rel), "{rel} in {files:?}");
    }
    assert_is_example(&app, "demo", &["Cargo.toml", ".gitignore"]);
    assert!(read(&app, "Cargo.toml").contains("name = \"demo-copy\""));
}

#[test]
fn no_template_asked_gives_the_demo() {
    let cwd = Dir::new("default");
    let app = new_app(&cwd, "app", &[]);
    assert!(app.join("src/routes/wisple/+page.wisp").is_file());
}

#[test]
fn api_is_the_api_example_in_every_spelling() {
    let cwd = Dir::new("api");
    let first = new_app(&cwd, "one", &["--api"]);
    let files = tree(&first);
    for rel in [
        "Cargo.toml",
        "build.rs",
        "src/main.rs",
        "src/hooks.rs",
        "src/routes/+server.rs",
        "src/routes/healthz/+server.rs",
        "src/routes/api/notes/+server.rs",
        "src/routes/api/events/+server.rs",
        "src/routes/api/chat/+server.rs",
    ] {
        assert!(files.iter().any(|f| f == rel), "{rel} in {files:?}");
    }
    // No pages to wrap, style or serve files for.
    for gone in ["src/app.html", "src/app.css", "static/favicon.svg"] {
        assert!(!files.iter().any(|f| f == gone), "{gone}");
    }
    assert_is_example(&first, "api", &["Cargo.toml", ".gitignore", "build.rs"]);

    for (name, args) in [
        ("two", ["-t", "api"]),
        ("three", ["--template=api", "--tailwind"]),
    ] {
        let app = new_app(&cwd, name, &args);
        assert_eq!(tree(&app), files, "{name}");
    }
    let o = wisp(
        &cwd,
        &["new", "four", "-y", "--no-git", "--no-install", "--api"],
    );
    has(
        &o.out,
        &["from the API template", "http://127.0.0.1:3000/_wisp/docs"],
    );
}

#[test]
fn tailwind_imports_it_and_keeps_the_template_styles() {
    let cwd = Dir::new("tailwind");
    for (name, template) in [("m", "minimal"), ("d", "demo")] {
        let plain = new_app(
            &cwd,
            &format!("{name}-plain"),
            &["-t", template, "--no-tailwind"],
        );
        assert!(!read(&plain, "src/app.css").contains("tailwindcss"));

        let tw = new_app(&cwd, &format!("{name}-tw"), &["-t", template, "--tailwind"]);
        let css = read(&tw, "src/app.css");
        assert!(
            css.starts_with("@import \"tailwindcss\";\n\n@layer base {\n"),
            "{css}"
        );
        assert!(css.ends_with("}\n"));
        // The plain styles are all still there, indented into the layer.
        for line in read(&plain, "src/app.css")
            .lines()
            .filter(|l| !l.is_empty())
        {
            assert!(css.contains(&format!("  {line}\n")), "{line}");
        }
        assert_eq!(tree(&tw), tree(&plain));
    }
}

#[test]
fn dot_names_the_app_after_its_folder() {
    let cwd = Dir::new("dot");
    let o = wisp(
        &cwd,
        &[
            "new",
            ".",
            "-y",
            "--no-git",
            "--no-install",
            "-t",
            "minimal",
        ],
    );
    assert!(o.ok, "{}", o.err);
    // The folder's own name, made into a package name.
    let toml = read(&cwd, "Cargo.toml");
    let name = toml
        .lines()
        .find_map(|l| l.strip_prefix("name = \""))
        .unwrap();
    let name = name.trim_end_matches('"');
    assert!(
        name.starts_with("wisp-cli-test-") && name.ends_with("-dot"),
        "{name}"
    );
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "{name}"
    );
}

#[test]
fn names_become_package_names() {
    let cwd = Dir::new("names");
    for (dir, package) in [
        ("My App", "my-app"),
        ("2048", "app-2048"),
        ("_a__b_", "a-b"),
    ] {
        let o = wisp(
            &cwd,
            &[
                "new",
                dir,
                "-y",
                "--no-git",
                "--no-install",
                "-t",
                "minimal",
            ],
        );
        assert!(o.ok, "{dir}: {}", o.err);
        let toml = read(&cwd.join(dir), "Cargo.toml");
        assert!(toml.contains(&format!("name = \"{package}\"")), "{dir}");
    }
    // A space needs quotes in the shell.
    let o = wisp(
        &cwd,
        &["new", "spaced out", "-y", "--no-git", "--no-install"],
    );
    assert!(o.out.contains("cd \"spaced out\""), "{}", o.out);
    // Parent folders are made.
    let deep = new_app(&cwd, "a/b/deep", &["-t", "minimal"]);
    assert!(deep.join("Cargo.toml").is_file());
}

#[test]
fn an_empty_folder_is_used_and_a_full_one_refused() {
    let cwd = Dir::new("taken");
    fs::create_dir(cwd.join("empty")).unwrap();
    new_app(&cwd, "empty", &["-t", "minimal"]);

    write(&cwd, "full/keep.txt", "mine");
    for template in ["minimal", "demo", "api"] {
        let o = fail(
            &cwd,
            &["new", "full", "-y", "-t", template],
            "full already exists and is not empty.",
        );
        has(&o.err, &["Pick another name, or empty the folder first."]);
    }
    assert_eq!(tree(&cwd.join("full")), ["keep.txt"]);
    assert_eq!(read(&cwd, "full/keep.txt"), "mine");

    // A file is not a folder to write an app in.
    write(&cwd, "file", "x");
    fail(
        &cwd,
        &["new", "file", "-y", "--no-git", "--no-install"],
        "file",
    );
    assert_eq!(read(&cwd, "file"), "x");
}

#[test]
fn names_cargo_cannot_use_are_refused_before_anything_is_written() {
    let cwd = Dir::new("bad-names");
    for (name, why) in [
        ("build", "Cargo uses that name for a folder of its own."),
        ("deps", "Cargo uses that name for a folder of its own."),
        ("examples", "Cargo uses that name for a folder of its own."),
        (
            "incremental",
            "Cargo uses that name for a folder of its own.",
        ),
        ("Test", "It is the name of Rust's built-in test library."),
        ("wisp", "Wisp's own crates are called that."),
        ("wisp_build", "Wisp's own crates are called that."),
        ("wisp-macros", "Wisp's own crates are called that."),
        ("wisp cli", "Wisp's own crates are called that."),
    ] {
        let o = fail(
            &cwd,
            &["new", name, "-y", "--no-git", "--no-install"],
            "An app cannot be called",
        );
        has(&o.err, &[why, "Pick another name, like my-"]);
        assert!(!cwd.join(name).exists(), "{name}");
    }
    for name in ["___", "_"] {
        fail(
            &cwd,
            &["new", name, "-y"],
            "Could not make a package name from",
        );
    }
    assert!(fs::read_dir(&*cwd).unwrap().next().is_none());
}

#[test]
fn inside_a_workspace_the_app_is_its_own() {
    let cwd = Dir::new("workspace");
    write(&cwd, "ws/Cargo.toml", "[workspace]\nmembers = []\n");
    let inside = new_app(&cwd, "ws/apps/app", &["-t", "minimal"]);
    let toml = read(&inside, "Cargo.toml");
    assert!(
        toml.ends_with("\n# Not part of the workspace this folder is in.\n[workspace]\n"),
        "{toml}"
    );

    // A Cargo.toml that is a package, not a workspace, is no reason.
    write(&cwd, "pkg/Cargo.toml", "[package]\nname = \"x\"\n");
    let outside = new_app(&cwd, "pkg/app", &["-t", "minimal"]);
    assert!(!read(&outside, "Cargo.toml").contains("[workspace]"));
}

#[test]
fn git_by_default_only_outside_a_repository() {
    if !is_git() {
        return;
    }
    let cwd = Dir::new("git");
    let new = |at: &Path, name: &str, flag: &[&str]| -> Out {
        let mut args = vec!["new", name, "-y", "--no-install", "-t", "minimal"];
        args.extend_from_slice(flag);
        let o = wisp(at, &args);
        assert!(o.ok, "{}", o.err);
        o
    };

    let o = new(&cwd, "default", &[]);
    assert!(cwd.join("default/.git").is_dir() && o.out.contains("Created a git repository."));
    new(&cwd, "asked", &["--git"]);
    assert!(cwd.join("asked/.git").is_dir());
    let o = new(&cwd, "refused", &["--no-git"]);
    assert!(!cwd.join("refused/.git").exists() && !o.out.contains("git repository"));

    // Inside a repository: none, unless asked.
    let outer = cwd.join("default");
    let o = new(&outer, "inner", &[]);
    assert!(!outer.join("inner/.git").exists() && !o.out.contains("git repository"));
    new(&outer, "forced", &["--git"]);
    assert!(outer.join("forced/.git").is_dir());
}

#[test]
fn install_compiles_the_dependencies() {
    let cwd = Dir::new("install");
    let o = wisp(
        &cwd,
        &["new", "app", "-y", "--no-git", "--install", "-t", "minimal"],
    );
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &["Compiling dependencies.", "Compiled the dependencies."],
    );
    assert!(cwd.join("app/Cargo.lock").is_file());
}

#[test]
fn a_failed_install_warns_and_still_creates_the_app() {
    let cwd = Dir::new("install-fails");
    // Offline, with no downloaded crates to build from. Tailwind is asked
    // for too: its "install" is the one named in WISP_TAILWIND.
    let home = Dir::new("empty-cargo-home");
    let tailwind = fake_tailwind(&cwd, 0);
    let o = wisp_env(
        &cwd,
        &[
            "new",
            "app",
            "-y",
            "--no-git",
            "--install",
            "--tailwind",
            "-t",
            "minimal",
        ],
        &[("CARGO_HOME", &home), ("WISP_TAILWIND", &tailwind)],
    );
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "Created app from the minimal template.",
            "The dependencies did not compile. wisp dev will show the errors.",
            "Next Steps",
        ],
    );
    assert!(!o.out.contains("Compiled the dependencies."));
    assert!(cwd.join("app/Cargo.toml").is_file());
}
