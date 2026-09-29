//! `wisp check`: routes and templates, with no compiling.

use crate::{Dir, fail, has, new_app, wisp, write};

#[test]
fn every_template_is_valid() {
    let cwd = Dir::new("check-ok");
    for (name, flags) in [
        ("minimal", &["-t", "minimal"][..]),
        ("tailwind", &["-t", "minimal", "--tailwind"]),
        ("demo", &["-t", "demo"]),
        ("api", &["--api"]),
    ] {
        let app = new_app(&cwd, name, flags);
        let o = wisp(&app, &["check"]);
        assert!(o.ok, "{name}: {}", o.err);
        assert!(
            o.out.contains("Routes and templates are valid."),
            "{name}: {}",
            o.out
        );
        // Nothing was compiled, and nothing left behind to compile from.
        assert!(!app.join("target").exists(), "{name}");
    }
}

#[test]
fn a_broken_file_is_named_with_its_line() {
    let cwd = Dir::new("check-broken");
    let app = new_app(&cwd, "app", &["-t", "minimal"]);
    let plant = |rel: &str, text: &str, said: &str| {
        let old = std::fs::read_to_string(app.join(rel)).ok();
        write(&app, rel, text);
        let o = fail(&app, &["check"], said);
        assert!(o.out.is_empty(), "{}", o.out);
        match old {
            Some(old) => write(&app, rel, &old),
            None => std::fs::remove_dir_all(app.join(rel).parent().unwrap()).unwrap(),
        }
        assert!(wisp(&app, &["check"]).ok, "{rel} put back");
    };
    plant(
        "src/routes/+page.wisp",
        "<h1>Hi</h1>\n<p>{oops</p>\n",
        "src/routes/+page.wisp:2:4: unclosed {",
    );
    plant(
        "src/routes/+layout.wisp",
        "<main>\n{#each x}\n<slot />\n</main>\n",
        "src/routes/+layout.wisp:2:1: expected {#each <expr> as",
    );
    plant(
        "src/routes/+error.wisp",
        "<h1>{status</h1>\n",
        "src/routes/+error.wisp:1:5: unclosed {",
    );
    plant(
        "src/routes/about/+page.wisp",
        "<p>ok</p>\n{#if x}\n<p>y</p>\n",
        "src/routes/about/+page.wisp:2:1: {#if} is never closed",
    );
    plant(
        "src/routes/u/[id=nope]/+page.wisp",
        "<p>x</p>\n",
        "no param matcher `nope`: add src/params/nope.rs",
    );

    write(&app, "src/routes/+page.wisp", "<Card />\n");
    let o = fail(
        &app,
        &["check"],
        "src/routes/+page.wisp:1: no component `Card`",
    );
    has(&o.err, &["src/components"]);
    write(&app, "src/components/Card.wisp", "<div><slot /></div>\n");
    assert!(wisp(&app, &["check"]).ok);
}
