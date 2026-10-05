use super::*;

#[test]
fn static_files_of_the_app_and_its_layers() {
    let d = std::env::temp_dir().join(format!("wisp-static-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    let (app, base) = (d.join("app"), d.join("base"));
    for (dir, f, s) in [
        (&base, "static/logo.svg", "base"),
        (&base, "static/fonts/a.woff2", "base"),
        (&app, "static/logo.svg", "own"),
    ] {
        fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        fs::write(dir.join(f), s).unwrap();
    }
    let toml = "[package.metadata.wisp]\nextends = [\"../base\"]\n";
    fs::write(app.join("Cargo.toml"), toml).unwrap();
    let found = static_files(&app).unwrap();
    let urls: Vec<&str> = found.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(urls, ["/logo.svg", "/fonts/a.woff2"]);
    assert_eq!(fs::read_to_string(&found[0].1).unwrap(), "own");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn config_rules_are_baked_or_refused() {
    let page = ("src/routes/docs/[...p]/+page.wisp", "<p>{p}</p>");
    let with = |name: &str, rules: &str| {
        let toml = format!(
            "[package]
name = \"a\"
[package.metadata.wisp]
{rules}
"
        );
        app(name, &[page, ("Cargo.toml", &toml)])
    };
    let none = app("rules-none", &[page]).unwrap();
    for w in ["REDIRECTS", "REWRITES", "HEADERS"] {
        assert!(!none.contains(&format!("const {w}")), "{w}");
    }
    let code = with(
        "rules-all",
        "redirects = [\"/old/[id] /docs/[id] 301\", \"/x https://x.dev/y\"]
             rewrites = [
  \"/guide/[...p] /docs/[...p]\",
]
             headers = [\"/[...p] x-frame-options: DENY\"]",
    )
    .unwrap();
    for want in [
        "const REDIRECTS: bool = true;",
        "(\"/old/[id]\", \"/docs/[id]\", 301), (\"/x\", \"https://x.dev/y\", 308)",
        "const REWRITES: bool = true;",
        "(\"/guide/[...p]\", 0, &[\"p\"])",
        "const HEADERS: bool = true;",
        "(\"/[...p]\", \"x-frame-options\", \"DENY\")",
    ] {
        assert!(
            code.contains(want),
            "{want}
{code}"
        );
    }
    for (rules, err) in [
        ("redirects = [\"/a\"]", "write `from to`"),
        ("redirects = [\"/a /b 200\"]", "the status is"),
        ("redirects = [\"a /b\"]", "starts with a `/`"),
        ("redirects = [\"/a /b/[id]\"]", "`[id]` is not in `/a`"),
        ("redirects = [\"/a //evil.example\"]", "a path of the app"),
        ("redirects = [\"/[h] https://[h].dev\"]", "host"),
        ("rewrites = [\"/g/[...p] /nope\"]", "not a route"),
        ("rewrites = [\"/g /docs/[...p]\"]", "has no `[p]`"),
        (
            "headers = [\"/a content-length: 1\"]",
            "not a header to set",
        ),
        ("headers = [\"/a\"]", "write `pattern name: value`"),
        ("redirects = [\"/a /a\"]", "goes round for ever: /a -> /a"),
        ("redirects = [\"/a /b\", \"/b /a\"]", "/a -> /b -> /a"),
        ("redirects = [\"/a/[...p] /a/b/[...p]\"]", "goes round"),
        ("redirects = [\"/a /b\", \"/b /a x\"]", "the status is"),
        ("redirects = [\"/a//b /c\"]", "empty segment"),
        (
            "redirects = [\"/a /b\" \"/c /d\"]",
            "a comma goes between strings",
        ),
        ("redirects = [\"/a /b\"", "no closing `]`"),
        ("redirects = \"/a /b\"", "a list of strings"),
        ("redirects = [\"/a /b]", "no closing quote"),
        ("redirects = [\"/a /b\\q\"]", "escapes"),
        ("headers = ['/a x: a	b']", "control character"),
        (
            "redirects = [\"/[a]/[b]/[c]/[d]/[e]/[f]/[g]/[h]/[i] /x\"]",
            "more than 8",
        ),
        ("rewrites = [\"/g/[p] /docs/[...p]\"]", "only one of"),
    ] {
        let e = with("rules-bad", rules).err().unwrap_or_default();
        assert!(e.contains(err), "{rules}: {e}");
    }
}

#[test]
fn field_paths() {
    assert!(is_field_path("data.posts"));
    assert!(is_field_path("p.tags.0"));
    assert!(!is_field_path("posts"));
    assert!(!is_field_path("data.posts.iter()"));
    assert!(!is_field_path("0..n"));
}

#[test]
fn let_conditions_borrow_places() {
    let locals = ["user".to_string()];
    let cond = |c: &str| if_condition(c, &locals);
    assert_eq!(
        cond("let Some(u) = data.user"),
        "let Some(u) = &(data.user)"
    );
    assert_eq!(cond("let Some(u) = user"), "let Some(u) = &(user)");
    assert_eq!(cond("let Some(u) = find(x)"), "let Some(u) = find(x)");
    assert_eq!(cond("let 1..=5 = n"), "let 1..=5 = n");
    assert_eq!(cond("a == b"), "a == b");
    assert_eq!(cond("letter"), "letter");
    assert_eq!(
        rust_scan::let_names(
            "let (a, mut b) = x;\nif c { let d = 1; }\nlet Some(e) = f else { return };\nlet g: Vec<u8> = h;"
        ),
        ["a", "b", "e", "g"]
    );
}

/// Build-time guard: the code generated per route (what rustc compiles on
/// every edit) stays small. One route against fifty of the shape of
/// `bench/build/gen.sh`, dev and release; budgets are about 1.3x what was
/// measured (bench/README.md, "Build times"), in lines and bytes, so the
/// test never depends on the machine.
#[test]
fn codegen_per_route_stays_small() {
    let route = |i: usize| {
        let rs = format!(
            "struct Data {{ title: String, items: Vec<u32>, flag: bool }}\n\
             fn load(cx: &mut Cx) -> Data {{ let _ = cx; Data {{ title: \"R{i}\".into(), items: vec![1], flag: true }} }}\n"
        );
        let wisp = format!(
            "<head><title>{{title}}</title></head>\n<h1>{{title}}</h1>\n\
             {{#if flag}}<p>even {i}</p>{{:else}}<p>odd</p>{{/if}}\n<ul>\n\
             {{#each items as item, k}}<li data-k=\"{{k}}\">{{item}} <a href=\"/gen/r{i}\">{{title}}</a></li>{{/each}}\n</ul>\n"
        );
        let dir = format!("src/routes/gen/r{i}");
        (
            [format!("{dir}/+page.rs"), format!("{dir}/+page.wisp")],
            [rs, wisp],
        )
    };
    let size = |n: usize, release: bool| {
        let routes: Vec<_> = (0..n).map(route).collect();
        let mut files = vec![("src/routes/+page.wisp", "<p>home</p>")];
        for (paths, texts) in &routes {
            files.push((&paths[0], &texts[0]));
            files.push((&paths[1], &texts[1]));
        }
        let code = build(&format!("per-route-{n}-{release}"), &files, release).unwrap();
        (code.lines().count(), code.len())
    };
    for (release, max_lines, max_bytes) in [(false, 100, 5_500), (true, 90, 4_600)] {
        let (l1, b1) = size(1, release);
        let (l51, b51) = size(51, release);
        let (lines, bytes) = ((l51 - l1) / 50, (b51 - b1) / 50);
        eprintln!("release {release}: {lines} lines, {bytes} bytes per route");
        assert!(
            lines <= max_lines && bytes <= max_bytes,
            "release {release}: {lines} lines and {bytes} bytes of generated code per route, over {max_lines} and {max_bytes}"
        );
    }
}

/// Generates the app made of `files` (path, contents), in a scratch
/// directory: the error if it fails, `Ok` with the code otherwise.
fn app(name: &str, files: &[(&str, &str)]) -> Result<String, String> {
    build(name, files, false)
}

/// `app`, as a release build or a dev one.
fn build(name: &str, files: &[(&str, &str)], release: bool) -> Result<String, String> {
    in_dir(name, files, |root| {
        generate(&Input {
            root,
            release,
            maps: !release,
            prerendered: None,
        })
        .map(|o| o.code)
    })
}

/// The model of the app made of `files`, or the error reading it.
fn model(name: &str, files: &[(&str, &str)]) -> Result<Model, String> {
    in_dir(name, files, |root| {
        Project::load(&Input {
            root,
            release: false,
            maps: true,
            prerendered: None,
        })
        .map(|p| p.model)
    })
}

/// `f` of a scratch directory holding `files` (path, contents).
fn in_dir<T>(name: &str, files: &[(&str, &str)], f: impl FnOnce(&Path) -> T) -> T {
    let root = std::env::temp_dir().join(format!("wisp-codegen-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, contents) in files {
        let p = root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }
    let out = f(&root);
    let _ = fs::remove_dir_all(&root);
    out
}

/// Each route's pattern and whether a request through it may wait.
fn waits(m: &Model) -> Vec<(&str, bool)> {
    m.routes
        .iter()
        .map(|r| (r.pattern.as_str(), m.route_waits(r)))
        .collect()
}

/// `field` of each row of the generated `App::ROUTES`, as written.
fn facts<'a>(code: &'a str, field: &str) -> Vec<&'a str> {
    let field = format!("{field}: ");
    (code.lines())
        .filter(|l| l.trim_start().starts_with("::wisp::rt::RouteFacts {"))
        .map(|l| {
            let v = &l[l.find(&field).unwrap() + field.len()..];
            &v[..v.find([',', ' ']).unwrap()]
        })
        .collect()
}

#[test]
fn a_route_knows_whether_a_file_may_be_at_its_paths() {
    let s = |x: &str| Seg::Static(x.into());
    let (user, param, rest) = (
        s("user"),
        Seg::Param("id".into(), None),
        Seg::Rest("r".into()),
    );
    assert!(may_match(&[&user, &param], "/user/a.png"));
    assert!(!may_match(&[&user, &param], "/static/app.js"));
    assert!(!may_match(&[&user, &param], "/user/a/b.png"));
    assert!(!may_match(&[&user], "/user/a.png"));
    assert!(may_match(&[&s("a b")], "/a%20b"));
    assert!(may_match(&[&user, &rest], "/x.css"));
    assert!(!may_match(&[], "/x.css"));
}

#[test]
fn routes_that_never_wait_are_answered_now() {
    let files = [
        ("src/routes/+page.wisp", "<p>hi</p>"),
        ("src/routes/a/+page.wisp", "<p>{data.n}</p>"),
        (
            "src/routes/a/+page.rs",
            "pub struct Data { pub n: u8 }\npub async fn load() -> Data { Data { n: 1 } }",
        ),
        (
            "src/routes/b/+server.rs",
            "pub fn get(id: u64) -> String { id.to_string() }",
        ),
        ("src/routes/c/+page.wisp", "<p>{db::n().await}</p>"),
        (
            "src/routes/d/+page.wisp",
            "---\nlet n = 1;\n---\n<p>{n}</p>",
        ),
    ];
    let m = model("now", &files).unwrap();
    assert_eq!(
        waits(&m),
        [
            ("/", false),
            ("/a", true),
            ("/b/[id=int]", false),
            ("/c", true),
            ("/d", false)
        ]
    );
    assert!(m.root_error.is_none() && !m.root_waits());
    // The table `handle`'s server reads, and unmatched paths.
    let code = app("now", &files).unwrap();
    assert_eq!(
        facts(&code, "now"),
        ["true", "false", "true", "false", "true"]
    );
    assert!(code.contains("const NOT_FOUND_NOW: bool = true;"), "{code}");
    // A `before` that waits is before every route.
    let mut hooked = files.to_vec();
    hooked.push(("src/hooks.rs", "pub async fn before(cx: &mut wisp::Cx) {}"));
    let code = app("now-hooked", &hooked).unwrap();
    assert_eq!(facts(&code, "now"), ["false"; 5]);
    assert!(!code.contains("NOT_FOUND_NOW"), "{code}");
    // An error page whose layout loads with `.await`: its routes wait,
    // and so does a path no route matches.
    let mut errors = files.to_vec();
    errors.push(("src/routes/+error.wisp", "<p>{status}</p>"));
    errors.push(("src/routes/+layout.wisp", "<slot />"));
    errors.push((
        "src/routes/+layout.rs",
        "pub struct Data;\npub async fn load() -> Data { Data }",
    ));
    let m = model("now-error", &errors).unwrap();
    assert!(m.routes.iter().all(|r| m.route_waits(r)));
    assert!(m.root_error == Some(0) && m.root_waits());
    assert!(m.layouts[0].waits && m.error_waits(0) && m.errors[0].layouts == [0]);
    let code = app("now-error", &errors).unwrap();
    assert!(!code.contains("NOT_FOUND_NOW"), "{code}");
    assert_eq!(facts(&code, "error"), ["Some(0)"; 5]);
}

#[test]
fn routes_that_may_wait_are_never_now() {
    let waits = |name: &str, files: &[(&str, &str)]| -> Vec<bool> {
        let m = model(name, files).unwrap();
        m.routes.iter().map(|r| m.route_waits(r)).collect()
    };
    let server = |src| [("src/routes/+server.rs", src)];
    // Awaits a macro may hide, and one spaced out.
    for (n, block) in [
        "let (a, b) = tokio::join!(f(), g());",
        "let a = get!(f());",
        "let a = f(). await;",
        "let a = f()./* c */await;",
    ]
    .iter()
    .enumerate()
    {
        let src = format!("---\n{block}\n---\n<p>{{a}}</p>");
        let files = [("src/routes/+page.wisp", src.as_str())];
        assert_eq!(waits(&format!("now-macro-{n}"), &files), [true], "{block}");
    }
    // An `async fn` in the module, even a helper.
    let helper = "async fn h() {}\npub fn get() -> String { String::new() }";
    assert_eq!(waits("now-helper", &server(helper)), [true]);
    // A plain `fn` whose response awaits later, in a task of its own;
    // std's macros, which cannot await; an async `init`, which runs
    // before any request.
    let ws = "fn get() -> Response { Response::websocket(|ws| async move { while let Some(m) = ws.recv().await { ws.send(m).await?; } Ok(()) }) }";
    let mut files = server(ws).to_vec();
    files.push(("src/hooks.rs", "async fn init() -> Result { Ok(()) }"));
    files.push((
        "src/routes/a/+page.wisp",
        "---\nlet a = format!(\"{}\", vec![1].len());\n---\n<p>{a}</p>",
    ));
    assert_eq!(waits("now-plain", &files), [false, false]);
    let code = app("now-plain", &files).unwrap();
    assert_eq!(facts(&code, "now"), ["true", "true"]);
}

#[test]
fn release_builds_have_an_id_for_version_skew() {
    let id = |name: &str, page: &'static str, release: bool| {
        let page = ("src/routes/+page.wisp", page);
        let code = build(name, &[page], release).unwrap();
        let at = code
            .find("name=\\\"wisp-build\\\" content=\\\"")
            .map(|i| i + 31);
        at.map(|i| code[i..i + 16].to_string())
    };
    let (a, b) = (
        id("id-a", "x", true).unwrap(),
        id("id-b", "y", true).unwrap(),
    );
    assert_ne!(a, b);
    assert_eq!(a, id("id-c", "x", true).unwrap());
    assert_eq!(id("id-dev", "x", false), None);
}

#[test]
fn reroute_costs_nothing_unless_defined() {
    let page = ("src/routes/+page.wisp", "x");
    let hooks = |src: &'static str| ("src/hooks.rs", src);
    let none = app("no-reroute", &[page, hooks("fn init() {}")]).unwrap();
    assert!(!none.contains("REROUTE"), "{none}");
    let code = app(
        "reroute",
        &[page, hooks("fn reroute(path: &str) -> &str { path }")],
    )
    .unwrap();
    for want in [
        "const REROUTE: bool = true;",
        "fn reroute(path: &str) -> &str { hooks::__call::reroute(path) }",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let err = app("reroute-bad", &[page, hooks("fn reroute(cx: &mut Cx) {}")]).err();
    assert!(
        err.unwrap()
            .contains("`reroute` is `fn reroute(path: &str) -> &str`")
    );
}

#[test]
fn routes_are_typed_functions() {
    let code = app(
        "typed",
        &[
            ("src/routes/+page.wisp", "x"),
            ("src/routes/blog/[slug]/+page.wisp", "x"),
            ("src/routes/blog/[slug]/edit-it/+page.wisp", "x"),
            ("src/routes/files/[...path]/+page.wisp", "x"),
            ("src/routes/type/[[type]]/+page.wisp", "x"),
            ("src/routes/api/+server.rs", "fn get() {}"),
            ("src/routes/blog_slug/+page.wisp", "x"),
        ],
    )
    .unwrap();
    for want in [
        "pub fn home() -> String { let mut s = String::from(\"\"); if s.is_empty()",
        "pub fn blog_slug(slug: impl ::core::fmt::Display) -> String { let mut s = String::from(\"\"); s.push_str(\"/blog\"); s.push('/'); ::wisp::rt::path_param(&mut s, &slug, false);",
        "pub fn blog_slug_edit_it(slug: impl",
        "pub fn files_path(path: impl ::core::fmt::Display) -> String { let mut s = String::from(\"\"); s.push_str(\"/files\"); s.push('/'); ::wisp::rt::path_param(&mut s, &path, true);",
        "pub fn type_type(r#type: Option<impl ::core::fmt::Display>)",
        "pub fn api() -> String",
        "pub fn blog_slug_2() -> String",
        "pub use super::routes;",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
}

#[test]
fn endpoints_check_the_origin_of_unsafe_methods() {
    let code = |rs: &'static str| {
        app("csrf", &[("src/routes/api/+server.rs", rs)])
            .unwrap()
            .replace("Method::*", "")
    };
    let check = "endpoint(cx); ::wisp::rt::check_origin(cx)?;";
    let on = code("fn get() {}\nfn post() {}\nfn delete() {}");
    assert_eq!(on.matches(check).count(), 2, "{on}");
    assert!(!code("const CSRF: bool = false;\nfn post() {}").contains("check_origin(cx)"));
    assert!(!code("const CORS: &str = \"*\";\nfn post() {}").contains("check_origin(cx)"));
}

#[test]
fn a_page_can_leave_its_layouts() {
    let files = |a: &'static str, b: &'static str| {
        [
            ("src/routes/+layout.wisp", "<slot />"),
            ("src/routes/(app)/+layout.wisp", "<slot />"),
            ("src/routes/(app)/a/+layout.wisp", "<slot />"),
            (a, "x"),
            (b, "x"),
        ]
    };
    let chains = |a: &'static str, b: &'static str| {
        let m = model("reset", &files(a, b)).unwrap();
        m.routes
            .iter()
            .map(|r| (r.pattern.clone(), r.layouts.clone()))
            .collect::<Vec<_>>()
    };
    // `@` leaves all of them, `@app` keeps up to the `(app)` one.
    let all = chains(
        "src/routes/(app)/a/+page@.wisp",
        "src/routes/(app)/a/b/+page.wisp",
    );
    assert_eq!(all[0], ("/a".to_string(), vec![]));
    assert_eq!(all[1], ("/a/b".to_string(), vec![0, 1, 2]));
    let some = chains(
        "src/routes/(app)/a/+page@app.wisp",
        "src/routes/x/+page.wisp",
    );
    assert_eq!(some[0], ("/a".to_string(), vec![0, 1]));
    let err = model(
        "reset",
        &files(
            "src/routes/(app)/a/+page@nope.wisp",
            "src/routes/y/+page.wisp",
        ),
    )
    .err()
    .unwrap();
    assert!(err.contains("resets to the layout of `nope`"), "{err}");
}

#[test]
fn layouts_and_error_pages_resolve_per_route() {
    let m = model(
        "chains",
        &[
            ("src/routes/+layout.wisp", "<slot />"),
            ("src/routes/+error.wisp", "{status}"),
            ("src/routes/+page.wisp", "x"),
            ("src/routes/blog/+layout.wisp", "<slot />"),
            (
                "src/routes/blog/+layout.rs",
                "struct Data;\nfn load() -> Data { Data }",
            ),
            ("src/routes/blog/[slug]/+page.wisp", "{slug}"),
            ("src/routes/blog/[slug]/+error.wisp", "{status}"),
            ("src/routes/docs/+page.wisp", "x"),
            ("src/components/Card.wisp", "x"),
        ],
    )
    .unwrap();
    let shape: Vec<(&str, &[usize], Option<usize>)> = (m.routes.iter())
        .map(|r| (r.pattern.as_str(), &r.layouts[..], r.error))
        .collect();
    assert_eq!(
        shape,
        [
            ("/", &[0][..], Some(0)),
            ("/blog/[slug]", &[0, 1], Some(1)),
            ("/docs", &[0], Some(0))
        ]
    );
    // Each layout and error page knows its own template, whatever else
    // comes before it (template 0 is the component).
    assert_eq!(
        m.layouts
            .iter()
            .map(|l| (l.tpl, l.load))
            .collect::<Vec<_>>(),
        [(1, false), (2, true)]
    );
    assert_eq!(m.errors.iter().map(|e| e.tpl).collect::<Vec<_>>(), [3, 4]);
    assert_eq!(
        (m.root_error, &m.errors[1].layouts[..]),
        (Some(0), &[0, 1][..])
    );
    let page = m.routes[1].page.as_ref().unwrap();
    assert_eq!((page.module.as_str(), page.tpl), ("page_1", 6));
}

#[test]
fn component_uses_are_checked() {
    let card = (
        "src/components/Card.wisp",
        "{@props title: &str, big: bool = false}\n<h2>{title}</h2>{@render children()}",
    );
    let badge = ("src/components/Badge.wisp", "<b>new</b>");
    let page = |src: &'static str| ("src/routes/+page.wisp", src);
    assert!(
        app(
            "ok",
            &[card, badge, page("<Card title=\"x\" big><Badge /></Card>")]
        )
        .is_ok()
    );
    let err = |name, src| app(name, &[card, badge, page(src)]).unwrap_err();
    assert!(
        err("unknown", "\n<Crad title=\"x\" />")
            .starts_with("src/routes/+page.wisp:2: no component `Crad`")
    );
    assert!(err("unknown", "<Crad />").contains("there are Badge, Card"));
    assert!(err("missing", "<Card />").contains("<Card> needs `title`"));
    assert!(
        err("extra", "<Card title=\"x\" titel=\"y\" />")
            .contains("no prop `titel`; it takes title, big")
    );
    assert!(err("flag", "<Card title />").contains("`title` alone means true"));
    assert!(err("children", "<Badge>hi</Badge>").contains("does not show children"));
    assert!(app("blank", &[badge, page("<Badge>\n</Badge>")]).is_ok());
    assert!(err("head", "<wisp:head><Badge /></wisp:head>").contains("cannot go in <wisp:head>"));
    assert!(
        app("name", &[("src/components/card.wisp", "x"), page("x")])
            .unwrap_err()
            .contains("such as Card.wisp")
    );
    assert!(
        app("caps", &[("src/components/UI.wisp", "x"), page("x")])
            .unwrap_err()
            .contains("such as Ui.wisp")
    );
    assert!(
        app("props", &[page("{@props a: u8}")])
            .unwrap_err()
            .contains("only components")
    );
}

#[test]
fn markup_awaits() {
    let page = |src: &'static str| ("src/routes/+page.wisp", src);
    let code = app(
            "await-ok",
            &[page(
                "---\nlet n = 1;\n---\n<h1>{db::title().await}</h1>\n\n{#each db::items(n)\n  .await as i}{i}{/each}",
            )],
        )
        .unwrap();
    for want in [
        "        let n = 1; // src/routes/+page.wisp:2",
        " let __wisp_a0 = db::title().await; // src/routes/+page.wisp:4",
        " let __wisp_a1 = db::items(n) // src/routes/+page.wisp:6",
        "  .await; // src/routes/+page.wisp:7",
        "__wisp_a0",
        "__wisp_a1",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    // No block: the page still awaits, before it renders.
    let bare = app("await-bare", &[page("<p>{db::n().await}</p>")]).unwrap();
    assert!(
        bare.contains("let __wisp_a0 = db::n().await; // src/routes/+page.wisp:1"),
        "{bare}"
    );
    let err = |name, files: &[(&str, &str)]| app(name, files).unwrap_err();
    assert_eq!(
        err("await-in", &[page("{#if ok}\n{db::n().await}{/if}")]),
        "src/routes/+page.wisp:2: `.await` in markup runs before the page renders, so it goes outside any block \
             (`{#each db::items().await as item}`); inside one, await in the `---` block and name the value"
    );
    assert!(
        err(
            "await-load",
            &[
                page("{db::n().await}"),
                (
                    "src/routes/+page.rs",
                    "struct Data;\nfn load() -> Data { Data }"
                )
            ]
        )
        .starts_with("src/routes/+page.wisp:1: `.await` in markup runs with the page's statements")
    );
    assert!(
        err(
            "await-layout",
            &[("src/routes/+layout.wisp", "{db::n().await}<slot />")]
        )
        .starts_with("src/routes/+layout.wisp:1: a layout renders without waiting")
    );
    assert!(
        err(
            "await-comp",
            &[(
                "src/components/Card.wisp",
                "{@props n: u8}\n<p>{db::n().await}</p>"
            )]
        )
        .starts_with("src/components/Card.wisp:2: a component renders without waiting")
    );
    // In a string, `.await` is text.
    let text = app("await-text", &[page("<p>{\"x.await\"}</p>")]).unwrap();
    assert!(!text.contains("__wisp_a0"), "{text}");
}

#[test]
fn server_names_in_browser_code() {
    let code = app(
        "bare-names",
        &[(
            "src/routes/+page.wisp",
            "---\nlet items = vec![1u8];\nlet guess = 2u8;\n---\n\
                 <p :text=\"items.length\"></p><p :text=\"data.guess\"></p>\n\
                 <script>let guess = data.guess\nlet n = items[0]</script>",
        )],
    )
    .unwrap();
    // `items` is sent under its own name; the script's `guess` is its
    // own, and `data.guess` the server's.
    for want in [
        "::wisp::rt::json(__b, &(items));",
        "const { data, items } = __wisp_props(__wisp_p, [\\\"data\\\", \\\"items\\\"])",
        "let guess = __wisp_s(data.v.guess)",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
}

#[test]
fn helpers_come_from_the_runtime() {
    // `matches` needs no import: every module's function is handed it.
    let live = wisp_shared::LIVE_JS;
    assert!(HELPERS.contains(" matches,"));
    assert!(live.contains("export const matches = ") && live.contains("  matches,"));
}

#[test]
fn one_title_decided_at_build() {
    // A layout's `<title>` stands when nothing inside writes one.
    let code = app(
            "one-title",
            &[
                ("src/routes/+layout.wisp", "---\nlet p = cx.path();\n---\n<head><title>{p}</title><meta name=\"a\"></head>{@render children()}"),
                ("src/routes/+page.wisp", "---\nlet n = 1;\n---\n<title>Home {n}</title><p>x</p>"),
                ("src/routes/about/+page.wisp", "---\nlet n = 1;\n---\n<p>{n}</p>"),
                ("src/routes/docs/+page.md", "---\ntitle: Docs\n---\nBody"),
            ],
        )
        .unwrap();
    for want in [
        "pub fn render<const TITLE: bool>(",
        "if TITLE {",
        "layout_0::render::<false>(",
        "layout_0::render::<true>(",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    // The Markdown page's title is the layout's to write.
    assert!(!code.contains("<title>Docs"), "{code}");
}

#[test]
fn page_blocks() {
    let page = |src: &'static str| ("src/routes/blog/[slug]/+page.wisp", src);
    let code = app(
            "block-ok",
            &[
                page("---\nlet n = slug.len();\n\n#[action]\nfn like() { cx.flash(\"x\"); }\nlet s = \"a\nb\";\n---\n<p>{n}{#each list as x}{x}{/each}</p>"),
                ("src/routes/+layout.wisp", "---\nconst A: u8 = 1;\nlet p = cx.path();\n---\n{@render children()}"),
                ("src/db.rs", "pub fn f() {}"),
                ("src/main.rs", "mod own;\nwisp::main!();"),
                ("src/own.rs", ""),
            ],
        )
        .unwrap();
    for want in [
        "let slug = cx.param(\"slug\").to_string();",
        "    let n = slug.len(); // src/routes/blog/[slug]/+page.wisp:2",
        "    fn like() { cx.flash(\"x\"); } // src/routes/blog/[slug]/+page.wisp:5",
        "        let s = \"a\nb\"; // src/routes/blog/[slug]/+page.wisp:7",
        "pub async fn like(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { Ok(::wisp::rt_traits::Answer::answer(super::like(cx)?)) }",
        "pub async fn render(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, __wrap:",
        "page_0::tpl_page_0::render(cx, __o, |__o: &mut ::wisp::Out, cx: &::wisp::Cx, __p: &dyn Fn(&mut ::wisp::Out)| layout_0::tpl_layout_0::render(__o, cx, &|__o: &mut ::wisp::Out| __p(__o))).await",
        "    const A: u8 = 1; // src/routes/+layout.wisp:2",
        "        let p = cx.path(); // src/routes/+layout.wisp:3",
        "pub mod db {",
        "Err(e) => ::wisp::rt::input::failed(cx, e)?,",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    assert!(!code.contains("pub mod own"), "{code}");
    let err = |name, files: &[(&str, &str)]| app(name, files).unwrap_err();
    assert!(
        err(
            "both",
            &[
                page("---\nlet a = 1;\n---\n"),
                ("src/routes/blog/[slug]/+page.rs", "")
            ]
        )
        .contains("and +page.rs is beside it")
    );
    assert!(
        err(
            "load",
            &[page(
                "---\nstruct Data;\nfn load() -> Data { Data }\nlet a = 1;\n---\n"
            )]
        )
        .starts_with("src/routes/blog/[slug]/+page.wisp:4: the statements of a `---` block")
    );
    assert!(
        err("open", &[page("\n---\nlet a = 1;\n")])
            .starts_with("src/routes/blog/[slug]/+page.wisp:2:1: this `---` starts")
    );
    assert!(
        err(
            "comp",
            &[page("x"), ("src/components/Card.wisp", "---\n---\nx")]
        )
        .contains("a component takes")
    );
}

#[test]
fn a_page_reading_a_live_table_listens_to_it() {
    let db = (
        "src/db.rs",
        "#[model]
struct P { t: String }
pub static POSTS: Table<P> = Table::saved().live();
pub static KEPT: Table<P> = Table::saved();",
    );
    let reads = (
        "src/routes/+page.wisp",
        "---
let n = POSTS.len();
---
<p>{n}</p>",
    );
    let code = app("live-page", &[db, reads]).unwrap();
    assert!(
        code.contains("listen('/_wisp/live/posts', invalidate)"),
        "{code}"
    );
    let other = (
        "src/routes/+page.wisp",
        "---
let n = KEPT.len();
---
<p>{n}</p>",
    );
    let code = app("live-other", &[db, other]).unwrap();
    assert!(!code.contains("/_wisp/live/"), "{code}");
    // A page with browser code of its own listens too.
    let scripted = (
        "src/routes/+page.wisp",
        "---
let n = POSTS.len();
---
<p>{n}</p>
<script>let k = 1;</script>",
    );
    let code = app("live-script", &[db, scripted]).unwrap();
    assert!(code.contains("/_wisp/live/posts"), "{code}");
}

#[test]
fn after_and_report_cost_nothing_unless_defined() {
    let page = ("src/routes/+page.wisp", "x");
    let hooks = |src: &'static str| ("src/hooks.rs", src);
    let none = app("no-after", &[page, hooks("fn init() {}")]).unwrap();
    assert!(
        !none.contains("AFTER") && !none.contains("REPORT"),
        "{none}"
    );
    let code = app(
        "after",
        &[
            page,
            hooks(
                "fn after(cx: &mut Cx, reply: &mut Reply) {}
fn report(cx: &mut Cx, err: &Error) {}",
            ),
        ],
    )
    .unwrap();
    for want in [
        "const AFTER: bool = true;",
        "const REPORT: bool = true;",
        "pub fn after(cx: &mut ::wisp::Cx, reply: &mut ::wisp::Reply) { super::after(cx, reply) }",
        "fn report(cx: &mut ::wisp::Cx, err: &::wisp::Error) { hooks::__call::report(cx, err) }",
    ] {
        assert!(
            code.contains(want),
            "{want}
{code}"
        );
    }
    let err = |src| app("after-err", &[page, hooks(src)]).unwrap_err();
    assert!(
        err("fn after(cx: &mut Cx) {}")
            .contains("`after` is `fn after(cx: &mut Cx, reply: &mut Reply)`")
    );
    assert!(err("async fn report(cx: &mut Cx, err: &Error) {}").contains("`report` is"));
}

#[test]
fn hooks_are_checked() {
    let page = ("src/routes/+page.wisp", "x");
    let hooks = |src: &'static str| ("src/hooks.rs", src);
    let code = app("hooks-ok", &[page, hooks("async fn init() -> Result<()> { Ok(()) }\nfn before(cx: &mut Cx) -> Option<Response> { None }\nfn helper() {}")]).unwrap();
    for want in [
        "pub async fn init() -> ::wisp::Result<()> { let () = super::init().await?; Ok(()) }",
        "pub async fn before(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { Ok(::wisp::rt_traits::Answer::answer(super::before(cx))) }",
        "hooks::__call::init().await?;",
        "if let Some(r) = hooks::__call::before(cx).await? {",
        "const BEFORE: bool = true;",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let err = |name, src| app(name, &[page, hooks(src)]).unwrap_err();
    assert!(err("typo", "pub fn befor(cx: &mut Cx) {}").contains("`befor` is not a hook"));
    assert!(err("init-cx", "fn init(cx: &mut Cx) {}").contains("has no `cx`"));
    assert!(err("before-input", "fn before(cx: &mut Cx, id: u8) {}").contains("takes only `cx`"));
    let inner = "//! Hooks.\n#![allow(dead_code)]\nfn init() {}";
    let code = app("inner-hooks", &[page, hooks(inner)]).unwrap();
    assert!(
            code.contains("//! Hooks.\n#![allow(dead_code)]\n    #[allow(unused_imports)]\n    use super::__mods::*;"),
            "{code}"
        );
    assert!(
        err("returns", "pub fn before(cx: &mut Cx) -> u8 { 1 }").contains("`before` returns `u8`")
    );
    let main = ("src/main.rs", "mod hooks;\nwisp::main!();");
    assert!(
        app("main", &[page, hooks("pub fn init() {}"), main])
            .unwrap_err()
            .starts_with("src/main.rs:1: remove `mod hooks`")
    );
}

#[test]
fn server_files_serve_their_route_and_its_id() {
    let s = |src: &'static str| ("src/routes/notes/+server.rs", src);
    let files = [s("#[derive(Rest)]\nstruct N { t: String }\n\
                 fn before(cx: &mut Cx) {}\nfn delete(id: u64) -> Option<()> { None }\n\
                 fn before_create(cx: &mut Cx, n: &mut N) -> Result { Ok(()) }\n\
                 fn after_update(row: &Row<N>, id: u64) {}")];
    let m = model("rest", &files).unwrap();
    // Each route: its pattern, its file's module and `before`, and the
    // method and shim of each handler.
    let shape: Vec<String> = (m.routes.iter())
        .map(|r| {
            let s = r.server.as_ref().unwrap();
            let hs: Vec<String> = (s.handlers.iter())
                .map(|h| format!("{} {}", h.op.method, h.shim))
                .collect();
            format!("{} {} {}: {}", r.pattern, s.module, s.before, hs.join(", "))
        })
        .collect();
    assert_eq!(
        shape,
        [
            "/notes server_0 true: get __rest_list, post __rest_create",
            "/notes/[id=int] server_0 true: delete delete, get __rest_get, put __rest_put, patch __rest_patch"
        ]
    );
    let code = app("rest", &files).unwrap();
    for want in [
        "::wisp::rt::rest::list::<super::N>(cx, &__REST_HOOKS)",
        "use ::wisp::rt_traits::ret::*; let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"id\")?; (&&&Ret::new(super::delete(__a0))).respond()",
        "fn __hook_before_create(cx: &mut ::wisp::Cx, v: &mut super::N) -> ::wisp::Result { super::before_create(cx, v)?; Ok(()) }",
        "let () = super::after_update(row, row.id); Ok(())",
        "Hooks { before_create: Some(__hook_before_create), after_update: Some(__hook_after_update), ..",
    ] {
        assert!(code.contains(want), "{want}: {code}");
    }
    let err = |name, src| app(name, &[s(src)]).unwrap_err();
    assert!(
        err(
            "bad-hook",
            "#[derive(Rest)] struct N { t: String }\nfn before_create(n: N) {}"
        )
        .contains(":2: `before_create` takes `cx` and `note: &mut N`, not `N`")
    );
    assert!(err("list-id", "fn list(id: u64) {}").contains(":1: `list` answers GET on the route"));
    assert!(
        err("two-gets", "fn get() {}\nfn list() {}")
            .contains(":2: `list` and `get` both answer GET")
    );
    assert!(err("none", "fn helper() {}").contains("+server.rs: defines none of"));
    assert!(
        err("checked", "fn post(#[validate(len = 1..)] n: String) {}")
            .contains("checks an action's input")
    );
    let under_id = (
        "src/routes/n/[id]/+server.rs",
        "#[derive(Rest)] struct A { a: u8 }",
    );
    assert!(
        app("rest-id", &[under_id])
            .unwrap_err()
            .contains("already has an `id`")
    );
}

#[test]
fn an_error_where_only_endpoints_are_is_json() {
    let get = "pub fn get() -> u8 { 1 }";
    let page = ("src/routes/+page.wisp", "x");
    let api = |p| (p, get);
    // Endpoints and pages: the first segments with endpoints alone.
    let code = app(
        "api-prefix",
        &[
            page,
            api("src/routes/api/+server.rs"),
            api("src/routes/api/[id]/+server.rs"),
            api("src/routes/hook/+server.rs"),
            ("src/routes/hook/docs/+page.wisp", "y"),
        ],
    )
    .unwrap();
    assert!(
        code.contains("const API_PREFIXES: &'static [&'static str] = &[\"api\"];"),
        "{code}"
    );
    assert!(!code.contains("API_ONLY"), "{code}");
    // No page at all.
    let code = app("api-only", &[api("src/routes/a/+server.rs")]).unwrap();
    assert!(code.contains("const API_ONLY: bool = true;"), "{code}");
    assert!(!code.contains("API_PREFIXES"), "{code}");
    // Pages alone: no check.
    let code = app("no-api", &[page]).unwrap();
    assert!(!code.contains("API_"), "{code}");
}

#[test]
fn saved_tables_load_at_startup() {
    let code = app(
            "tables",
            &[
                ("src/routes/+page.wisp", "x"),
                (
                    "src/routes/+page.rs",
                    "static TODOS: Table<String> = Table::saved(\"todos\");\nconst C: Table<u8> = Table::new();",
                ),
                (
                    "src/db.rs",
                    "pub static USERS: wisp::Table<u8> = Table::saved(\"users\");",
                ),
                (
                    "src/routes/api/+server.rs",
                    "#[derive(Rest)] struct N { t: String }",
                ),
            ],
        )
        .unwrap();
    for want in [
        "pub fn __ready() { super::TODOS.ready(); }",
        "pub fn __ready() { super::USERS.ready(); }",
        "pub fn __ready() { super::N::table().ready(); }",
        "__mods::db::__call::__ready();",
        "page_0::__call::__ready();",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    assert_eq!(code.matches("::__call::__ready();").count(), 3, "{code}");
    assert!(!code.contains("C.ready()"), "{code}");
}

#[test]
fn db_items_are_in_every_route_file() {
    let db = ("src/db.rs", "pub fn items() -> Vec<u8> { vec![] }");
    let page = (
        "src/routes/+page.wisp",
        "---\nlet n = items().len();\n---\n{n}",
    );
    let bare = ("src/routes/a/+page.wisp", "{#each items() as i}{i}{/each}");
    let code = app("db-glob", &[db, page, bare]).unwrap();
    let globs = code.matches("use super::__mods::db::*;").count();
    // The page (its template inherits it) and the page with no block: not `db` itself.
    assert_eq!(globs, 2, "{code}");
    let none = app("db-none", &[page]).unwrap();
    assert!(!none.contains("db::*"), "{none}");
}

#[test]
fn an_upload_is_held_to_a_size() {
    let page = |src: &'static str| ("src/routes/+page.wisp", src);
    let none = "---\n#[action]\nfn a(img: Image, b: Option<Image>) {}\n---\n<form action=\"?/a\"><input name=\"img\" type=\"file\"></form>";
    let code = app("size-default", &[page(none)]).unwrap();
    assert!(
        code.contains("max_size(__v, (::wisp::MAX_SIZE) as usize)"),
        "{code}"
    );
    assert!(
        code.contains("UPLOADS: usize = 0 + ::wisp::MAX_SIZE + ::wisp::MAX_SIZE;"),
        "{code}"
    );
    assert!(
        code.contains(r#"<form action=\"?/a\" method=\"post\" enctype=\"multipart/form-data\">"#),
        "{code}"
    );
    // Alone, too: not "one input, nothing to check".
    let one = app(
        "size-one",
        &[page("---\n#[action]\nfn a(img: Image) {}\n---\nx")],
    )
    .unwrap();
    assert!(
        one.contains("max_size(__v, (::wisp::MAX_SIZE) as usize)"),
        "{one}"
    );
    let own = "---\n#[action]\nfn a(#[validate(max_size = 3 * MB)] img: Image) {}\n---\nx";
    let code = app("size-own", &[page(own)]).unwrap();
    assert!(
        code.contains("UPLOADS: usize = 0 + (3 * MB) as usize;"),
        "{code}"
    );
    assert!(!code.contains("MAX_SIZE"), "{code}");
}

#[test]
fn cx_user_needs_the_users_table() {
    let page = ("src/routes/+page.wisp", "---\nlet me = cx.user()?;\n---\nx");
    let err = app("user-none", &[page]).unwrap_err();
    assert!(err.contains("wisp::users(&db::USERS)"), "{err}");
    let hooks = ("src/hooks.rs", "fn init() { wisp::users(&db::USERS); }");
    let db = ("src/db.rs", "pub static USERS: Table<u8> = Table::saved();");
    let code = app("user-some", &[page, hooks, db]).unwrap();
    assert!(code.contains("cx.user(&db::USERS)?"), "{code}");
    assert!(code.contains("Table::saved(\"users\")"), "{code}");
    // Without `init`, the one table of a model with a password.
    let db = (
        "src/db.rs",
        "#[model]\npub struct User { email: Email, password: Password }\npub static USERS: Table<User> = Table::saved();",
    );
    let code = app("user-lone", &[page, db]).unwrap();
    assert!(code.contains("cx.user(&db::USERS)?"), "{code}");
}

#[test]
fn action_parameters_are_checked() {
    let page = |src: &'static str| ("src/routes/+page.wisp", src);
    let code = app(
            "rules",
            &[page("---\n#[action]\nfn add(#[validate(len = 1..10, email)] t: String, #[validate(min = 1)] n: Option<u8>) {}\n---\nx")],
        )
        .unwrap();
    // Every input is read and checked before the answer, which lists
    // each that did not pass.
    for want in [
        "let __a0 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"t\"))?; \
             if let Some(__v) = &__a0 { __p.check(\"t\", ::wisp::rt_traits::len(__v, 1..10)); \
             __p.check(\"t\", ::wisp::json::check::email(__v)); }",
        "if let Some(__v) = &__a1 { __p.check(\"n\", ::wisp::json::check::min(__v, 1i128)); }",
        "let (Some(__a0), Some(__a1), true) = (__a0, __a1, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); };",
    ] {
        assert!(code.contains(want), "{want}: {code}");
    }
    let err = |name, src| app(name, &[page(src)]).unwrap_err();
    assert!(
        err(
            "rule",
            "---\n#[action]\nfn a(#[validate(size = 1)] t: String) {}\n---\nx"
        )
        .contains("has no `size`")
    );
    assert!(
        err(
            "range",
            "---\n#[action]\nfn a(#[validate(len = 5)] t: String) {}\n---\nx"
        )
        .contains("needs a range")
    );
}

#[test]
fn guards_are_checked_and_run_first() {
    let rs = "const RATE_LIMIT: u32 = 5;\nconst CORS: &str = \"*\";\nconst TIMEOUT: u32 = 2;\nfn get() {}";
    let code = app("guard-ok", &[("src/routes/+server.rs", rs)]).unwrap();
    for want in [
        "pub async fn before(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { cx.cors(super::CORS)?; __RATE.check(cx.client_ip())?; Ok(None) }",
        "pub const TIMEOUT: u32 = super::TIMEOUT;",
        "::wisp::rt::within(server_0::__call::TIMEOUT, async {",
    ] {
        assert!(code.contains(want), "{want}: {code}");
    }
    let page = (
        "src/routes/+page.wisp",
        "---\nconst RATE_LIMIT: u32 = 5;\n---\nx",
    );
    let code = app("guard-page", &[page]).unwrap();
    assert!(code.contains("page_0::__call::__guard(cx)?;"), "{code}");
    // In hooks.rs, with a `before` of its own or without.
    let hooks = |src| {
        app(
            "guard-hooks",
            &[("src/routes/+page.wisp", "x"), ("src/hooks.rs", src)],
        )
    };
    for src in [
        "const CORS: &str = \"*\";",
        "const CORS: &str = \"*\";\nfn before(cx: &mut Cx) {}",
    ] {
        let code = hooks(src).unwrap();
        assert!(code.contains("{ cx.cors(super::CORS)?; "), "{code}");
        assert!(code.contains("hooks::__call::before(cx)"), "{code}");
    }
    let err = app(
        "guard-ty",
        &[(
            "src/routes/+server.rs",
            "const RATE_LIMIT: u8 = 5;\nfn get() {}",
        )],
    );
    assert!(err.unwrap_err().contains("make it a `u32`"));
    let layout = (
        "src/routes/+layout.wisp",
        "---\nconst RATE_LIMIT: u32 = 5;\n---\n{@render children()}",
    );
    let err = app("guard-layout", &[("src/routes/+page.wisp", "x"), layout]);
    assert!(
        err.unwrap_err()
            .contains("a layout's `RATE_LIMIT` does nothing")
    );
}

#[test]
fn slots_are_drawn_by_their_layout() {
    let layout = (
        "src/routes/d/+layout.wisp",
        "<aside>{@render stats()}</aside>{@render children()}",
    );
    let slot = ("src/routes/d/@stats/+page.wisp", "<p>s</p>");
    let page = ("src/routes/d/+page.wisp", "<h1>d</h1>");
    let code = app("slot-ok", &[layout, slot, page]).unwrap();
    // The layout takes it as a parameter, the page's route gives it.
    assert!(
        code.contains("children: &dyn Fn(&mut ::wisp::Out), stats: &dyn Fn(&mut ::wisp::Out))"),
        "{code}"
    );
    assert!(
        code.contains(", &|__o: &mut ::wisp::Out| tpl_page_"),
        "{code}"
    );
    // Without a slot, a layout's render has no extra parameter.
    let plain = app(
        "slot-none",
        &[("src/routes/d/+layout.wisp", "{@render children()}"), page],
    )
    .unwrap();
    assert!(!code_has_slot(&plain), "{plain}");
    for (name, files, want) in [
        (
            "slot-nolayout",
            vec![slot, page],
            "a slot needs a +layout.wisp",
        ),
        (
            "slot-undrawn",
            vec![
                ("src/routes/d/+layout.wisp", "{@render children()}"),
                slot,
                page,
            ],
            "draw it with {@render stats()}",
        ),
        (
            "slot-waits",
            vec![
                layout,
                (
                    "src/routes/d/@stats/+page.wisp",
                    "---\nlet n = 1;\n---\n<p>{n}</p>",
                ),
                page,
            ],
            "cannot wait",
        ),
        (
            "slot-name",
            vec![layout, ("src/routes/d/@children/+page.wisp", "x"), page],
            "is not a slot name",
        ),
    ] {
        let err = app(name, &files).unwrap_err();
        assert!(err.contains(want), "{name}: {err}");
    }
}

fn code_has_slot(code: &str) -> bool {
    code.contains(": &dyn Fn(&mut ::wisp::Out))") && code.contains("stats:")
}

#[test]
fn named_middleware_is_resolved_into_the_route() {
    let mw = (
        "src/middleware.rs",
        "pub fn auth(cx: &mut Cx) -> Result { Ok(()) }\npub fn audit(cx: &mut Cx) -> Result { Ok(()) }\nfn hidden() {}",
    );
    let page = (
        "src/routes/admin/+page.wisp",
        "---\nconst MIDDLEWARE: &[&str] = &[\"auth\", \"audit\"];\n---\nx",
    );
    let code = app("mw-page", &[mw, page, ("src/routes/+page.wisp", "y")]).unwrap();
    assert!(
        code.contains("middleware::auth(cx)?; middleware::audit(cx)?; Ok(()) }"),
        "{code}"
    );
    // A folder's: in its layout, for every page below.
    let layout = (
        "src/routes/team/+layout.wisp",
        "---\nconst MIDDLEWARE: &[&str] = &[\"auth\"];\n---\n{@render children()}",
    );
    let code = app(
        "mw-layout",
        &[mw, layout, ("src/routes/team/+page.wisp", "x")],
    )
    .unwrap();
    assert!(code.contains("middleware::auth(cx)?; Ok(()) }"), "{code}");
    // An endpoint's.
    let rs = "const MIDDLEWARE: &[&str] = &[\"auth\"];\nfn get() {}";
    let code = app("mw-server", &[mw, ("src/routes/+server.rs", rs)]).unwrap();
    assert!(code.contains("middleware::auth(cx)?;"), "{code}");
    // Nothing named, nothing run.
    let code = app("mw-none", &[mw, ("src/routes/+page.wisp", "x")]).unwrap();
    assert!(!code.contains("middleware::"), "{code}");
    for (name, files, want) in [
        ("mw-nofn", vec![mw, page], "no `pub fn nope"),
        ("mw-private", vec![mw, page], "no `pub fn hidden"),
        (
            "mw-nofile",
            vec![page],
            "src/middleware.rs, which is not there",
        ),
    ] {
        let bad = match name {
            "mw-nofn" => "---\nconst MIDDLEWARE: &[&str] = &[\"nope\"];\n---\nx",
            "mw-private" => "---\nconst MIDDLEWARE: &[&str] = &[\"hidden\"];\n---\nx",
            _ => "---\nconst MIDDLEWARE: &[&str] = &[\"auth\"];\n---\nx",
        };
        let files: Vec<_> = files
            .into_iter()
            .filter(|f| f.0 != page.0)
            .chain([(page.0, bad)])
            .collect();
        let err = app(name, &files).unwrap_err();
        assert!(err.contains(want), "{name}: {err}");
    }
    let numbers = "---\nconst MIDDLEWARE: &[&str] = &[\"1x\"];\n---\nx";
    let err = app("mw-ident", &[mw, (page.0, numbers)]).unwrap_err();
    assert!(err.contains("is not a function name"), "{err}");
}

#[test]
fn runtime_is_a_checked_literal() {
    let page = ("src/routes/+page.wisp", "x");
    let ok = |v: &'static str| vec![page, ("src/routes/+page.rs", v)];
    let code = app(
        "runtime-ok",
        &ok("const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;"),
    )
    .unwrap();
    assert!(
        code.contains("const _: ::wisp::Runtime = super::RUNTIME;"),
        "{code}"
    );
    let e = app("runtime-bad", &ok("const RUNTIME: wisp::Runtime = pick();")).unwrap_err();
    assert!(
        e.contains("+page.rs:1") && e.contains("as a literal"),
        "{e}"
    );
    let e = app("runtime-ty", &ok("const RUNTIME: bool = true;")).unwrap_err();
    assert!(e.contains("as a literal"), "{e}");
    let layout = [
        ("src/routes/+layout.wisp", "{@render children()}"),
        (
            "src/routes/+layout.rs",
            "pub const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;",
        ),
        page,
    ];
    assert!(
        app("runtime-layout", &layout)
            .unwrap_err()
            .contains("a layout's `RUNTIME` does nothing")
    );
}

#[test]
fn body_limits_are_checked() {
    let page = ("src/routes/+page.wisp", "x");
    let rs = |src: &'static str| ("src/routes/+page.rs", src);
    let files = [page, rs("const BODY_LIMIT: usize = 8 * wisp::MB;")];
    let m = model("limit-ok", &files).unwrap();
    assert_eq!(m.routes[0].body_limit.as_deref(), Some("page_0"));
    let code = app("limit-ok", &files).unwrap();
    assert_eq!(
        facts(&code, "body_limit"),
        ["Some(page_0::__call::BODY_LIMIT)"]
    );
    assert_eq!(facts(&code, "uploads"), ["None"]);
    assert!(
        code.contains("pub const BODY_LIMIT: usize = super::BODY_LIMIT;"),
        "{code}"
    );
    // The page's limit is the page's: the `/[id]` its `+server.rs` also
    // serves has none (its module has no `BODY_LIMIT` to name); the
    // file's own limit is both routes'.
    let m = model(
        "limit-page-only",
        &[
            page,
            rs("const BODY_LIMIT: usize = 1;"),
            ("src/routes/+server.rs", "fn put(id: u64) {}"),
        ],
    )
    .unwrap();
    let limits: Vec<Option<&str>> = m.routes.iter().map(|r| r.body_limit.as_deref()).collect();
    assert_eq!(limits, [Some("page_0"), None]);
    let m = model(
        "limit-server",
        &[(
            "src/routes/a/+server.rs",
            "const BODY_LIMIT: usize = 1;\nfn post() {}\nfn put(id: u64) {}",
        )],
    )
    .unwrap();
    let limits: Vec<Option<&str>> = m.routes.iter().map(|r| r.body_limit.as_deref()).collect();
    assert_eq!(limits, [Some("server_0"), Some("server_0")]);
    assert!(
        app("limit-type", &[page, rs("pub const BODY_LIMIT: u64 = 1;")])
            .unwrap_err()
            .contains("make it a `usize`")
    );
    let layout = [
        ("src/routes/+layout.wisp", "{@render children()}"),
        ("src/routes/+layout.rs", "pub const BODY_LIMIT: usize = 1;"),
        page,
    ];
    assert!(
        app("limit-layout", &layout)
            .unwrap_err()
            .contains("a layout's `BODY_LIMIT` does nothing")
    );
}

#[test]
fn uploads_raise_the_body_limit() {
    let page = |src: &'static str| [("src/routes/+page.wisp", src)];
    let code = app(
            "uploads",
            &page(
                "---\nconst BODY_LIMIT: usize = 64 * KB;\n#[action]\nfn a(#[validate(max_size = 1 * MB)] pic: Image, #[validate(max_size = 500)] mut more: Option<wisp::Image>) {}\n#[action]\nfn b(pic: Image) {}\n---\nx",
            ),
        )
        .unwrap();
    assert_eq!(
        facts(&code, "body_limit"),
        ["Some(page_0::__call::BODY_LIMIT)"]
    );
    assert_eq!(facts(&code, "uploads"), ["Some(page_0::__call::UPLOADS)"]);
    for want in [
        "pub const UPLOADS: usize = 0 + (1 * MB) as usize + (500) as usize + ::wisp::MAX_SIZE;",
        "if let Some(__v) = &__a0 { __p.check(\"pic\", ::wisp::rt_traits::max_size(__v, (1 * MB) as usize)); }",
        "use super::*;",
    ] {
        assert!(code.contains(want), "{want}: {code}");
    }
    let alone = page("---\n#[action]\nfn a(#[validate(max_size = 9)] pic: Image) {}\n---\nx");
    let m = model("uploads-alone", &alone).unwrap();
    let r = &m.routes[0];
    assert_eq!(
        (r.uploads.as_deref(), r.body_limit.as_deref()),
        (Some("page_0"), None)
    );
    let none = model(
        "uploads-none",
        &page("---\n#[action]\nfn a(pic: String) {}\n---\nx"),
    );
    assert!(none.unwrap().routes[0].uploads.is_none());
    let wrong = app(
        "uploads-text",
        &page("---\n#[action]\nfn a(#[validate(max_size = 9)] pic: String) {}\n---\nx"),
    )
    .unwrap_err();
    assert!(
        wrong.contains("+page.wisp:3: `max_size` is for an upload, and `pic` is a `String`"),
        "{wrong}"
    );
}

#[test]
fn noindex_pages_and_private_groups_leave_the_sitemap() {
    assert!(noindex("<META content='NOINDEX, follow' name=robots>"));
    assert!(!noindex(
        "<meta name=\"description\" content=\"noindex\"> robots"
    ));
    let files = [
        ("src/routes/+page.wisp", "x"),
        ("src/routes/(private)/a/+page.wisp", "x"),
        ("src/routes/b.md", "---\nnoindex: true\n---\nx"),
    ];
    let code = app("sitemap", &files).unwrap();
    for (pattern, indexed) in [("/", true), ("/a", false), ("/b", false)] {
        let want = format!(
            "pattern: {pattern:?}, page: true, actions: false, server: false, entries: None, indexed: {indexed}, ssr: true, prerender: false }}"
        );
        assert!(code.contains(&want), "{want}\n{code}");
    }
}

#[test]
fn markdown_pages_bake_and_list() {
    let post = "{@props title: &str}<article>{title}{@render children()}</article>";
    let files = [
        ("src/components/Post.wisp", post),
        (
            "src/routes/blog/a.md",
            "---\ndate: 2026-01-01\n---\n# First",
        ),
        (
            "src/routes/blog/b.md",
            "---\nlayout: Post\ntitle: Second\ndate: 2026-02-01\n---\nText {x}",
        ),
        ("src/routes/+page.md", "Home"),
    ];
    let code = app("markdown", &files).unwrap();
    for want in [
        "static BAKED_0",
        "static BAKED_1",
        "static BAKED_2",
        r"<article>Second\n<p>Text &#123;x&#125;</p>\n</article>",
        r#"::wisp::MdPage { path: "/", title: "", fields: &[] },"#,
        r#"::wisp::MdPage { path: "/blog/b", title: "Second", fields: &[("layout", "Post"), ("title", "Second"), ("date", "2026-02-01")] },"#,
        r#"::wisp::MdPage { path: "/blog/a", title: "First", fields: &[("date", "2026-01-01"), ("title", "First")] },"#,
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    assert!(code.find("/blog/b\", title") < code.find("/blog/a\", title"));
    let bad = [("src/routes/a.md", "---\nlayout: Nope\n---\n")];
    let err = app("markdown-bad", &bad).unwrap_err();
    assert!(
        err.starts_with("src/routes/a.md:1: `layout: Nope`"),
        "{err}"
    );
}

#[test]
fn constant_pages_are_baked() {
    let files = [
        (
            "src/app.html",
            "<html><head>%wisp.head%</head><body>%wisp.body%</body></html>",
        ),
        (
            "src/routes/+layout.wisp",
            "<nav>{\"a&b\"}</nav>{@render children()}",
        ),
        // A GET never has what an action refused: the form is baked too.
        (
            "src/routes/+page.wisp",
            "<title>T</title><h1>{1}</h1><form action=\"?/add\"><input name=\"x\"></form>",
        ),
        ("src/routes/+page.rs", "#[action]\nfn add() {}"),
        ("src/routes/user/[name]/+page.wisp", "<h1>{name}</h1>"),
        (
            "src/routes/loads/+page.wisp",
            "---\nlet n = 1;\n---\n<h1>n</h1>",
        ),
    ];
    let code = app("baked", &files).unwrap();
    let doc = "<html><head><script defer src=\\\"/_app/wisp.js?v=VERSION\\\"></script><title>T</title></head><body><nav>a&amp;b</nav><h1>1</h1><form action=\\\"?/add\\\" method=\\\"post\\\"><input name=\\\"x\\\"></form></body></html>"
            .replace("VERSION", crate::runtime_version());
    let etag = format!(
        "\\\"{:016x}\\\"",
        fnv1a(doc.replace("\\\"", "\"").as_bytes())
    );
    for want in [
            format!(
                "static BAKED_0: ::wisp::rt::Baked = ::wisp::rt::Baked::new(\"HTTP/1.1 200 OK\\r\\ncontent-type: text/html; charset=utf-8\\r\\netag: {etag}\\r\\ncontent-length: {}\\r\\n\", \"{doc}\", \"{etag}\"); // /",
                doc.replace("\\\"", "\"").len()
            ),
            "(0, Get | Head) => { ::wisp::rt::browser_ok(cx)?; if ::wisp::rt::baked(cx, __o, &BAKED_0) { Ok(()) } else { serve_page_0(cx, __o).await } },".into(),
            // The action's post renders as before.
            "(0, Post) => {".into(),
        ] {
            assert!(code.contains(&want), "{want}\n{code}");
        }
    assert_eq!(code.matches("static BAKED_").count(), 1, "{code}");
}

#[test]
fn pages_without_server_rendering() {
    // The markup is one client block the server does not paint; the
    // head is the server's.
    let code = app(
        "drawn",
        &[(
            "src/routes/+page.wisp",
            "---\nconst SSR: bool = false;\nlet n = 1;\n---\n<title>{n}</title><p>{:n}</p>",
        )],
    )
    .unwrap();
    assert!(code.contains("const _: bool = super::SSR;"), "{code}");
    assert!(
        !code.contains("COPY") && !code.contains("<!--[-->"),
        "{code}"
    );
    for (name, page, want) in [
        (
            "drawn-rust",
            "---\nconst SSR: bool = false;\nlet n = 1;\n---\n<p>\n{n}</p>",
            "+page.wisp:6:1: this page has `const SSR: bool = false;`",
        ),
        (
            "drawn-flag",
            "---\nconst SSR: bool = 1 > 2;\n---\nx",
            "the build reads `SSR`",
        ),
        (
            "drawn-type",
            "---\nstatic SSR: bool = false;\n---\nx",
            "the build reads `SSR`",
        ),
    ] {
        let err = app(name, &[("src/routes/+page.wisp", page)]).unwrap_err();
        assert!(err.contains(want), "{name}: {err}");
    }
    // A layout's `SSR` is its pages', unless a page says otherwise.
    let layout = (
        "src/routes/+layout.wisp",
        "---\nconst SSR: bool = false;\nconst PRERENDER: bool = true;\n---\n<slot />",
    );
    let m = model(
        "drawn-layout",
        &[
            layout,
            ("src/routes/+page.wisp", "x"),
            (
                "src/routes/b/+page.wisp",
                "---\nconst SSR: bool = true;\nconst PRERENDER: bool = false;\n---\nx",
            ),
        ],
    )
    .unwrap();
    let flags: Vec<_> = (m.routes.iter())
        .map(|r| r.page.as_ref().map(|p| (p.drawn, p.prerender)))
        .collect();
    assert_eq!(flags, [Some((true, true)), Some((false, false))]);
}

#[test]
fn prerendered_pages() {
    // Until `wisp build` renders it, each worker keeps its first render.
    let page = "---\nconst PRERENDER: bool = true;\nlet n = 1;\n---\n{n}";
    let code = app("pre", &[("src/routes/+page.wisp", page)]).unwrap();
    for want in [
        "const _: bool = super::PRERENDER;",
        "pub const CACHE: u32 = u32::MAX;",
        "if ::wisp::rt::cached::<false>(cx, __o, true) { return Ok(()); }",
        "::wisp::ExportRoute { pattern: \"/\", page: true, actions: false, server: false, entries: None, indexed: true, ssr: true, prerender: true },",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    // What `wisp build` rendered is served by path, before that.
    let files = [
        (
            "src/routes/[slug]/+page.wisp",
            "---\nconst PRERENDER: bool = true;\nfn entries() -> Vec<&'static str> { vec![\"a\"] }\n---\n<p>{slug}</p>",
        ),
        (
            "pre/index.tsv",
            "/[slug]\t/a\t0.html\n/gone\t/gone\t1.html\n",
        ),
        ("pre/0.html", "<p>a</p>"),
    ];
    let code = in_dir("pre-built", &files, |root| {
        let pre = root.join("pre");
        generate(&Input {
            root,
            release: true,
            maps: false,
            prerendered: Some(&pre),
        })
        .map(|o| o.code)
    })
    .unwrap();
    for want in [
        "static PRE_0_0: ::wisp::rt::Baked = ::wisp::rt::Baked::new(\"HTTP/1.1 200 OK\\r\\ncontent-type: text/html; charset=utf-8\\r\\netag: \\\"",
        "if let Some(b) = match cx.path() { \"/a\" => Some(&PRE_0_0), _ => None } { if ::wisp::rt::baked(cx, __o, b) { return Ok(()); } }",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    assert!(
        !code.contains("PRE_0_1") && !code.contains("/gone"),
        "{code}"
    );
    for (name, files, want) in [
        (
            "pre-cx",
            vec![(
                "src/routes/+page.wisp",
                "---\nconst PRERENDER: bool = true;\n---\n{cx.cookie(\"a\")}",
            )],
            "+page.wisp:2: this page is prerendered",
        ),
        (
            "pre-load",
            vec![
                ("src/routes/+page.wisp", "{n}"),
                (
                    "src/routes/+page.rs",
                    "const PRERENDER: bool = true;\nstruct Data { n: u8 }\nfn load(cx: &Cx) -> Data { Data { n: 1 } }",
                ),
            ],
            "+page.rs:1: this page is prerendered",
        ),
        (
            "pre-params",
            vec![(
                "src/routes/[id]/+page.wisp",
                "---\nconst PRERENDER: bool = true;\n---\n{id}",
            )],
            "say which pages to render",
        ),
        (
            "pre-cache",
            vec![(
                "src/routes/+page.wisp",
                "---\nconst PRERENDER: bool = true;\nconst CACHE: u32 = 5;\n---\nx",
            )],
            "`PRERENDER` and `CACHE` do not go together",
        ),
    ] {
        let err = app(name, &files).unwrap_err();
        assert!(err.contains(want), "{name}: {err}");
    }
}

#[test]
fn links_against_the_trailing_slash_are_warned() {
    let warnings = |name: &str, hooks: Option<&'static str>| {
        let mut files = vec![
            (
                "src/routes/+page.wisp",
                "<a href=\"/about/\">a</a>\n<a href=\"/about?x=1\">b</a>\n<a href=\"/api/x\">c</a>\n\
                     <a href=\"/feed.xml\">d</a><a href=\"/_app/x\">e</a><a href=\"/\">f</a>",
            ),
            ("src/routes/about/+page.wisp", "x"),
            ("src/routes/api/x/+server.rs", "fn get() {}"),
        ];
        files.extend(hooks.map(|h| ("src/hooks.rs", h)));
        in_dir(name, &files, |root| {
            check(&Input {
                root,
                release: false,
                maps: true,
                prerendered: None,
            })
            .map(|o| o.1)
        })
        .unwrap()
    };
    assert_eq!(
        warnings("slash-never", None),
        [
            "src/routes/+page.wisp:1: href=\"/about/\" gets a 308 to /about (wisp::trailing_slash(Never)): link there"
        ]
    );
    assert_eq!(
        warnings(
            "slash-always",
            Some("fn init() {\n    wisp::trailing_slash(Always);\n}")
        ),
        [
            "src/routes/+page.wisp:2: href=\"/about?x=1\" gets a 308 to /about/ (wisp::trailing_slash(Always)): link there"
        ]
    );
    let ignore = "fn init() { wisp::trailing_slash(wisp::TrailingSlash::Ignore); }";
    assert!(warnings("slash-ignore", Some(ignore)).is_empty());
}

#[test]
fn cache_stale_and_tags_are_consts_the_shim_passes() {
    let plain = (
        "src/routes/+page.wisp",
        "---
const CACHE: u32 = 60;
---
x",
    );
    let code = app("cache-more-none", &[plain]).unwrap();
    assert!(
        code.contains("CacheMore::new(0, &[])"),
        "no window, no tags: {code}"
    );
    let more = (
        "src/routes/+page.wisp",
        "---
const CACHE: u32 = 60;
const CACHE_STALE: u32 = 600;
const CACHE_TAGS: &[&str] = &[\"posts\"];
---
x",
    );
    let code = app("cache-more", &[more]).unwrap();
    assert!(
        code.contains("CacheMore::new(super::CACHE_STALE, super::CACHE_TAGS)"),
        "{code}"
    );
}

#[test]
fn cache_keeps_gets() {
    let page = (
        "src/routes/+page.wisp",
        "---\nconst CACHE: u32 = 60;\nlet n = 1;\n---\n{n}",
    );
    let cache = |m: &Model| -> Vec<Option<(String, bool, bool)>> {
        (m.routes.iter())
            .map(|r| (r.cache.as_ref()).map(|c| (c.module.clone(), c.public, r.by_accept())))
            .collect()
    };
    let m = model("cache-ok", &[page]).unwrap();
    assert_eq!(cache(&m), [Some(("page_0".into(), false, false))]);
    let code = app("cache-ok", &[page]).unwrap();
    for want in [
        "pub const CACHE: u32 = super::CACHE;",
        "(0, Get | Head) => { ::wisp::rt::browser_ok(cx)?; { if ::wisp::rt::cached::<false>(cx, __o, false) { return Ok(()); } \
             serve_page_0(cx, __o).await?; ::wisp::rt::keep::<Self, false>(cx, __o, page_0::__call::CACHE, false, page_0::__call::MORE); Ok(()) } },",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let server = (
        "src/routes/api/+server.rs",
        "const CACHE_PUBLIC: u32 = 5;\nfn get() -> u8 { 1 }\nfn post() {}",
    );
    let m = model("cache-server", &[server]).unwrap();
    assert_eq!(cache(&m), [Some(("server_0".into(), true, false))]);
    let code = app("cache-server", &[server]).unwrap();
    for want in [
        "pub const CACHE: u32 = super::CACHE_PUBLIC;",
        "(0, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<false>(cx, __o, true) { return Ok(()); } \
             ::wisp::rt::respond(__o, server_0::__call::get(cx).await?); ::wisp::rt::keep::<Self, false>(cx, __o, server_0::__call::CACHE, true, server_0::__call::MORE); Ok(()) }",
        "(0, Post) => { ::wisp::rt::endpoint(cx); ::wisp::rt::check_origin(cx)?; \
             server_0::__call::post(cx).await?; ::wisp::rt::no_content(__o); Ok(()) }",
        // The same, sync, with no future: what `handle_now` answers.
        "now: true, sync: ::wisp::Method::Get.bit() | ::wisp::Method::Head.bit() | ::wisp::Method::Post.bit(),",
        "(0, Get | Head) => { ::wisp::rt::hooked(cx); ::wisp::rt::endpoint(cx); \
             if ::wisp::rt::cached::<false>(cx, __o, true) { return Ok(true); } \
             ::wisp::rt::respond(__o, server_0::__call::get_now(cx)?); \
             ::wisp::rt::keep::<Self, false>(cx, __o, server_0::__call::CACHE, true, server_0::__call::MORE); Ok(true) }",
        "(0, Post) => { ::wisp::rt::hooked(cx); ::wisp::rt::endpoint(cx); \
             server_0::__call::post_now(cx)?; ::wisp::rt::no_content(__o); Ok(true) }",
        "pub fn post_now(cx: &mut ::wisp::Cx) -> ::wisp::Result<()> { super::post(); Ok(()) }",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let rs = |src: &'static str| [("src/routes/+page.wisp", "x"), ("src/routes/+page.rs", src)];
    for (name, files, want) in [
        (
            "cache-type",
            rs("const CACHE: u64 = 1;"),
            "make it a `const` `u32`",
        ),
        (
            "cache-static",
            rs("static CACHE: u32 = 1;"),
            "make it a `const` `u32`",
        ),
        (
            "cache-both",
            rs("const CACHE: u32 = 1;\nconst CACHE_PUBLIC: u32 = 1;"),
            "set one of them",
        ),
    ] {
        let err = app(name, &files).unwrap_err();
        assert!(
            err.contains(want) && err.starts_with("src/routes/+page.rs:"),
            "{err}"
        );
    }
    let layout = [
        (
            "src/routes/+layout.wisp",
            "---\nconst CACHE_PUBLIC: u32 = 1;\n---\n{@render children()}",
        ),
        ("src/routes/+page.wisp", "x"),
    ];
    // The pages below it are kept too, but for one that sets its own.
    let m = model(
        "cache-layout",
        &[
            layout[0],
            layout[1],
            (
                "src/routes/b/+page.wisp",
                "---\nconst CACHE: u32 = 9;\n---\nx",
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        cache(&m),
        [
            Some(("layout_0".into(), true, false)),
            Some(("page_1".into(), false, false))
        ]
    );
    let twice = [
        (
            "src/routes/+page.wisp",
            "---\nconst CACHE: u32 = 1;\n---\nx",
        ),
        (
            "src/routes/+server.rs",
            "const CACHE: u32 = 1;\nfn post() {}",
        ),
    ];
    let err = app("cache-twice", &twice).unwrap_err();
    assert!(err.contains("set it in one place"), "{err}");

    // A list answers JSON or NDJSON by `accept`: what is kept of its
    // GET is kept apart by it, and only there.
    let rest = [(
        "src/routes/notes/+server.rs",
        "const CACHE: u32 = 5;\n#[derive(Rest)]\nstruct N { t: String }",
    )];
    let m = model("cache-accept", &rest).unwrap();
    assert_eq!(
        cache(&m),
        [
            Some(("server_0".into(), false, true)),
            Some(("server_0".into(), false, false))
        ]
    );
    let code = app("cache-accept", &rest).unwrap();
    for want in [
        "(0, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<true>(cx, __o, false) { return Ok(()); } \
             ::wisp::rt::respond(__o, server_0::__call::__rest_list(cx).await?); \
             ::wisp::rt::keep::<Self, true>(cx, __o, server_0::__call::CACHE, false, server_0::__call::MORE); Ok(()) }",
        "(1, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<false>(cx, __o, false) {",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    // A list of the file's own answers as it is written: one key.
    let own = [(
        "src/routes/notes/+server.rs",
        "const CACHE: u32 = 5;\n#[derive(Rest)]\nstruct N { t: String }\nfn list() -> Vec<u8> { vec![] }",
    )];
    let m = model("cache-own-list", &own).unwrap();
    assert!(m.routes.iter().all(|r| r.cache.is_some() && !r.by_accept()));
}

#[test]
fn routes_without_parameters_match_whole() {
    let files = [
        ("src/routes/+page.wisp", "x"),
        ("src/routes/about/+page.wisp", "x"),
        ("src/routes/blog/[slug]/+page.wisp", "{slug}"),
        ("src/routes/blog/new/+page.wisp", "x"),
    ];
    let code = app("router", &files).unwrap();
    for want in [
        "let whole = match path.len() {",
        "1 => (path == \"/\").then_some(0), // /",
        "6 => (path == \"/about\").then_some(1), // /about",
        "9 => (path == \"/blog/new\").then_some(2), // /blog/new",
        "let Some(r) = r.strip_prefix(\"blog/\") else { break 'a0; };",
        "let (p1, None) = ::wisp::rt::seg(r) else { break 'a0; };",
        "return Some((3, [p1, E, E, E, E, E, E, E]));",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    assert!(!code.contains("::wisp::rt::split"), "{code}");
    let code = app("router-flat", &files[..2]).unwrap();
    assert!(
        code.contains(
            "6 => (path == \"/about\").then_some((1, [\"\"; ::wisp::rt::MAX_PARAMS])), // /about"
        ),
        "{code}"
    );
    // Paths of one length, told apart by a byte, then compared whole.
    let five = [
        ("src/routes/echo/+server.rs", "fn get() {}"),
        ("src/routes/json/+server.rs", "fn get() {}"),
        ("src/routes/user/[id]/+server.rs", "fn get(id: String) {}"),
    ];
    let code = app("router-bytes", &five).unwrap();
    for want in [
        "5 => match path.as_bytes()[1] {",
        "101 => (path == \"/echo\").then_some(0), // /echo",
        "let Some(r) = r.strip_prefix(\"user/\") else { break 'a0; };",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let rest = [("src/routes/docs/[...path]/+page.wisp", "{path}")];
    let code = app("router-rest", &rest).unwrap();
    assert!(
        code.contains("let mut segs = [\"\"; ::wisp::rt::MAX_SEGS];"),
        "{code}"
    );
}

/// The devtools' hooks are in dev builds only.
#[test]
fn devtools_are_dev_only() {
    let card = (
        "src/components/Card.wisp",
        "{@props title: &str, n: u8 = 1}
<h2>{title}</h2><button on:click=\"k++\">{:k}</button>",
    );
    let page = ("src/routes/+page.wisp", "<Card title=\"a\" />");
    let dev = build("devtools-dev", &[card, page], false).unwrap();
    assert!(dev.contains("__wisp_dev?.state("));
    let release = build("devtools-release", &[card, page], true).unwrap();
    assert!(!release.contains("__wisp_dev"));
}

#[test]
fn release_builds_write_runs_of_text_at_once() {
    let page = (
        "src/routes/+page.wisp",
        "<p title={\"a\"}>{\"<b>\"} {2}</p>{#if on}{x}!{/if}<i>{3}</i>",
    );
    let code = build("runs", &[page], true).unwrap();
    for want in [
        "__o.body.push_str(\"<p title=\\\"a\\\">&lt;b&gt; 2</p>\");",
        "(&::wisp::rt::Text(&(x))).put(&mut __o.body);",
        "__o.body.push_str(\"!\");",
        "__o.body.push_str(\"<i>3</i>\");",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    // A dev build keeps each text apart, for `wisp dev` to swap.
    let code = build("runs-dev", &[page], false).unwrap();
    assert!(code.contains("__o.body.push_str(__wisp_s(0));"), "{code}");
    assert!(
        code.contains("(&::wisp::rt::Text(&(\"<b>\"))).put(&mut __o.body);"),
        "{code}"
    );
}

#[test]
fn actions_may_answer_with_a_response() {
    let page = ("src/routes/+page.wisp", "x");
    let rs = (
        "src/routes/+page.rs",
        "#[action] fn csv() -> Response { todo!() }\n#[action] fn maybe() -> Result<Option<Response>> { todo!() }",
    );
    let code = app("respond", &[page, rs]).unwrap();
    for want in [
        "{ Ok(::wisp::rt_traits::Answer::answer(super::csv())) }",
        "{ Ok(::wisp::rt_traits::Answer::answer(super::maybe()?)) }",
        "\"csv\" => match page_0::__call::csv(cx).await { Ok(Some(r)) => { ::wisp::rt::respond(__o, r); return Ok(()); } Ok(None) => {} Err(e) => ::wisp::rt::input::failed(cx, e)?, },",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let bad = ("src/routes/+page.rs", "#[action] pub fn n() -> u8 { 1 }");
    assert!(
        app("respond-bad", &[page, bad])
            .unwrap_err()
            .contains("or a `Response` to send instead of the page")
    );
}

#[test]
fn a_form_needs_the_action_it_posts_to() {
    let rs = ("src/routes/+page.rs", "#[action] fn add(text: String) {}");
    let ok = (
        "src/routes/+page.wisp",
        "<form action=\"?/add\"><button formaction=\"?/add&x=1\">go</button></form>",
    );
    assert!(app("posts-ok", &[ok, rs]).is_ok());
    let bad = (
        "src/routes/+page.wisp",
        "<p>x</p>
<form action=\"?/remove\"></form>",
    );
    let e = app("posts-bad", &[bad, rs]).unwrap_err();
    assert!(
        e.contains("+page.wisp:2: a form posts to `?/remove`"),
        "{e}"
    );
}

#[test]
fn inputs_are_read_by_name() {
    let files = [
        ("src/routes/[id]/+page.wisp", "x"),
        (
            "src/routes/[id]/+page.rs",
            "struct Data;\nfn load(id: u64, q: Option<&str>) -> Data { Data }\n\
                 #[action] async fn add(cx: &mut Cx, text: &str, tags: Vec<String>, on: bool) {}",
        ),
        (
            "src/routes/api/+server.rs",
            "fn get(n: Option<u8>) -> Vec<u8> { vec![] }\nfn post(name: String) {}\nfn delete() -> Option<Response> { None }\n\
                 fn put(body: Note) {}\nfn patch(body: Option<String>) {}",
        ),
    ];
    let code = app("inputs", &files).unwrap();
    for want in [
        "let __a0 = ::wisp::rt::input::body(cx)?; super::put(__a0); Ok(())",
        "let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"body\")?;",
        "(0, Options) => { ::wisp::rt::respond(__o, ::wisp::rt::options(\"GET, HEAD, POST, DELETE, PUT, PATCH, OPTIONS\")); Ok(()) }",
        "let __a0 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"id\"))?; \
             let __a1: Option<Option<String>> = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"q\"))?; \
             let (Some(__a0), Some(__a1), true) = (__a0, __a1, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); }; \
             Ok(Loaded(super::load(__a0, __a1.as_deref())))",
        "let __a1: Option<String> = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"text\"))?; \
             let __a2 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"tags\"))?; \
             let __a3 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"on\"))?; \
             let (Some(__a1), Some(__a2), Some(__a3), true) = (__a1, __a2, __a3, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); }; \
             Ok(::wisp::rt_traits::Answer::answer(super::add(cx, &__a1, __a2, __a3).await?))",
        "let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"name\")?; super::post(__a0); Ok(())",
        "(&&&Ret::new(super::get(__a0))).respond()",
        "(&&&Ret::new(super::delete())).respond()",
    ] {
        assert!(code.contains(want), "{want}\n{code}");
    }
    let bad = [
        ("src/routes/+page.wisp", "x"),
        (
            "src/routes/+page.rs",
            "\n#[action] fn a((x, y): (u8, u8)) {}",
        ),
    ];
    let err = app("inputs-bad", &bad).unwrap_err();
    assert!(
        err.starts_with("src/routes/+page.rs:2: `a` takes `(x, y): (u8, u8)`"),
        "{err}"
    );
    let page = |rs: &'static str| [("src/routes/+page.wisp", "x"), ("src/routes/+page.rs", rs)];
    // `//!` docs and `#![…]` first (CRLF read as LF), then what Wisp adds.
    let top = "//! A page.\r\n#![allow(dead_code)]\r\nuse std::fmt;";
    let code = app("inner", &page(top)).unwrap();
    assert!(
        code.contains("//! A page.\n#![allow(dead_code)]\n    #[allow(unused_imports)]"),
        "{code}"
    );
    assert!(code.contains("use std::fmt;"), "{code}");
    // The prelude comes after the file's own import of it (the one
    // rustc counts as used), and CRLF and a BOM are read as LF.
    let long = "\u{feff}use wisp::prelude::*;\r\nfn helper() {}";
    let files = [
        ("src/routes/+page.wisp", "a\r\nb"),
        ("src/routes/+page.rs", long),
    ];
    let code = app("dup-prelude", &files).unwrap();
    assert!(
        code.contains("include!(") && code.find("include!(") < code.find("use ::wisp::prelude::*;"),
        "{code}"
    );
    assert!(!code.contains('\r') && code.contains("\"a\\nb\""), "{code}");
    // An `Err(error(..))` from before `error()` returned the `Result`.
    let old = "\n#[action] fn a() -> Result<()> { Err(error(400, \"no\")) }";
    assert!(
        app("old-error", &page(old))
            .unwrap_err()
            .starts_with("src/routes/+page.rs:2: `error()` returns the `Result`")
    );
}

#[test]
fn attributes_may_be_left_out() {
    let page = ("src/routes/+page.wisp", "<a href={link} title={t}>x</a>");
    let rs = (
        "src/routes/+page.rs",
        "pub struct Data { pub link: Option<String>, pub t: u8 }
pub fn load() -> Data { todo!() }",
    );
    let code = app("optional", &[page, rs]).unwrap();
    assert!(
        code.contains("if let Some(__v) = (&::wisp::rt::Attr(&(link))).get() {"),
        "{code}"
    );
    assert!(code.contains("guard_url"), "{code}");
    // One of two literals is written whole, escaped, at build time; a
    // URL that would run script is left to the runtime's guard.
    let page = (
        "src/routes/+page.wisp",
        "<p class={if on { \"a&\" } else { \"b\" }}>x</p><a href={if on { \"javascript:x\" } else { \"/\" }}>y</a>",
    );
    let rs = (
        "src/routes/+page.rs",
        "pub struct Data { pub on: bool }\npub fn load() -> Data { todo!() }",
    );
    let code = app("either", &[page, rs]).unwrap();
    assert!(
            code.contains(r#"if on { __o.body.push_str(" class=\"a&amp;\""); } else { __o.body.push_str(" class=\"b\""); }"#),
            "{code}"
        );
    assert!(
        code.contains("(&::wisp::rt::Attr(&(if on { \"javascript:x\" }"),
        "{code}"
    );
}

#[test]
fn data_fields_are_in_scope() {
    let rs = "pub struct Data {
    /// how many
    pub n: u32,
    pub title: String,
    pub pair: (u8, Option<&'static str>),
    pub rows: Vec<(u8, u8)>,
    #[allow(dead_code)]
    pub cb: fn(u8) -> u8,
    hidden: u8,
}
pub fn load() -> Data { todo!() }";
    let files = [
        (
            "src/routes/+page.wisp",
            "{n} {title} {#each rows as r}{r.0}{/each}",
        ),
        ("src/routes/+page.rs", rs),
    ];
    let code = app("fields", &files).unwrap();
    for want in [
        "let n = data.n;",
        "let title = &data.title;",
        "let pair = data.pair;",
        "let rows = &data.rows;",
        "let cb = &data.cb;",
        // Private fields too: the template is inside the page's module.
        "let hidden = data.hidden;",
        "pub mod page_0 {",
        "use ::wisp::prelude::*;",
        "pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx, __d: &super::__call::Loaded) {",
        "let data: &super::Data = &__d.0;",
        "let d = page_0::__call::load(cx).await?;",
        "page_0::tpl_page_0::render(__o, cx, &d)",
    ] {
        assert!(
            code.contains(want),
            "{want}
{code}"
        );
    }
}

/// The browser side of a page (with `load` if `loads`) made of `src`.
fn page_client(src: &str, loads: bool) -> Result<Client, String> {
    let t = Tpl {
        id: 7,
        module: "tpl_page_0".into(),
        rel: "src/routes/+page.wisp".into(),
        kind: Kind::Page,
        user: Some(("page_0".into(), loads)),
        data: Vec::new(),
        load_js: None,
        stmts: None,
        t: template::parse(src).unwrap(),
        i18n: false,
        slots: Vec::new(),
    };
    // The script's bare imports are the app's npm packages.
    let deps = vec![("a".into(), "1".into()), ("b".into(), "2".into())];
    let specs = Specs {
        remote: Some("/_app/c/remote.js?v=R".into()),
        lib: Vec::new(),
        lib_hash: "0".into(),
        npm: Npm::new(deps, None),
    };
    let cx = ClientCx {
        comps: &[],
        templates: &[],
        as_client: false,
        specs: &specs,
        load: None,
        release: false,
        maps: true,
        env: &[],
        i18n: None,
        extra: "/_app/c/extra.js",
        remotes: &["user".into(), "save".into(), "gone".into()],
    };
    client(&t, &cx).map(|c| c.expect("the page has browser code"))
}

#[test]
fn remote_functions_in_typescript() {
    let items = rust_scan::scan(
        "#[derive(Json)] struct User { name: String }\n\
             #[remote] fn user(id: u64, note: Option<String>) -> Result<Option<User>> { todo!() }\n\
             #[remote(get)] fn ping() {}",
    )
    .unwrap();
    let remotes: Vec<RemoteFn> = (items.fns.iter())
        .map(|f| RemoteFn {
            f: f.clone(),
            module: "page_0".into(),
            rel: "src/remote.rs".into(),
            types: items.types.clone(),
        })
        .collect();
    let ts = remote_ts(&remotes).unwrap();
    for want in [
        "declare function user(id: number, note?: string | null): Promise<User>;",
        "declare function ping(): Promise<void>;",
        "declare module 'wisp:remote' {\n  export function user(",
        "interface User {",
    ] {
        assert!(ts.contains(want), "{want}\n{ts}");
    }
    let js = remote_js(&remotes);
    assert!(
        js.contains(&format!(
            "export const ping = (...a) => call(\"{}\", 1, [], a);",
            remotes[1].path()
        )),
        "{js}"
    );
    // A release build's, shortened, keeps its exports.
    let small = js::runtime(&js);
    assert!(
        small.len() < js.len() && small.contains("export let ping="),
        "{small}"
    );
}

#[test]
fn scripts_call_remote_functions_without_an_import() {
    let src = "<button on:click=\"save(1)\">x</button><script>let u = await user(5); let gone = 1</script>";
    let c = page_client(src, false).unwrap();
    assert!(
        c.source
            .contains("import { user, save } from \"/_app/c/remote.js?v=R\";\n"),
        "{}",
        c.source
    );
    // None used: no import.
    let c = page_client("<b on:click=\"n++\">x</b><script>let n = 0</script>", false).unwrap();
    assert!(!c.source.contains("remote.js"), "{}", c.source);
}

#[test]
fn typescript_scripts_lib_files_and_page_ts() {
    let files = [
        (
            "src/routes/+page.wisp",
            "<p>{:n}</p>\n<script lang=\"ts\">\n  import { twice, type Num } from '$lib/util'\n  let n: Num = twice(2 as Num)\n</script>",
        ),
        (
            "src/lib/util.ts",
            "export type Num = number\nexport function twice(x: Num): Num {\n  return x * 2\n}\n",
        ),
        (
            "src/routes/+page.ts",
            "export function load({ data }: { data: object }): object {\n  return data\n}\n",
        ),
    ];
    let code = app("ts", &files).unwrap();
    for js in [
        "let n = __wisp_s(twice(2))",
        "export function twice(x) {",
        "export function load({ data }) {",
        "/_app/c/lib/util.ts?v=",
    ] {
        // Its types are spaces now.
        let squeezed = code.replace("\\\"", "\"").replace(' ', "");
        assert!(squeezed.contains(&js.replace(' ', "")), "{js}: {code}");
    }

    // What TypeScript would write code for is an error at its place.
    let err = app(
        "ts-enum",
        &[(
            "src/routes/+page.wisp",
            "<p>{:x}</p>\n<script lang=\"ts\">\n  enum E { A }\n  let x = 1\n</script>",
        )],
    )
    .unwrap_err();
    assert!(
        err.contains("+page.wisp:3:3:") && err.contains("`enum`"),
        "{err}"
    );
    let err = app(
        "ts-lib-enum",
        &[
            ("src/routes/+page.wisp", "<p>hi</p>"),
            ("src/lib/e.ts", "\nexport enum E { A }\n"),
        ],
    )
    .unwrap_err();
    assert!(err.starts_with("src/lib/e.ts:2:8:"), "{err}");
    // Another attribute keeps the script as HTML.
    let code = app(
        "ts-typed",
        &[(
            "src/routes/+page.wisp",
            "<script lang=\"ts\" type=\"module\">let a: number = 1</script>",
        )],
    )
    .unwrap();
    assert!(code.contains("let a: number = 1"), "{code}");
}

#[test]
fn translations_are_checked_and_compiled() {
    let page = "<h1>{t(\"title\")}</h1>\n<p>{t(\"items\", n)}</p>\n<Card />\n<b>{:t('items', k)}</b>\n<script>\n  let k = 1\n</script>";
    let files = [
        ("src/routes/[[lang=locale]]/+page.wisp", page),
        ("src/components/Card.wisp", "<i>{t(\"title\")}</i>"),
        (
            "src/locales/en.json",
            "{\"title\": \"Hi\", \"items\": \"{count, plural, one {# item} other {# items}}\", \"unused\": \"x\"}",
        ),
        (
            "src/locales/fr.json",
            "{\"title\": \"Salut\", \"items\": \"{count, plural, one {# article} other {# articles}}\", \"unused\": \"y\"}",
        ),
    ];
    let code = app("i18n", &files).unwrap();
    for want in [
        "const LOCALES: &'static [&'static str] = &[\"en\", \"fr\"];",
        "pub static K1: [&str; 2] = [\"Hi\", \"Salut\"]; // title",
        "__wisp_i18n::K1[__wisp_l as usize]",
        "::wisp::rt::Count::count(&(n))",
        "pub static J0: [&str; 2]",
        "texts: &[&__wisp_i18n::J0]",
        "__o.lang = ::wisp::rt::pick_locale(cx);",
        "matches!(p0, \"en\" | \"fr\")",
        "let __wisp_l: u8 = __o.lang;",
    ] {
        assert!(code.contains(want), "{want}: {code}");
    }
    // Keys no template uses are checked, not compiled.
    assert!(!code.contains("// unused") && !code.contains("pub static J2"));
    // An unknown key, at its line; values that do not match.
    let bad = |page: &str| {
        let mut f = files;
        f[0].1 = page;
        app("i18n-bad", &f).unwrap_err()
    };
    assert_eq!(
        bad("<p>\n{t(\"nope\")}</p>"),
        "src/routes/[[lang=locale]]/+page.wisp:2: no \"nope\" in src/locales/en.json: add it to every locale"
    );
    assert!(bad("{t(\"items\")}").ends_with("\"items\" needs {count}: it has {count}"));
    assert!(
        bad("<b>{:t('nope')}</b>\n<script>\n</script>")
            .starts_with("src/routes/[[lang=locale]]/+page.wisp:1: no \"nope\""),
    );
    // Browser modules get the text from a page's script.
    let mut lib = files.to_vec();
    lib.push(("src/lib/x.js", "export const x = () =>\n  t('title')\n"));
    assert!(
        app("i18n-lib", &lib)
            .unwrap_err()
            .starts_with("src/lib/x.js:2:3: t('…') shows a message in a .wisp file's script")
    );
    // `[[lang=locale]]` needs locales.
    let err = app(
        "i18n-none",
        &[("src/routes/[[lang=locale]]/+page.wisp", "<p>x</p>")],
    );
    assert!(err.unwrap_err().contains("add src/locales/en.json"));
}

#[test]
fn public_env_in_browser_code() {
    let page = "<p>{:env.PUBLIC_NAME}</p>\n<script>\n  import { api } from '$lib/api.js'\n  let url = env.PUBLIC_API + api\n</script>";
    let files = [
        ("src/routes/+page.wisp", page),
        ("src/lib/api.js", "export const api = env.PUBLIC_PATH\n"),
        (
            "src/routes/+page.js",
            "export const load = () => ({ v: env.PUBLIC_API })\n",
        ),
        (
            ".env",
            "PUBLIC_API=https://api.example\nPUBLIC_NAME=Wisp\nPUBLIC_PATH=/v1\nSECRET=hunter2\n",
        ),
    ];
    for release in [false, true] {
        let code = build("env", &files, release).unwrap();
        for filled in [
            r#"let url = __wisp_s(\"https://api.example\" + api)"#,
            r#"[\"hole\", () => (\"Wisp\")]"#,
            r#"export const api = \"/v1\""#,
            r#"({ v: \"https://api.example\" })"#,
        ] {
            assert!(code.contains(filled), "{filled}: {code}");
        }
        // (A dev build's source maps hold the files as written.)
        assert!(!code.contains("hunter2") && (!release || !code.contains("env.PUBLIC")));
    }
    // A secret, or a name no one set, is an error at its line.
    let secret = page.replace("env.PUBLIC_API", "env.SECRET");
    let err = app(
        "env-secret",
        &[
            files[0],
            files[1],
            files[3],
            ("src/routes/+page.wisp", &secret),
        ],
    )
    .unwrap_err();
    assert!(
        err.starts_with("src/routes/+page.wisp:4: `env.SECRET` is not sent to the browser"),
        "{err}"
    );
    let err = app("env-missing", &[files[0], files[1]]).unwrap_err();
    assert!(
        err.contains("src/lib/api.js:1:20: `env.PUBLIC_PATH` is not set"),
        "{err}"
    );
}

#[test]
fn block_values_are_probed_for_types_in_dev_builds_only() {
    let files = [(
        "src/routes/+page.wisp",
        "---\nlet n = 3;\nlet unread = 1;\n---\n<p>{:n}</p>",
    )];
    let dev = build("probe", &files, false).unwrap();
    // Only what the browser reads: `n`, not `unread`.
    assert!(dev.contains("::wisp::Result::Ok(((&&::wisp::ts::probe(&n)).pick(), ()))"));
    assert!(dev.contains("::wisp::ts::page(&__f, \"src/routes/+page.wisp\", &[\"n\"], __out);"));
    assert!(dev.contains("::wisp::__ts! {\n        fn types() -> String {"));
    assert!(
        !build("probe-release", &files, true)
            .unwrap()
            .contains("__ts!")
    );
}

#[test]
fn typescript_files_for_tsc() {
    let files = [
        (
            "src/routes/+page.wisp",
            "---\n#[derive(Json)]\nstruct Item { name: String, price: Option<u32> }\nlet items: Vec<Item> = Vec::new();\nlet other = 1;\n---\n\
                 <p>{:items.length}</p>\n<script lang=\"ts\">\n  import { cart } from '$lib/cart'\n  let other: number = $cart\n</script>",
        ),
        (
            "src/components/Card.wisp",
            "{@props title: &str, count: u32 = 0}\n<b>{:title}</b><script lang=\"ts\">let n: number = count</script>",
        ),
        (
            "src/routes/plain/+page.wisp",
            "<p>{:x}</p><script>let x = 1</script>",
        ),
    ];
    let out = in_dir("types", &files, |root| {
        types(
            &Input {
                root,
                release: false,
                maps: false,
                prerendered: None,
            },
            "",
        )
    })
    .unwrap()
    .0;
    let file = |p: &str| &out.iter().find(|(n, _)| n == p).unwrap().1;
    assert!(file("wisp.d.ts").contains("declare function $state<T>"));
    // On the lines of the file, with what it reads declared after it.
    let page = file("src/routes/+page.wisp.ts");
    assert_eq!(
        page.lines().nth(8),
        Some("  import { cart } from '$lib/cart'")
    );
    assert!(
        page.contains(
            "declare const data: { items: Item[]; other: any };\ndeclare const items: Item[];\n"
        ) && !page.contains("declare const other")
            && page
                .contains("declare let $cart: typeof cart extends { value: infer V } ? V : never;")
            && page
                .contains("export interface Item {\n  name: string;\n  price?: number | null;\n}"),
        "{page}"
    );
    let card = file("src/components/Card.wisp.ts");
    assert!(
        card.starts_with(&format!("\n{}let n: number = count", " ".repeat(33)))
            && card.contains("declare const title: string;\ndeclare const count: number;"),
        "{card:?}"
    );
    assert_eq!(out.len(), 3);
}

#[test]
fn source_maps_in_dev_and_on_request() {
    let files = [
        (
            "src/routes/+page.wisp",
            "<p>{:n}</p>\n<script>\n  import { a } from '$lib/a.js'\n  let n = a\n</script>",
        ),
        ("src/lib/a.js", "export const a = 1\n"),
        (
            "src/routes/+page.js",
            "export function load({ data }) { return data }\n",
        ),
    ];
    let dev = app("maps", &files).unwrap();
    for m in ["t1.js", "lib/a.js", "t1.load.js"] {
        let name = m.rsplit('/').next().unwrap();
        assert!(dev.contains(&format!("\"/_app/c/{m}.map\"")), "{m}: {dev}");
        assert!(
            dev.contains(&format!("//# sourceMappingURL={name}.map")),
            "{m}"
        );
    }
    assert!(
        dev.contains(r#"\"sources\":[\"wisp:///src/lib/a.js\"]"#),
        "{dev}"
    );
    assert!(!dev.contains("sourceURL"));
    let release = build("maps-release", &files, true).unwrap();
    assert!(
        !release.contains(".map") && release.contains("sourceURL=wisp:///src/routes/+page.wisp")
    );
    let asked = in_dir("maps-asked", &files, |root| {
        generate(&Input {
            root,
            release: true,
            maps: true,
            prerendered: None,
        })
        .map(|o| o.code)
    });
    assert!(asked.unwrap().contains("\"/_app/c/t1.js.map\""));

    // What a browser reads of a stack trace: the line a `throw` runs on
    // is, through the map, its line of the file.
    let src = "<h1>Maps</h1>\n<button on:click=\"boom()\">Boom</button>\n<p>{:n}</p>\n\n<script>\n  import { twice } from 'a'\n  let n = twice(2)\n  function boom() {\n    throw new Error('boom')\n  }\n</script>\n";
    let c = page_client(src, false).unwrap();
    let map = sourcemap::encode("t7.js", "wisp:///x.wisp", src, &c.lines);
    let segs = sourcemap::tests::decode(sourcemap::tests::mappings(&map));
    let at = |text: &str, s: &str| s.lines().position(|l| l.contains(text)).unwrap();
    let thrown = at("throw new", &c.source);
    assert_eq!(segs[thrown], [[0, 0, at("throw new", src) as i64, 0]]);
    let button = at("(boom())", &c.source);
    assert_eq!(segs[button], [[0, 0, 1, 0]]);
}

/// What `wisp dev` swaps into a running build: a change to browser
/// code or text alone leaves the rest of the program as it was (lines
/// moving included); one to Rust does not. Templates mark what they
/// render in dev builds, and a module names its file and a top-level
/// statement that may not run twice.
#[test]
fn hot_tells_what_needs_a_compile() {
    let hot = |page: &str| {
        in_dir("hot", &[("src/routes/+page.wisp", page)], |root| {
            super::hot(&Input {
                root,
                release: false,
                maps: true,
                prerendered: None,
            })
            .unwrap()
        })
    };
    let base =
        "<p>{1 + 1}</p><button on:click=\"n++\">Hi {:n}</button>\n<script>let n = 0</script>";
    let a = hot(base);
    let script = hot(&base.replace("let n = 0", "\n\nlet n = 0\nfunction f() {}"));
    let text = hot(&base.replace("Hi", "Hello"));
    let rust = hot(&base.replace("1 + 1", "1 + 2"));
    let first = hot(&base.replace("let n = 0", "let n = 1"));
    assert_eq!(a.rust, script.rust);
    assert_eq!(a.rust, text.rust);
    assert_ne!(a.rust, rust.rust);
    assert_ne!(a.rust, first.rust, "the first paint has it");
    let module = |h: &Hot| {
        h.files
            .iter()
            .find(|f| f.0 == "/_app/c/t1.js")
            .unwrap()
            .2
            .clone()
    };
    assert_ne!(module(&a), module(&script));
    assert!(module(&a).contains("file: \"src/routes/+page.wisp\""));
    assert!(!module(&a).contains("effect:"));
    let effect = hot(&base.replace("let n = 0", "let n = 0\nstart()"));
    assert!(module(&effect).contains("effect: 3"), "{}", module(&effect));
    let page = &a.templates[1];
    assert_eq!(
        (page.rel.as_str(), page.chunks[0].as_str()),
        ("src/routes/+page.wisp", "<p>")
    );
    assert_ne!(page.shape, script.templates[1].shape);

    // A component's text, whose shape has it.
    let comp = |text: &str| {
        let files = [
            ("src/routes/+page.wisp", "<Note text=\"a\" />"),
            ("src/components/Note.wisp", text),
        ];
        in_dir("hot-comp", &files, |root| {
            super::hot(&Input {
                root,
                release: false,
                maps: true,
                prerendered: None,
            })
            .unwrap()
        })
    };
    let (a, b) = (
        comp("{@props text: &str}\n<p>Note: {text}</p>"),
        comp("{@props text: &str}\n<p>A note: {text}</p>"),
    );
    assert_eq!(a.templates[1].shape, b.templates[1].shape);
    assert_eq!(a.rust, b.rust);

    let page = [("src/routes/+page.wisp", "<p>Hi</p>")];
    let mark = "if ::wisp::rt::marks() { __o.body.push_str(\"<!--w:src/routes/+page.wisp-->\"); }";
    assert!(build("marks", &page, false).unwrap().contains(mark));
    assert!(!build("marks", &page, true).unwrap().contains("marks()"));
}

#[test]
fn client_each_with_a_filter() {
    let src = "<script>let xs = [1, 2]; let q = 1</script>
{:#each xs as n if n > q}<i>{:n}</i>{/each}";
    let c = page_client(src, true).unwrap();
    assert!(c.source.contains(".filter((n) => n > q)"), "{}", c.source);
}

#[test]
fn html_and_const_in_client_blocks() {
    let src = "<script>\n  let items = [1, 2]\n  let h = '<b>x</b>'\n</script>\n{:#each items as n}{:@const sq = n * n}<i>{:sq}</i>{:/each}<div>{:@html h}</div>";
    let c = page_client(src, true).unwrap();
    assert!(c.source.contains("[\"html\", "), "{}", c.source);
    assert!(c.source.contains("[n * n]"), "{}", c.source);
    assert!(c.source.contains("import \"/_app/c/extra.js\";"));
    // A page without {:@html} does not load extra.js for it.
    let c = page_client("<script>let h = 'x'</script><p>{:h}</p>", true).unwrap();
    assert!(!c.source.contains("extra.js"), "{}", c.source);
}

#[test]
fn a_script_exports_its_snapshot_alone() {
    let src = "<p>{:n}</p>\n<script>\n  let n = 0\n  export const snapshot = { capture: () => n, restore: (v) => (n = v) }\n</script>";
    let c = page_client(src, true).unwrap();
    assert!(
        c.source
            .contains("\n         const snapshot = { capture: () => n.v,"),
        "{}",
        c.source
    );
    assert!(c.source.contains("], snap: snapshot };"), "{}", c.source);
    assert!(
        c.source.contains("import \"/_app/c/extra.js\";"),
        "extra.js keeps snapshots"
    );
    let Err(err) = page_client("<script>\n  export let x = 1\n</script>", true) else {
        panic!("exports x");
    };
    assert!(
        err.contains(":2:3: a script exports only `export const snapshot"),
        "{err}"
    );
}

#[test]
fn modules() {
    let src = "<p>hi</p>\n<button on:click=\"toggle\" :hidden=\"data.done\">x</button>\n\n\n\n\n\n\n<script>\n  import a from 'a'\n  import {\n    b } from \"b\";\n  let open = false\n  function toggle() { open = !open }\n</script>";
    let c = page_client(src, true).unwrap();
    assert_eq!(c.id, "t7");
    let head = format!(
        "import {{ define }} from \"/_app/live.js?v={}\";\nimport a from \"https://esm.sh/a@1?target=es2022\"\nimport {{\n    b }} from \"https://esm.sh/b@2?target=es2022\";\n\
             define(\"t7\", function (__wisp_p, __wisp_h) {{ const {{ {HELPERS} }} = __wisp_h; {{ const {{ data }} = __wisp_props(__wisp_p, [\"data\"]); {{\n",
        crate::runtime_version()
    );
    assert!(c.source.starts_with(&head), "{}", c.source);
    // Blank lines, then the script with its imports blanked out: `let
    // open` is on line 13 of the file, and of the module.
    let blanked = format!(
        "\n\n\n\n{}\n{}\n{}\n  let open = __wisp_s(false)\n",
        " ".repeat(19),
        " ".repeat(10),
        " ".repeat(17)
    );
    assert!(c.source[head.len()..].starts_with(&blanked), "{}", c.source);
    assert_eq!(
        c.source
            .lines()
            .position(|l| l == "  let open = __wisp_s(false)"),
        Some(12)
    );
    // A dev build hands the devtools the file, its state and their lines.
    let tail = "  function toggle() { open.v = !open.v }\n\
                    globalThis.__wisp_dev?.state(\"src/routes/+page.wisp\", { open }, { open: 13 });\nreturn { g: [\n  [[\"on\", \"click\", 8192, (_, event) => toggle(event)], [\"attr\", \"hidden\", () => (data.v.done)]],\n] };\n} } }, { file: \"src/routes/+page.wisp\" });\n\
                    //# sourceMappingURL=t7.js.map\n";
    assert!(c.source.ends_with(tail), "{}", c.source);
    // Its map: the imports and the script on their lines of the file
    // (0-based), the group on its element's, Wisp's own lines on none.
    assert_eq!(c.lines.len(), c.source.lines().count() - 1);
    assert_eq!(
        c.lines[..5],
        [None, Some((9, 0)), Some((10, 0)), Some((11, 0)), None]
    );
    assert_eq!(c.lines[12..14], [Some((12, 0)), Some((13, 0))]);
    let group = c
        .source
        .lines()
        .position(|l| l.starts_with("  [["))
        .unwrap();
    assert_eq!(c.lines[group], Some((1, 0)));
    assert_eq!(c.lines[group + 1..], [None, None]);
    assert_eq!(
        c.blob,
        [
            Piece::Text("{\"data\":{\"done\":".into()),
            Piece::Value {
                expr: "data.done".into(),
                line: 2
            },
            Piece::Text("}}".into())
        ]
    );
    assert_eq!(c.hash, image::hash(c.source.as_bytes()));

    // Without `load`, `data` is JavaScript's; without a script, the
    // module only has bindings.
    let c = page_client("<p :text=\"data\"></p>", false).unwrap();
    assert!(
        !c.source.contains("__wisp_props(") && c.source.contains("[[\"text\", () => (data)]]"),
        "{}",
        c.source
    );
    assert_eq!(c.blob, [Piece::Text("{}".into())]);
}

#[test]
fn handlers_and_bindings() {
    let src = "<input on:input.debounce.300ms=\"q = event.target.value\" on:keydown.enter=\"if (ok) send(); else warn()\" \
                   on:click=\"a(); b() // why\" bind:value=\"form.q\" bind:this=\"el\" style:--x=\"x\" transition:fly=\"{ y: 4 }\" \
                   use:tip=\"'hi'\" use:focus class:on=\"{ a: 1 }.a\">";
    let c = page_client(src, false).unwrap();
    let group = c.source.lines().find(|l| l.starts_with("  [[")).unwrap();
    let want = [
        "[\"on\", \"input\", 8192, (_, event) => (q = event.target.value), null, 300]",
        "[\"on\", \"keydown\", 8192, (_, event) => { if (ok) send(); else warn() }, [\"enter\"]]",
        "[\"on\", \"click\", 8192, (_, event) => { a(); b() // why\n }]",
        "[\"bind\", \"value\", () => (form.q), (_, __wisp_v) => { (form.q) = __wisp_v }]",
        // No script declares `el`: the binding does, as state.
        "[\"bind\", \"this\", null, (_, __wisp_v) => { (el.v) = __wisp_v }]",
        "[\"style\", \"--x\", () => (x)]",
        "[\"transition\", \"fly\", () => ({ y: 4 }), 0]",
        "[\"use\", () => tip, () => ('hi')]",
        "[\"use\", () => focus, null]",
        "[\"class\", \"on\", () => ({ a: 1 }.a)]",
    ];
    let got = c
        .source
        .split("return { g: [\n  [")
        .nth(1)
        .unwrap()
        .split("],\n]")
        .next()
        .unwrap();
    assert_eq!(got, want.join(", "), "{group}");
}

#[test]
fn loop_values_are_sent_per_element() {
    let src = "{#each data.keys as key, i}\n{@const n: u8 = 1}\n<b on:click=\"press(key.letter, i, n)\" :title=\"key.mark.label() + data.x.length\"></b>{/each}\
                   {#if let Some((a, b)) = data.pair}<i :text=\"a + b\"></i>{/if}\
                   {#match data.m}{:case Mark::Hit { at: spot, .. } if spot > 1}<u :text=\"spot\"></u>{:case _}{/match}\
                   <template each=\"key in keys\"><s :text=\"key\"></s></template><p :text=\"key\"></p>";
    let c = page_client(src, true).unwrap();
    let text = |ps: &[Piece]| {
        ps.iter()
            .map(|p| match p {
                Piece::Text(t) => t.clone(),
                Piece::Value { expr, .. } => format!("<{expr}>"),
            })
            .collect::<String>()
    };
    assert_eq!(
        text(&c.locals[0]),
        "{\"key\":{\"letter\":<key.letter>,\"mark\":<key.mark>},\"i\":<i>,\"n\":<n>}"
    );
    assert_eq!(text(&c.locals[1]), "{\"a\":<a>,\"b\":<b>}");
    assert_eq!(text(&c.locals[2]), "{\"spot\":<spot>}");
    // A client `<template each>` name is the browser's; outside the
    // loop, `key` is JavaScript's too.
    assert!(c.locals[3..].iter().all(Vec::is_empty));
    assert_eq!(text(&c.blob), "{\"data\":{\"x\":<data.x>}}");
    assert!(
        c.source.contains(
            "[\"on\", \"click\", 8192, ({ key, i, n }, event) => (press(key.letter, i, n))]"
        ),
        "{}",
        c.source
    );
    assert!(
        c.source
            .contains("[\"attr\", \"title\", ({ key }) => (key.mark.label() + data.v.x.length)]"),
        "{}",
        c.source
    );
    assert!(
        c.source.contains("[\"text\", ({ key }) => (key)]"),
        "{}",
        c.source
    );
}

#[test]
fn a_toggle_or_a_count_is_state() {
    assert_eq!(state_of("open = !open"), Some(("open", " = false")));
    assert_eq!(state_of(" n++ "), Some(("n", " = 0")));
    assert_eq!(state_of("--n;"), Some(("n", " = 0")));
    assert_eq!(state_of("open = !shut"), None);
    assert_eq!(state_of("a.b++"), None);
    assert_eq!(state_of("n = 1"), None);
    assert_eq!(state_of("open == !open"), None);
}

#[test]
fn server_names_the_script_declares_are_refused() {
    let err = page_client("<p>x</p>\n<script>\n  let a = 1, data = 2\n</script>", true)
        .err()
        .unwrap();
    assert_eq!(
        err,
        "src/routes/+page.wisp:3:14: `data` is both a server value and a script variable; rename one"
    );
    let err = page_client(
        "{#each xs as guess}<p :text=\"guess\"></p>{/each}<script>function guess() {}</script>",
        false,
    )
    .err()
    .unwrap();
    assert!(
        err.starts_with("src/routes/+page.wisp:1:65: `guess` is both"),
        "{err}"
    );
    // Unused, a loop name is no one's business.
    assert!(
        page_client(
            "{#each xs as guess}<p :text=\"x\"></p>{/each}<script>let guess</script>",
            false
        )
        .is_ok()
    );
}

#[test]
fn components_send_their_props() {
    let card = (
        "src/components/Card.wisp",
        "{@props title: &str, n: u8 = 0}\n<h2 :text=\"title.toUpperCase()\">{title}</h2><script>const twice = n * 2</script>",
    );
    let page = (
        "src/routes/+page.wisp",
        "<Card title=\"a\" /><Card title=\"b\" />",
    );
    let code = app("live-props", &[card, page]).unwrap();
    // The script reads `n`, then the directive reads `title`.
    assert!(code.contains("__b.push_str(\"{\\\"n\\\":\");\n            ::wisp::rt::json(__b, &(n)); // src/components/Card.wisp:2"), "{code}");
    assert!(code.contains("::wisp::rt::json(__b, &(title)); // src/components/Card.wisp:2\n            __b.push_str(\"}\");"), "{code}");
    assert!(
        code.contains("\"/_app/c/t1.js\" => Some(&tpl_component_0::__WISP_CLIENT),"),
        "{code}"
    );
    assert!(
        code.contains("const { n, title } = __wisp_props(__wisp_p, [\\\"n\\\", \\\"title\\\"]);"),
        "{code}"
    );
}

#[test]
fn snippets_are_props_of_components_the_browser_draws() {
    let table = (
        "src/components/Table.wisp",
        "{@props items: Vec<u32>, row: Snippet<&u32, usize>}\n<ul>{:#each items as it, i}<li>{:@render row(it, i)}</li>{:/each}</ul>",
    );
    for (n, page) in [
        "<Table items={:xs} {row} />",
        "<Table items={:xs}>{#snippet row(x, i)}<b>{:i}: {:x}</b>{/snippet}</Table>",
    ]
    .iter()
    .enumerate()
    {
        let src = if n == 0 {
            format!(
                "{{#snippet row(x, i)}}<b>{{:i}}: {{:x}}</b>{{/snippet}}{page}<script>let xs = [1, 2]</script>"
            )
        } else {
            format!("{page}<script>let xs = [1, 2]</script>")
        };
        let code = app("snippet-props", &[table, ("src/routes/+page.wisp", &src)]).unwrap();
        // The block before the tag, and the draw in the component.
        assert!(
            code.contains("[\\\"snip\\\", \\\"row\\\", [\\\"x\\\", \\\"i\\\"]]"),
            "{code}"
        );
        assert!(code.contains("[\\\"draw\\\", "), "{code}");
        assert!(
            !code.contains("Table::paint("),
            "no first paint when it is given a snippet"
        );
    }
    let bad = |page: &str| {
        app(
            "snippet-bad",
            &[
                (
                    "src/components/Table.wisp",
                    "{@props items: Vec<u32>}\n<p>{:items.length}</p>",
                ),
                ("src/routes/+page.wisp", page),
            ],
        )
        .unwrap_err()
    };
    let bad =
        bad("{#snippet row(x)}{/snippet}<Table items={:xs} {row} /><script>let xs = []</script>");
    assert!(bad.contains("has no prop `row`"), "{bad}");
}

#[test]
fn props_rune_and_islands() {
    let item = (
        "src/components/Item.wisp",
        "{@props label: &str, count: u32 = 0}\n<button on:click=\"count++\">{:label}</button>\n\
             <script>\n  let { label, count = $bindable(1) } = $props()\n</script>",
    );
    let page = (
        "src/routes/+page.wisp",
        "<Item label=\"a\" client:visible /><Item label=\"b\" client:media=\"(min-width: 800px)\" /><Item label=\"c\" client:load />",
    );
    let code = app("runes-props", &[item, page]).unwrap();
    // Every prop the rune names is sent; its default is the browser's.
    assert!(
            code.contains(
                "const { label, count } = __wisp_props(__wisp_p, [\\\"label\\\", \\\"count\\\"], { count: () => (1) });"
            ),
            "{code}"
        );
    assert!(code.contains("::wisp::rt::live_how(__o, \"v\");"), "{code}");
    assert!(
        code.contains("::wisp::rt::live_how(__o, \"m(min-width: 800px)\");"),
        "{code}"
    );
    assert_eq!(code.matches("live_how").count(), 2, "{code}");

    let err = |files: &[(&str, &str)]| app("runes-bad", files).err().unwrap();
    let bad = err(&[
        ("src/components/Plain.wisp", "<p>hi</p>"),
        ("src/routes/+page.wisp", "<Plain client:idle />"),
    ]);
    assert!(bad.contains("<Plain> has no browser code"), "{bad}");
    // On an element, it and what is inside wait.
    let code = app(
        "runes-el",
        &[(
            "src/routes/+page.wisp",
            "<p client:visible on:click=\"n++\">{:n}</p><script>let n = 0</script>",
        )],
    )
    .unwrap();
    assert!(
        code.contains("[\\\"wait\\\", \\\"v\\\"], [\\\"on\\\""),
        "{code}"
    );
    let bad = err(&[
        ("src/components/Item.wisp", item.1),
        ("src/routes/+page.wisp", "<Item label=\"a\" client:soon />"),
    ]);
    assert!(bad.contains("client:soon"), "{bad}");
    // Once a component names its props, only a $bindable one may be bound.
    let bad = err(&[
        (
            "src/components/Item.wisp",
            "{@props label: &str, count: u32 = 0}\n<b>{:label}</b><script>let { label, count } = $props()</script>",
        ),
        (
            "src/routes/+page.wisp",
            "<Item label={:\"x\"} bind:count=\"n\" /><script>let n = 0</script>",
        ),
    ]);
    assert!(bad.contains("is not bindable"), "{bad}");
    let bad = err(&[
        (
            "src/components/Item.wisp",
            "{@props label: &str}\n<b>{:label}</b><script>let { label, size } = $props()</script>",
        ),
        ("src/routes/+page.wisp", "<Item label=\"x\" />"),
    ]);
    assert!(
        bad.contains("src/components/Item.wisp:2:") && bad.contains("`size` is not a prop"),
        "{bad}"
    );
    let bad = err(&[(
        "src/routes/+page.wisp",
        "<p>{:x}</p><script>let { x } = $props()</script>",
    )]);
    assert!(bad.contains("for components"), "{bad}");
    let bad = err(&[("src/routes/+page.wisp", "<p :text=\"$state(1)\"></p>")]);
    assert!(
        bad.contains("src/routes/+page.wisp:1: `$state` goes in the script"),
        "{bad}"
    );
}

#[test]
fn patterns_bind_names() {
    assert_eq!(pattern_names("(k, v)"), ["k", "v"]);
    assert_eq!(pattern_names("Some(Point { x, y: py, .. })"), ["x", "py"]);
    assert_eq!(pattern_names("std::option::Option::Some(ref mut x)"), ["x"]);
    assert_eq!(pattern_names("n @ 1..=5u8"), ["n"]);
    assert_eq!(
        pattern_names("_ | Status::Draft | \"a\" | 'b'"),
        [] as [&str; 0]
    );
    assert_eq!(guardless("Some(x) if x > 1"), "Some(x)");
    assert_eq!(let_pattern(" Some(u) = data.user"), "Some(u)");
}

#[test]
fn json_trees() {
    let p = |s: &str, line: u32| (s.split('.').map(String::from).collect::<Vec<_>>(), line);
    let tree = json_tree(&[
        p("data.a.b", 1),
        p("data.c", 2),
        p("data.a", 3),
        p("data.c", 4),
        p("data.type", 5),
    ]);
    assert_eq!(
        tree,
        [
            Piece::Text("{\"data\":{\"c\":".into()),
            Piece::Value {
                expr: "data.c".into(),
                line: 2
            },
            Piece::Text(",\"a\":".into()),
            Piece::Value {
                expr: "data.a".into(),
                line: 3
            },
            Piece::Text(",\"type\":".into()),
            Piece::Value {
                expr: "data.r#type".into(),
                line: 5
            },
            Piece::Text("}}".into()),
        ]
    );
    assert_eq!(
        server_path(&["data".into(), "list".into(), "length".into()]),
        ["data", "list"]
    );
    assert_eq!(server_path(&["data".into(), "$x".into()]), ["data"]);
}

#[test]
fn path_encoding() {
    assert_eq!(encode_path("über uns"), "%C3%BCber%20uns");
    assert_eq!(
        encode_path("a-b_c.d~e!$&'()*+,;=:@"),
        "a-b_c.d~e!$&'()*+,;=:@"
    );
}

#[test]
fn imports_are_rewritten() {
    let v = crate::runtime_version();
    let src = "import { store } from 'wisp'\nimport a from '$lib/a.js'\nexport * from '../b.js'\nimport './c.js'\nimport x from 'https://esm.sh/x'\nconst y = import('$lib/y.js')\nconst s = 'wisp'";
    let deps = vec![
        ("canvas-confetti".into(), "1.9.3".into()),
        ("@s/p".into(), "2.0.0".into()),
    ];
    let specs = |vendor| Specs {
        remote: None,
        lib: ["a.js", "b.js", "sub/c.js", "y.js", "t.ts"]
            .map(String::from)
            .to_vec(),
        lib_hash: "H".into(),
        npm: Npm::new(deps.clone(), vendor),
    };
    let dev = specs(None);
    let out = rewrite_specifiers(src, &dev, Some("lib/sub")).unwrap();
    assert_eq!(
        out,
        format!(
            "import {{ store }} from \"/_app/live.js?v={v}\"\nimport a from \"/_app/c/lib/a.js?v=H\"\nexport * from \"/_app/c/lib/b.js?v=H\"\n\
                 import \"/_app/c/lib/sub/c.js?v=H\"\nimport x from 'https://esm.sh/x'\nconst y = import(\"/_app/c/lib/y.js?v=H\")\nconst s = 'wisp'"
        )
    );
    // From no file, a relative path is left as written.
    assert_eq!(
        rewrite_specifiers("import './c.js'", &dev, None).unwrap(),
        "import './c.js'"
    );
    // From a page's, it is src/lib's file; `x.js` may be `x.ts`.
    assert_eq!(
        rewrite_specifiers(
            "import('../../lib/sub/c.js'); import '$lib/t.js'",
            &dev,
            Some("routes/blog")
        )
        .unwrap(),
        "import(\"/_app/c/lib/sub/c.js?v=H\"); import \"/_app/c/lib/t.ts?v=H\""
    );
    for (spec, want) in [
        (
            "./x.js",
            "`./x.js` is src/routes/blog/x.js; the browser loads only src/lib's files",
        ),
        ("../../../x.js", "`../../../x.js` is outside src"),
        (
            "$lib/nope.js",
            "`$lib/nope.js`: there is no src/lib/nope.js",
        ),
    ] {
        let err = rewrite_specifiers(&format!("import('{spec}')"), &dev, Some("routes/blog"))
            .unwrap_err();
        assert!(err.starts_with(want), "{err}");
    }
    // A package name is the package: from esm.sh in dev, from the app
    // in a release build; one package.json does not list is an error.
    let npm = "import confetti from 'canvas-confetti'\nimport { q } from '@s/p/sub'\nconst m = import('canvas-confetti')";
    assert_eq!(
        rewrite_specifiers(npm, &dev, None).unwrap(),
        "import confetti from \"https://esm.sh/canvas-confetti@1.9.3?target=es2022\"\n\
             import { q } from \"https://esm.sh/@s/p@2.0.0/sub?target=es2022\"\n\
             const m = import(\"https://esm.sh/canvas-confetti@1.9.3?target=es2022\")"
    );
    // A release build's: a stub's module, or the module itself.
    let dir = std::env::temp_dir().join(format!("wisp-npm-specs-{}", std::process::id()));
    fs::create_dir_all(dir.join("@s/p@2.0.0")).unwrap();
    let stub = "export * from \"/_app/c/npm/canvas-confetti@1.9.3/es2022/canvas-confetti.mjs\";";
    fs::write(dir.join("canvas-confetti@1.9.3_target_es2022.js"), stub).unwrap();
    fs::write(
        dir.join("@s/p@2.0.0/sub_target_es2022.js"),
        "export const q = 1;",
    )
    .unwrap();
    let out = rewrite_specifiers(npm, &specs(Some(dir.clone())), None);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        out.unwrap(),
        "import confetti from \"/_app/c/npm/canvas-confetti@1.9.3/es2022/canvas-confetti.mjs\"\n\
             import { q } from \"/_app/c/npm/@s/p@2.0.0/sub_target_es2022.js\"\n\
             const m = import(\"/_app/c/npm/canvas-confetti@1.9.3/es2022/canvas-confetti.mjs\")"
    );
    let err = rewrite_specifiers("import 'left-pad'", &dev, None).unwrap_err();
    assert!(err.contains("`wisp add left-pad`"), "{err}");
}

#[test]
fn client_components_are_checked() {
    let comp = (
        "src/components/Card.wisp",
        "{@props title: &str}\n<p>{title}</p>",
    );
    let page = (
        "src/routes/+page.wisp",
        "{:#each xs as x}<Card title={:x} />{:/each}<script>let xs = []</script>",
    );
    assert!(
        app("client-comp-server", &[comp, page])
            .unwrap_err()
            .contains("its markup has server code")
    );
    let comp = (
        "src/components/Card.wisp",
        "{@props title: &str}\n<p>{:title}</p>",
    );
    let bad = ("src/routes/+page.wisp", "<Card title={:1} nope={:2} />");
    assert!(
        app("client-comp-prop", &[comp, bad])
            .unwrap_err()
            .contains("has no prop `nope`")
    );
    let code = app("client-comp", &[comp, page]).unwrap();
    assert!(
            code.contains(
                "{ html: \\\"<p><template data-w=\\\\\\\"0\\\\\\\"></template><!----></p>\\\", file: \\\"src/components/Card.wisp\\\" }"
            ),
            "{code}"
        );
}

#[test]
fn slots_loading_and_interceptions_are_checked() {
    let lay = (
        "src/routes/+layout.wisp",
        "<main>{@render children()}{@render modal()}</main>",
    );
    let page = ("src/routes/+page.wisp", "<p>hi</p>");
    let e = app("slot-missing", &[lay, page]).unwrap_err();
    assert!(
        e.contains("{@render modal()} draws a slot") && e.contains("no folder @modal"),
        "{e}"
    );
    let slot = ("src/routes/@modal/+page.wisp", "");
    let cut = ("src/routes/@modal/(.)nope/+page@.wisp", "x");
    let e = app("cut-nowhere", &[lay, page, slot, cut]).unwrap_err();
    assert!(
        e.contains("it intercepts /nope, which is not a page"),
        "{e}"
    );
    let lay = (
        "src/routes/+layout.wisp",
        "<main>{@render children()}</main>",
    );
    let load = |src| ("src/routes/+loading.wisp", src);
    assert!(app("loading-ok", &[lay, page, load("<i>l</i>")]).is_ok());
    let e = app("loading-script", &[lay, page, load("<SCRIPT>1</SCRIPT>")]).unwrap_err();
    assert!(e.contains("a loading view is static HTML"), "{e}");
}

#[test]
fn a_component_that_renders_itself_forever_is_refused() {
    let page = ("src/routes/+page.wisp", "<Aa />");
    let aa = |src: &'static str| ("src/components/Aa.wisp", src);
    let bb = ("src/components/Bb.wisp", "<Aa />");
    let e = app("self-forever", &[page, aa("<p>x</p><Aa />")]).unwrap_err();
    assert!(e.contains("<Aa> renders itself"), "{e}");
    let e = app("loop-forever", &[page, aa("<Bb />"), bb]).unwrap_err();
    assert!(e.contains("<Aa> -> <Bb> -> <Aa>"), "{e}");
    // A condition that is always true is the author's: no overflow.
    assert!(app("true-forever", &[page, aa("{#if true}<Aa />{/if}")]).is_ok());
}

#[test]
fn components_may_render_themselves() {
    let page = (
        "src/routes/+page.wisp",
        "<Tree node={:{ name: 'r', kids: [] }} />",
    );
    let forever = (
        "src/components/Tree.wisp",
        "{@props node: &str}\n<p>{:node.name}</p><Tree node={:node} />",
    );
    assert!(
        app("tree-forever", &[forever, page])
            .unwrap_err()
            .contains("nothing to stop it")
    );
    let tree = (
        "src/components/Tree.wisp",
        "{@props node: &str}\n<p>{:node.name}</p>{:#each node.kids as kid}<Tree node={:kid} />{/each}",
    );
    let code = app("tree", &[tree, page]).unwrap();
    // It paints itself, a level deeper each time, from its props' JSON.
    for want in [
        "pub fn paint(__o: &mut ::wisp::Out, __p: &[::wisp::rt::Js<'_>]",
        r#"__p[0].get("name").text(&mut __o.body);"#,
        r#"for (__wisp_k0, __wisp_e0) in __p[0].get("kids").items().enumerate() {"#,
        "super::tpl_component_0::paint(__o, &[__wisp_e0], &|__o: &mut ::wisp::Out| {",
        "}, __wisp_d + 1);",
        r#"super::tpl_component_0::paint(__o, &[::wisp::rt::Js("{\"name\":\"r\",\"kids\":[]}")], "#,
    ] {
        assert!(code.contains(want), "{want} in {code}");
    }
    // Two that render each other import each other.
    let a = (
        "src/components/Ping.wisp",
        "{@props n: u8}\n{:#if n}<Pong n={:n} />{/if}",
    );
    let b = (
        "src/components/Pong.wisp",
        "{@props n: u8}\n{:#if n}<Ping n={:n} />{/if}",
    );
    let page = ("src/routes/+page.wisp", "<Ping n={:1} />");
    let code = app("mutual", &[a, b, page]).unwrap();
    assert!(!code.contains("@wisp/comp/"), "{code}");
}

#[test]
fn first_paint_expressions() {
    let known = |path: &[String]| match path[0].as_str() {
        "d" => Pv::Val("D".into()).member(&path[1..]),
        _ => None,
    };
    let paint = |js: &str| match paint_expr(js, &mut known.clone()) {
        Some(Pv::Val(e) | Pv::Num(e) | Pv::Bool(e) | Pv::Test(e)) => e,
        None => "-".into(),
    };
    assert_eq!(paint("d.a.b"), "D.get(\"a\").get(\"b\")");
    assert_eq!(paint("d.list.length"), "D.get(\"list\").length()");
    assert_eq!(paint("!d.x"), "!(D.get(\"x\").truthy())");
    assert_eq!(
        paint("(d.x && !d.y) || d.z.length"),
        "((D.get(\"x\").truthy() && !(D.get(\"y\").truthy())) || D.get(\"z\").length().is_some_and(|n| n > 0))"
    );
    assert_eq!(
        paint("[1, 'a\\'b', { k: -2.50, 'q': [true, null,] }]"),
        "::wisp::rt::Js(\"[1,\\\"a'b\\\",{\\\"k\\\":-2.5,\\\"q\\\":[true,null]}]\")"
    );
    assert_eq!(paint("undefined"), "::wisp::rt::Js(\"null\")");
    for no in [
        "f(d)",
        "d.a()",
        "d[0]",
        "d?.a",
        "d.a + 1",
        "x",
        "d.a === 1",
        "{ k: x }",
        "`t`",
    ] {
        assert_eq!(paint(no), "-", "{no}");
    }
}
