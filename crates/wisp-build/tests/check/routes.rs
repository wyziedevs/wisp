//! What the route scan refuses: names, shapes, and routes that collide.

use crate::common::fails;

const HOME: (&str, &str) = ("src/routes/+page.wisp", "x");

#[test]
fn route_files_are_named_with_a_plus() {
    fails(&[
        (
            "page without +",
            &[("src/routes/about/page.wisp", "x")],
            &[
                "src/routes/about/page.wisp: route files start with `+`",
                "did you mean `+page.wisp`?",
            ],
        ),
        (
            "any .wisp without +",
            &[("src/routes/foo.wisp", "x")],
            &["src/routes/foo.wisp:", "did you mean `+foo.wisp`?"],
        ),
        (
            "layout.rs without +",
            &[HOME, ("src/routes/layout.rs", "")],
            &["src/routes/layout.rs:", "did you mean `+layout.rs`?"],
        ),
        (
            "server.rs without +",
            &[("src/routes/api/server.rs", "")],
            &["src/routes/api/server.rs:", "did you mean `+server.rs`?"],
        ),
        (
            "unknown + file",
            &[HOME, ("src/routes/+pagee.wisp", "x")],
            &[
                "src/routes/+pagee.wisp: unknown route file",
                "expected one of +page.wisp, +page.rs",
            ],
        ),
        (
            "+page.rs alone",
            &[("src/routes/x/+page.rs", "")],
            &["src/routes/x: +page.rs needs a +page.wisp next to it"],
        ),
        (
            "+page.js alone",
            &[("src/routes/x/+page.js", "")],
            &["src/routes/x: +page.js needs a +page.wisp next to it"],
        ),
        (
            "+layout.rs alone, at the root",
            &[("src/routes/+layout.rs", "")],
            &["src/routes: +layout.rs needs a +layout.wisp next to it"],
        ),
    ]);
}

#[test]
fn segments_are_checked() {
    fails(&[
        (
            "reserved _app",
            &[("src/routes/_app/+page.wisp", "x")],
            &["src/routes/_app: /_app is reserved for Wisp's own files"],
        ),
        (
            "reserved _wisp",
            &[("src/routes/_wisp/+page.wisp", "x")],
            &["src/routes/_wisp: /_wisp is reserved"],
        ),
        (
            "reserved under a group",
            &[("src/routes/(g)/_app/+page.wisp", "x")],
            &["src/routes/(g)/_app: /_app is reserved"],
        ),
        (
            "empty group",
            &[("src/routes/()/+page.wisp", "x")],
            &["src/routes/(): `()` is not a group", "like `(marketing)`"],
        ),
        (
            "nested parentheses",
            &[("src/routes/(a(b))/+page.wisp", "x")],
            &["src/routes/(a(b)): `(a(b))` is not a group"],
        ),
        (
            "static text around a param",
            &[("src/routes/a[b]/+page.wisp", "x")],
            &[
                "src/routes/a[b]:",
                "mixed static and dynamic text in one segment",
            ],
        ),
        (
            "unbalanced brackets",
            &[("src/routes/[a/+page.wisp", "x")],
            &["src/routes/[a:", "mixed static and dynamic text"],
        ),
        (
            "param name starts with a digit",
            &[("src/routes/[1x]/+page.wisp", "x")],
            &["src/routes/[1x]: `1x` is not a valid parameter name"],
        ),
        (
            "param name with a dash",
            &[("src/routes/[a-b]/+page.wisp", "x")],
            &["`a-b` is not a valid parameter name"],
        ),
        (
            "empty optional",
            &[("src/routes/[[]]/+page.wisp", "x")],
            &["src/routes/[[]]: `` is not a valid parameter name"],
        ),
        (
            "bad matcher name",
            &[("src/routes/[a=1]/+page.wisp", "x")],
            &["src/routes/[a=1]: `1` is not a valid matcher name"],
        ),
        (
            "matcher on rest",
            &[("src/routes/[...r=int]/+page.wisp", "x")],
            &["src/routes/[...r=int]: matchers go on [name=matcher]"],
        ),
    ]);
}

#[test]
fn routes_have_limits() {
    let deep = format!("src/routes/{}+page.wisp", "a/".repeat(33));
    let shallow = format!("src/routes/{}+page.wisp", "a/".repeat(32));
    let nine = format!(
        "src/routes/{}+page.wisp",
        "abcdefghi"
            .chars()
            .map(|c| format!("[{c}]/"))
            .collect::<String>()
    );
    let two_rest = "src/routes/[...a]/x/[...b]/+page.wisp";
    fails(&[
        (
            "33 segments",
            &[(deep.as_str(), "x")],
            &[
                "deeper than 32 segments, which no request reaches",
                "src/routes/a/a/a",
            ],
        ),
        (
            "9 params",
            &[(nine.as_str(), "x")],
            &["/[a]/[b]/[c]/[d]/[e]/[f]/[g]/[h]/[i]: more than 8 parameters"],
        ),
        (
            "two rests",
            &[(two_rest, "x")],
            &["/[...a]/x/[...b]: at most one [...rest] segment per route"],
        ),
        (
            "a param twice",
            &[("src/routes/[a]/x/[a]/+page.wisp", "x")],
            &["/[a]/x/[a]: parameter `a` appears twice"],
        ),
        (
            "an optional twice",
            &[("src/routes/[a]/[[a]]/+page.wisp", "x")],
            &["parameter `a` appears twice"],
        ),
    ]);
    // 32 is the limit, so 32 is fine.
    crate::common::passes(&[(shallow.as_str(), "x")]);
}

#[test]
fn matchers_must_exist() {
    fails(&[
        (
            "no such matcher",
            &[("src/routes/[id=zip]/+page.wisp", "x")],
            &[
                "src/routes/[id=zip]: no param matcher `zip`",
                "add src/params/zip.rs with `fn matches(s: &str) -> bool`",
            ],
        ),
        (
            "no such matcher, optional",
            &[("src/routes/a/[[id=zip]]/+page.wisp", "x")],
            &["src/routes/a/[[id=zip]]: no param matcher `zip`"],
        ),
        (
            "matcher file without matches",
            &[
                ("src/routes/[id=zip]/+page.wisp", "x"),
                ("src/params/zip.rs", "fn other() {}"),
            ],
            &["src/params/zip.rs: a param matcher is `fn matches(s: &str) -> bool`"],
        ),
        (
            "matcher file with a late inner attribute",
            &[
                ("src/routes/[id=zip]/+page.wisp", "x"),
                (
                    "src/params/zip.rs",
                    "fn matches(s: &str) -> bool { true }\n#![allow(dead_code)]",
                ),
            ],
            &["src/params/zip.rs:2: `//!` docs and `#![…]` attributes go at the top"],
        ),
    ]);
    // `int` needs no file.
    crate::common::passes(&[("src/routes/[id=int]/+page.wisp", "{id}")]);
}

#[test]
fn colliding_routes_are_refused() {
    fails(&[
        (
            "two params",
            &[
                ("src/routes/[a]/+page.wisp", "x"),
                ("src/routes/[b]/+page.wisp", "x"),
            ],
            &["routes /[a] (src/routes/[a]) and /[b] (src/routes/[b]) match the same URLs"],
        ),
        (
            "a group hides a directory",
            &[
                ("src/routes/x/+page.wisp", "x"),
                ("src/routes/(g)/x/+page.wisp", "x"),
            ],
            &["/x (src/routes/", "match the same URLs"],
        ),
        (
            "an optional covers a static",
            &[
                ("src/routes/docs/+page.wisp", "x"),
                ("src/routes/docs/[[lang]]/+page.wisp", "x"),
            ],
            &["routes /docs", "match the same URLs"],
        ),
        (
            "a page and a server in different groups",
            &[
                ("src/routes/(a)/x/+page.wisp", "x"),
                ("src/routes/(b)/x/+page.wisp", "x"),
            ],
            &["match the same URLs"],
        ),
        (
            "an optional and a rest",
            &[("src/routes/[[a]]/+page.wisp", "x"), HOME],
            &["match the same URLs"],
        ),
        (
            "two rests",
            &[
                ("src/routes/[...a]/+page.wisp", "x"),
                ("src/routes/[...b]/+page.wisp", "x"),
            ],
            &["/[...a]", "/[...b]", "match the same URLs"],
        ),
    ]);
    // A matcher tells them apart, and so does a static segment.
    crate::common::passes(&[
        ("src/routes/[a=int]/+page.wisp", "{a}"),
        ("src/routes/[b]/+page.wisp", "{b}"),
        ("src/routes/x/+page.wisp", "x"),
        ("src/routes/[...r]/+page.wisp", "{r}"),
    ]);
}

#[test]
fn an_unreadable_directory_or_file_is_reported() {
    // A file where a directory of routes should be.
    let p = crate::common::Project::new(&[("src/routes", "not a directory")]);
    assert!(wisp_build::check(p.root()).is_ok());
    // No `src` at all.
    let p = crate::common::Project::new(&[]);
    std::fs::remove_dir(p.root().join("src")).unwrap();
    assert!(wisp_build::check(p.root()).is_ok());
    // A template that is not text.
    let p = crate::common::Project::new(&[HOME]);
    std::fs::write(
        p.root().join("src/routes/+page.wisp"),
        [0xff, 0xfe, 0x00, 0x9f],
    )
    .unwrap();
    let e = wisp_build::check(p.root()).unwrap_err();
    assert!(e.contains("+page.wisp") && e.contains("UTF-8"), "{e}");
}
