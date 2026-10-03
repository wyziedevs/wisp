//! Every way to give a command the wrong arguments, and what it says back.

use crate::{Dir, fail, has, new_app, wisp};
use std::fs;

const BUILD_USAGE: &str = "wisp build takes --static";

#[test]
fn build_arguments() {
    let cwd = Dir::new("build-args");
    for (args, said) in [
        (&["build", "--nope"][..], "There is no option --nope."),
        (&["build", "dist"], "Unexpected dist."),
        (&["build", "--out"], "--out needs a folder."),
        (&["build", "--static", "--out="], "--out needs a folder."),
        (&["build", "-o"], "-o needs a folder."),
        (&["build", "--target"], "--target needs a host."),
        (&["build", "-t"], "-t needs a host."),
        (
            &["build", "--target=node", "--static"],
            "--target <host> goes alone",
        ),
        (
            &["build", "--docker", "-t", "deno"],
            "--target <host> goes alone",
        ),
        (
            &["build", "--out", "site"],
            "--out goes with --static, --target or --client.",
        ),
        (
            &["build", "--docker", "--out=site"],
            "--out goes with --static, --target or --client.",
        ),
        (&["build", "--client"], "--client takes ts"),
        (&["build", "--client", "js"], "--client takes ts"),
        (
            &["build", "--client", "ts", "--static"],
            "--client ts goes alone",
        ),
        (
            &["build", "--client=ts", "--docker"],
            "--client ts goes alone",
        ),
        (&["build", "--force"], "--force goes with --docker."),
        (
            &["build", "--static", "--force"],
            "--force goes with --docker.",
        ),
    ] {
        let o = fail(&cwd, args, said);
        has(&o.err, &[BUILD_USAGE]);
    }
}

#[test]
fn target_error_lists_the_hosts() {
    let cwd = Dir::new("targets");
    fail(
        &cwd,
        &["build", "--target=", "-o", "x"],
        "There is no target .",
    );
    let o = fail(
        &cwd,
        &["build", "--target", "heroku"],
        "There is no target heroku.",
    );
    has(
        &o.err,
        &["cloudflare, deno, vercel, netlify, node, bun, lambda, native, static and docker."],
    );
}

#[test]
fn dev_arguments() {
    let cwd = Dir::new("dev-args");
    for (args, said) in [
        (&["dev", "--port"][..], "--port needs a port number."),
        (&["dev", "-p"], "-p needs a port number."),
        (&["dev", "--port", "x"], "x is not a port number."),
        (&["dev", "--port=70000"], "70000 is not a port number."),
        (&["dev", "--port="], " is not a port number."),
        (&["dev", "--prot", "3000"], "There is no option --prot."),
        (&["dev", "3000"], "Unexpected 3000."),
    ] {
        fail(&cwd, args, said);
    }
}

#[test]
fn check_takes_only_types() {
    let cwd = Dir::new("check-args");
    fail(
        &cwd,
        &["check", "--fix"],
        "Unexpected --fix.\n    wisp check takes --types, to check TypeScript with tsc.",
    );
}

#[test]
fn new_arguments() {
    let cwd = Dir::new("new-args");
    for (args, said) in [
        (&["new", "a", "b"][..], "Unexpected b."),
        (&["new", "--nope"], "There is no option --nope."),
        (&["new", "a", "-z"], "There is no option -z."),
        (
            &["new", "a", "--template", "vue"],
            "There is no template called vue.",
        ),
        (
            &["new", "a", "--template=vue"],
            "Pick demo, minimal or api.",
        ),
        (
            &["new", "a", "-t"],
            "-t needs a template: demo, minimal or api.",
        ),
        // Nothing to ask, and no name.
        (&["new", "-y"], "Which folder should the app go in?"),
        (&["new"], "Name it: wisp new my-app"),
    ] {
        fail(&cwd, args, said);
    }
    assert!(
        fs::read_dir(&*cwd).unwrap().next().is_none(),
        "nothing was created"
    );
}

#[test]
fn outside_an_app_the_commands_say_so() {
    let cwd = Dir::new("no-app");
    for args in [
        &["build"][..],
        &["build", "--static"],
        &["build", "--docker"],
        &["build", "--target", "node"],
        &["dev"],
        &["dev", "--port", "3001"],
        &["check"],
    ] {
        let o = fail(&cwd, args, "There is no Wisp app here.");
        has(&o.err, &["Cargo.toml and build.rs", "wisp new"]);
    }
    // A Cargo.toml alone is not an app.
    fs::write(cwd.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    fail(&cwd, &["check"], "There is no Wisp app here.");
    assert!(wisp(&cwd, &["--help"]).ok);
}

#[test]
fn out_cannot_be_part_of_the_app() {
    let cwd = Dir::new("out-in-app");
    let app = new_app(&cwd, "app", &["-t", "minimal"]);
    for args in [
        &["build", "--static", "--out", "static"][..],
        &["build", "--static", "--out=."],
        &["build", "--target", "node", "--out", "src/x"],
    ] {
        fail(&app, args, "is part of the app.");
    }
    assert!(app.join("static/favicon.svg").metadata().unwrap().len() > 0);
}

#[test]
fn names_cargo_would_refuse_are_refused_before_writing() {
    let cwd = Dir::new("bad-names");
    for name in ["fn", "nul"] {
        fail(&cwd, &["new", name, "-y"], "An app cannot be called");
    }
    assert!(fs::read_dir(&*cwd).unwrap().next().is_none());
}
