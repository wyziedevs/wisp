//! The compiler's public pieces on their own: reading sources, parsing a
//! `.wisp` file, hot reload's chunks, the route scan, the Rust scan, the
//! JavaScript minifier.

use crate::common::Project;
use std::fs;
use std::path::PathBuf;
use wisp_build::routes::{self, Seg};
use wisp_build::rust_scan::{self, Returns};
use wisp_build::{fnv1a, hot_chunks, minify_js, parse_wisp, read_source, template};

fn temp(name: &str, bytes: &[u8]) -> (Project, PathBuf) {
    let p = Project::new(&[]);
    let file = p.root().join(name);
    fs::write(&file, bytes).unwrap();
    (p, file)
}

#[test]
fn sources_are_read_as_text() {
    let read = |bytes: &[u8]| {
        let (_p, file) = temp("a.wisp", bytes);
        read_source(&file)
    };
    assert_eq!(read(b"a\r\nb\r\n").unwrap(), "a\nb\n");
    assert_eq!(read(b"a\nb").unwrap(), "a\nb");
    assert_eq!(read("\u{feff}x".as_bytes()).unwrap(), "x");
    // Only a `\r\n` is a line break; a `\r` alone is text.
    assert_eq!(read(b"a\rb\r\nc").unwrap(), "a\rb\nc");
    assert_eq!(read(b"").unwrap(), "");
    assert_eq!(
        read(&[0xff, 0xfe, 0x00]).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    let (p, _) = temp("b", b"");
    let e = read_source(&p.root().join("missing")).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn a_wisp_file_splits_into_rust_and_markup() {
    // No block: all markup.
    let (t, rust) = parse_wisp("<p>{a}</p>").unwrap();
    assert!(rust.is_none());
    assert_eq!(t.chunks.concat(), "<p></p>");

    // The block's Rust keeps its line; the markup is blanked, and the other
    // way round, so every line is on its own line in the file.
    let src = "\n\n---\nlet a = 1;\nlet b = 2;\n---\n<p>{a}{b}</p>";
    let (t, rust) = parse_wisp(src).unwrap();
    assert_eq!(rust.as_deref(), Some("\n\n\nlet a = 1;\nlet b = 2;\n\n"));
    assert_eq!(t.chunks.concat(), "<p></p>");
    let Some(template::Node::Expr(a)) = t
        .nodes
        .iter()
        .find(|n| matches!(n, template::Node::Expr(_)))
    else {
        panic!("no expression in {:?}", t.nodes);
    };
    assert_eq!(
        (a.src.as_str(), a.line),
        ("a", 7),
        "markup lines count the block"
    );

    // A `---` that is not the first line is markup.
    let (t, rust) = parse_wisp("<hr>\n---\n<hr>").unwrap();
    assert!(rust.is_none());
    assert!(t.chunks.concat().contains("---"));
    // A lone block is a page with no markup.
    let (t, rust) = parse_wisp("---\nlet a = 1;\n---").unwrap();
    assert_eq!(rust.as_deref(), Some("\nlet a = 1;\n"));
    assert!(t.nodes.iter().all(|n| matches!(n, template::Node::Text(_))));
    // CRLF was read as LF by now; a block's fence may have spaces around it.
    assert!(
        parse_wisp("  ---  \nlet a = 1;\n\t---\t\n{a}")
            .unwrap()
            .1
            .is_some()
    );

    // Errors say `line:col: message`, in the file's lines.
    assert_eq!(
        parse_wisp("\n---\nlet a = 1;\n").unwrap_err(),
        "2:1: this `---` starts a block of Rust, which needs a `---` line after it"
    );
    let e = parse_wisp("---\nlet a = 1;\n---\n<p>\n{#if x}\n</p>").unwrap_err();
    assert!(e.starts_with("5:1: "), "{e}");
}

#[test]
fn the_shape_is_what_a_running_build_cannot_change() {
    let shape = |src: &str| parse_wisp(src).unwrap().0.shape;
    // Text is not part of it; structure and code are.
    assert_eq!(
        shape("<h1>Hello {name}</h1>"),
        shape("<h2 class='big'>Goodbye {name}!</h2>")
    );
    assert_ne!(shape("<p>{a}</p>"), shape("<p>{b}</p>"));
    assert_ne!(shape("<p>{a}</p>"), shape("<p>{a}</p><p>{a}</p>"));
    // The block's Rust is part of it.
    assert_ne!(
        shape("---\nlet a = 1;\n---\n{a}"),
        shape("---\nlet a = 2;\n---\n{a}")
    );
    assert_eq!(
        shape("---\nlet a = 1;\n---\n{a}"),
        shape("---\nlet a = 1;\n---\n{a}")
    );
    assert_ne!(shape("{a}"), shape("---\n---\n{a}"));
}

#[test]
fn hot_reload_takes_new_chunks_of_the_same_shape() {
    let p = Project::new(&[
        ("src/routes/+page.wisp", "<h1>Hello {name}</h1>"),
        ("src/components/Card.wisp", "{@props t: &str}\n<b>{t}</b>"),
        (
            "src/app.html",
            "<html>%wisp.head%<body>%wisp.body%</body></html>",
        ),
        ("src/routes/bad/+page.wisp", "\n<p>{#if x}</p>"),
    ]);
    let (chunks, shape) = hot_chunks(p.root(), "src/routes/+page.wisp").unwrap();
    assert_eq!(chunks.concat(), "<h1>Hello </h1>");
    fs::write(
        p.root().join("src/routes/+page.wisp"),
        "<h2>Bye {name}</h2>\r\n",
    )
    .unwrap();
    let (chunks2, shape2) = hot_chunks(p.root(), "src/routes/+page.wisp").unwrap();
    assert_eq!(shape, shape2);
    assert_eq!(chunks2.concat(), "<h2>Bye </h2>");

    let (c, _) = hot_chunks(p.root(), "src/components/Card.wisp").unwrap();
    assert_eq!(c.concat(), "<b></b>");

    // The shell: text, head, text, body, text. Its shape is fixed.
    let (parts, shape) = hot_chunks(p.root(), "src/app.html").unwrap();
    assert_eq!(parts, ["<html>", "<body>", "</body></html>"]);
    fs::write(p.root().join("src/app.html"), "<x>%wisp.head%%wisp.body%").unwrap();
    let (parts, again) = hot_chunks(p.root(), "src/app.html").unwrap();
    assert_eq!((parts.len(), shape), (3, again));

    // Errors name the file, and a template's carry `line:col`.
    let e = hot_chunks(p.root(), "src/routes/bad/+page.wisp").unwrap_err();
    assert!(e.starts_with("src/routes/bad/+page.wisp:2:4: "), "{e}");
    fs::write(p.root().join("src/app.html"), "no markers").unwrap();
    assert_eq!(
        hot_chunks(p.root(), "src/app.html").unwrap_err(),
        "src/app.html: missing %wisp.head%"
    );
    fs::write(p.root().join("src/app.html"), "%wisp.body%%wisp.head%").unwrap();
    assert_eq!(
        hot_chunks(p.root(), "src/app.html").unwrap_err(),
        "src/app.html: %wisp.head% must come before %wisp.body%"
    );
    fs::write(
        p.root().join("src/app.html"),
        "%wisp.head%%wisp.body%%wisp.body%",
    )
    .unwrap();
    assert!(
        hot_chunks(p.root(), "src/app.html")
            .unwrap_err()
            .contains("appears more than once")
    );
    let e = hot_chunks(p.root(), "src/routes/missing.wisp").unwrap_err();
    assert!(e.starts_with("src/routes/missing.wisp: "), "{e}");
    let e = hot_chunks(p.root(), "src/app.html.gone").unwrap_err();
    assert!(e.starts_with("src/app.html.gone: "), "{e}");
}

#[test]
fn hashes_are_fnv() {
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_ne!(fnv1a(b"ab"), fnv1a(b"ba"));
}

#[test]
fn javascript_is_minified() {
    // A module that `export default`s is not renamed, so what these check is
    // the whitespace alone.
    let min = |s: &str| {
        let out = minify_js(&format!("export default 0;{s}"));
        out.strip_prefix("export default 0;").unwrap().to_string()
    };
    assert_eq!(min(""), "");
    assert_eq!(min("// nothing\n/* at all */"), "");
    assert_eq!(min("let a = 1 // one\nlet b = 2"), "let a=1\nlet b=2");
    assert_eq!(
        min("const f = (a, b) => {\n  return a + b\n}"),
        "const f=(a,b)=>{return a+b}"
    );
    // A break stays only where dropping it would join two statements.
    assert_eq!(min("a\nb"), "a\nb");
    assert_eq!(min("a()\n++b"), "a()\n++b");
    assert_eq!(min("a(\n1,\n2\n)"), "a(1,2)");
    // Strings, templates and regexes are kept as written.
    assert_eq!(min("x = 'a  b' + \"c  d\""), "x='a  b'+\"c  d\"");
    assert_eq!(min("x = `a  ${ b }  c`"), "x=`a  ${b}  c`");
    assert_eq!(min("x = /a  b/g.test(y)"), "x=/a  b/g.test(y)");
    // Two marks that would read as one stay apart.
    assert_eq!(min("a + +b"), "a+ +b");
    assert_eq!(min("a - -b"), "a- -b");
    assert_eq!(min("a = b / /c/.exec(d)"), "a=b/ /c/.exec(d)");
    // Words stay apart, and a number goes on through dots.
    assert_eq!(min("return   x"), "return x");
    assert_eq!(min("1 .toString()"), "1 .toString()");
    assert_eq!(min("x in y"), "x in y");
    assert_eq!(min("a = 1e3 + 2"), "a=1e3+2");
    assert_eq!(
        min("class A { #p = 1; get() { return this.#p } }"),
        "class A{#p=1;get(){return this.#p}}"
    );
    assert_eq!(min("a . b"), "a.b");
    // Never longer than what it minifies, and minifying twice changes nothing.
    let src = "import { a } from './a.js'\n\nexport async function load({ data }) {\n  // comment\n  const r = await fetch(`/x/${data.id}`)\n  return { ok: r.ok, n: [1, 2, 3].map((n) => n * 2) }\n}\n";
    let once = min(src);
    assert!(once.len() < src.len());
    assert_eq!(min(&once), once);
}

/// The runtime as release builds serve it (minified by `wisp`'s build.rs
/// and codegen) still parses. Run by Node when there is one.
#[test]
fn the_minified_runtime_parses() {
    let dir = std::env::temp_dir();
    for (file, js) in [
        ("live.mjs", wisp_shared::LIVE_JS),
        ("wisp.js", wisp_shared::WISP_JS),
        ("extra.mjs", wisp_shared::EXTRA_JS),
    ] {
        let path = dir.join(format!("wisp-check-{}-{file}", std::process::id()));
        fs::write(&path, minify_js(js)).unwrap();
        let checked = std::process::Command::new("node")
            .arg("--check")
            .arg(&path)
            .output();
        let _ = fs::remove_file(&path);
        let Ok(checked) = checked else {
            return; // no Node here
        };
        let why = String::from_utf8_lossy(&checked.stderr);
        assert!(checked.status.success(), "{file}: {why}");
    }
}

/// What the browser runtime is served as: names it binds are shortened,
/// what it does not (exports, globals, properties) is not.
#[test]
fn the_runtime_is_served_with_short_names() {
    let src = "let counter = 0
export function bump(step) {
  counter += step
  return document.title + counter
}
";
    let out = minify_js(src);
    assert!(!out.contains("counter") && !out.contains("step"), "{out}");
    assert!(
        out.contains("export function bump(") && out.contains("document.title"),
        "{out}"
    );
    assert!(out.len() < src.len(), "{out}");
    assert_eq!(
        minify_js(&out),
        out,
        "minifying what is minified changes nothing"
    );
}

#[test]
fn the_route_scan_orders_routes_by_specificity() {
    let files: Vec<(&str, &str)> = vec![
        ("src/routes/+page.wisp", "x"),
        ("src/routes/[...rest]/+page.wisp", "x"),
        ("src/routes/[slug]/+page.wisp", "x"),
        ("src/routes/[n=int]/+page.wisp", "x"),
        ("src/routes/blog/[[page]]/+page.wisp", "x"),
        ("src/routes/(g)/+layout.wisp", "{@render children()}"),
        ("src/routes/(g)/about/+page.wisp", "x"),
        ("src/routes/+layout.wisp", "{@render children()}"),
        ("src/routes/+error.wisp", "x"),
        ("src/routes/about/team/+error.wisp", "x"),
        ("src/routes/about/team/+page.wisp", "x"),
        (
            "src/routes/api/+server.rs",
            "fn get() {}\nfn get_one(id: u64) {}",
        ),
    ];
    let p = Project::new(&files);
    let tree = routes::scan(&p.root().join("src/routes")).unwrap();
    let patterns: Vec<String> = tree.routes.iter().map(routes::Route::pattern).collect();
    assert_eq!(
        patterns,
        [
            "/",
            "/about",
            "/about/team",
            "/api",
            "/blog/[[page]]",
            "/[n=int]",
            "/[slug]",
            "/[...rest]",
        ]
    );
    // Matching order: the optional segment's two forms are both arms, the
    // static ones first, the rest last.
    let arms: Vec<String> = tree
        .arms()
        .iter()
        .map(|(segs, id)| format!("{}:{}", segs.len(), tree.routes[*id].pattern()))
        .collect();
    assert_eq!(arms.first().map(String::as_str), Some("0:/"));
    assert!(
        arms.contains(&"1:/blog/[[page]]".to_string())
            || arms.contains(&"2:/blog/[[page]]".to_string())
    );
    assert_eq!(arms.last().map(String::as_str), Some("1:/[...rest]"));
    // Layouts, outermost first, and the nearest error page.
    let about = &tree.routes[1];
    assert_eq!((about.layouts.len(), about.error), (2, Some(0)));
    let team = &tree.routes[2];
    assert_eq!(team.error, Some(1));
    assert_eq!(tree.layouts.len(), 2);
    assert_eq!(tree.errors.len(), 2);
    assert!(
        tree.matchers
            .iter()
            .any(|(m, file)| m == "int" && file.is_none())
    );
    let blog = &tree.routes[4];
    assert_eq!(blog.params(), ["page"]);
    assert_eq!(blog.expansions().len(), 2);
    assert_eq!(
        blog.segs,
        [
            Seg::Static("blog".into()),
            Seg::Optional("page".into(), None)
        ]
    );
}

#[test]
fn editor_files_are_not_part_of_the_app() {
    for name in [
        ".x.wisp.swp",
        "#x#",
        "x~",
        "x.tmp",
        "x.bak",
        "x.orig",
        "x.swx",
        "x___jb_tmp___",
        "4913",
        ".DS_Store",
    ] {
        assert!(routes::editor_temp(name), "{name}");
    }
    for name in ["+page.wisp", "Card.wisp", "notes.rs", "a.b", "4914"] {
        assert!(!routes::editor_temp(name), "{name}");
    }
}

#[test]
fn a_server_file_says_what_it_serves() {
    let items = |src: &str| rust_scan::scan(src).unwrap();
    let get_id = items("fn get(id: u64) {}");
    let f = get_id.function("get").unwrap();
    let plain = [Seg::Static("n".into())];
    let routed = [Seg::Param("id".into(), None)];
    assert!(routes::is_member(f, &plain));
    assert!(!routes::is_member(f, &routed));
    assert!(!routes::is_member(
        items("fn get() {}").function("get").unwrap(),
        &plain
    ));
    assert_eq!(
        routes::rest_type(&items("#[derive(Json, Rest)]\nstruct Note { a: u8 }")),
        Some("Note")
    );
    assert_eq!(
        routes::rest_type(&items("#[derive(Json)]\nstruct Note { a: u8 }")),
        None
    );
    assert_eq!(
        routes::HANDLERS,
        ["get", "post", "put", "patch", "delete", "list"]
    );
    assert_eq!(routes::MAX_PARAMS, 8);
}

#[test]
fn the_rust_scan_reads_signatures() {
    let src = "\
//! A page.
#![allow(dead_code)]

use wisp::prelude::*;

#[derive(Json, Debug)]
pub struct Data {
    #[validate(len = 1..=9)]
    title: String,
    n: u32,
}

const BODY_LIMIT: usize = 4 * wisp::MB;
static mut COUNT: u8 = 0;

pub async fn load(cx: &mut Cx, slug: &str, page: Option<u32>) -> Result<Data> { todo!() }

#[action]
fn add(#[validate(len = 1..=10)] text: String, mut tags: Vec<String>) -> Result { Ok(()) }

#[wisp::action]
fn note() { cx.flash(\"x\"); }

fn helper() -> Option<Response> { None }
";
    let items = rust_scan::scan(src).unwrap();
    assert!(items.check().is_ok());
    let names: Vec<&str> = items.fns.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["load", "add", "note", "helper"]);

    let load = items.function("load").unwrap();
    assert!(load.public && load.is_async && load.fallible && !load.action);
    assert_eq!(load.line, 16);
    assert_eq!(load.returns, "Result<Data>");
    assert_eq!(load.value_type(), "Data");
    assert_eq!(load.returns_kind(), Returns::Other);
    assert_eq!(
        load.inputs().unwrap(),
        [("slug", "&str"), ("page", "Option<u32>")],
        "`cx` is not read from the request"
    );

    let add = items.function("add").unwrap();
    assert!(add.action && !add.public && !add.implicit_cx);
    assert_eq!(add.returns_kind(), Returns::Nothing);
    assert_eq!(
        add.checks,
        [("text".to_string(), "len = 1..=10".to_string())]
    );
    assert_eq!(
        add.inputs().unwrap(),
        [("text", "String"), ("tags", "Vec<String>")]
    );

    // `cx` used but not taken: `#[action]` adds it.
    let note = items.function("note").unwrap();
    assert!(note.action && note.implicit_cx);
    assert_eq!(
        items.function("helper").unwrap().returns_kind(),
        Returns::MaybeResponse
    );
    assert!(items.function("nope").is_none());

    let data = &items.types[0];
    assert_eq!(data.name, "Data");
    assert_eq!(data.derives, ["Json", "Debug"]);
    assert_eq!(
        data.fields,
        [
            ("title".to_string(), "String".to_string()),
            ("n".to_string(), "u32".to_string())
        ]
    );
    assert_eq!(
        data.rules,
        [("title".to_string(), "len = 1..=9".to_string())]
    );
    assert_eq!(items.data_fields().len(), 2);
    let limit = items.constant("BODY_LIMIT").unwrap();
    assert_eq!((limit.ty.as_str(), limit.line), ("usize", 13));
    assert_eq!(items.constant("COUNT").unwrap().ty, "u8");
    assert!(items.constant("NOPE").is_none());
}

#[test]
fn what_a_function_returns_is_classified() {
    let kind = |sig: &str| {
        let items = rust_scan::scan(&format!("{sig} {{}}")).unwrap();
        items.fns[0].returns_kind()
    };
    assert_eq!(kind("fn a()"), Returns::Nothing);
    assert_eq!(kind("fn a() -> ()"), Returns::Nothing);
    assert_eq!(kind("fn a() -> Result"), Returns::Nothing);
    assert_eq!(kind("fn a() -> Result<()>"), Returns::Nothing);
    assert_eq!(kind("fn a() -> Response"), Returns::Response);
    assert_eq!(kind("fn a() -> wisp::Response"), Returns::Response);
    assert_eq!(kind("fn a() -> Result<Response>"), Returns::Response);
    assert_eq!(kind("fn a() -> Option<Response>"), Returns::MaybeResponse);
    assert_eq!(
        kind("fn a() -> Result<Option<Response>, Error>"),
        Returns::MaybeResponse
    );
    assert_eq!(kind("fn a() -> Option<u8>"), Returns::Other);
    assert_eq!(kind("fn a() -> Vec<u8>"), Returns::Other);
    let opt = rust_scan::scan("fn a() -> Option<()> {}").unwrap();
    assert_eq!(opt.fns[0].optional_value(), Some("()"));
    let opt = rust_scan::scan("fn a() -> Result<Option<Note>> {}").unwrap();
    assert_eq!(opt.fns[0].optional_value(), Some("Note"));
    let none = rust_scan::scan("fn a() -> Option<Response> {}").unwrap();
    assert_eq!(none.fns[0].optional_value(), None);

    assert!(rust_scan::is_cx("&mut Cx"));
    assert!(rust_scan::is_cx("&Cx"));
    assert!(rust_scan::is_cx("&'a mut wisp::Cx"));
    assert!(rust_scan::is_cx("&mut ::wisp::Cx"));
    assert!(!rust_scan::is_cx("Cx<'a>"));
    assert!(!rust_scan::is_cx("&mut Context"));
    assert_eq!(rust_scan::last_segment("wisp::Response"), "Response");
    assert_eq!(rust_scan::last_segment("Option<T>"), "Option");
    assert_eq!(rust_scan::last_segment(" a::b::C "), "C");
}

#[test]
fn the_rust_scan_skips_what_is_not_a_top_level_item() {
    let src = "\
// fn commented() {}
/* fn blocked() {} */
const S: &str = \"fn quoted() {}\";
const R: &str = r#\"fn raw() {}\"#;
const C: char = '{';
impl A { fn method() {} }
mod m { fn inner() {} }
fn outer() { fn nested() {} }
type Alias = u8;
enum E { A }
union U { a: u8 }
fn(u8) -> u8;
fn real() {}
";
    let items = rust_scan::scan(src).unwrap();
    let names: Vec<&str> = items.fns.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["outer", "real"]);
    let types: Vec<&str> = items.types.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(types, ["Alias", "E", "U"]);
    // Half-typed attributes leave the rest to rustc.
    assert!(
        rust_scan::scan("#[derive(\nfn a() {}")
            .unwrap()
            .fns
            .is_empty()
    );
    // `//!` after an item does not belong there.
    assert!(
        rust_scan::scan("fn a() {}\n//! docs")
            .unwrap()
            .check_inner()
            .is_err()
    );
    assert!(
        rust_scan::scan("//! docs\n\n#![allow(x)]\nfn a() {}")
            .unwrap()
            .check_inner()
            .is_ok()
    );
    assert!(
        rust_scan::scan("fn a() {}\n/*! docs */")
            .unwrap()
            .check_inner()
            .is_err()
    );
    assert_eq!(
        rust_scan::inner_end("//! a\n#![x]\nfn f() {}"),
        "//! a\n#![x]".len()
    );
    assert_eq!(rust_scan::inner_end("fn f() {}"), 0);
    assert_eq!(
        rust_scan::inner_end("// plain\n//! doc\n/*! block */ fn f() {}"),
        "// plain\n//! doc\n/*! block */".len()
    );
    assert_eq!(rust_scan::inner_end("#![unclosed"), 0);
}

#[test]
fn a_block_splits_into_items_and_statements() {
    let (items, stmts) = rust_scan::split_items(
        "let a = 1;\n/// Doc.\n#[derive(Debug)]\nstruct S;\nfn f() {\n    let x = 2;\n}\nlet b = f();\nuse std::fmt;\n",
    );
    assert_eq!(
        items.len(),
        stmts.len(),
        "each keeps the whole block's length"
    );
    assert!(
        items.contains("struct S;") && items.contains("fn f()") && items.contains("use std::fmt;")
    );
    assert!(items.contains("/// Doc.") && items.contains("#[derive(Debug)]"));
    assert!(!items.contains("let a") && !items.contains("let b"));
    assert!(stmts.contains("let a = 1;") && stmts.contains("let b = f();"));
    assert!(!stmts.contains("struct") && !stmts.contains("let x"));
    assert_eq!(items.matches('\n').count(), stmts.matches('\n').count());

    assert_eq!(
        rust_scan::let_names(
            "let (a, mut b) = x;\nif c { let d = 1; }\nlet Some(e) = f else { return };"
        ),
        ["a", "b", "e"]
    );
    assert_eq!(
        rust_scan::let_names("let g: Vec<u8> = h;\nlet _ = 1;"),
        ["g"]
    );
    assert!(rust_scan::let_names("").is_empty());

    // A line ends in code unless a string or comment runs past it.
    assert_eq!(rust_scan::line_ends_in_code("a\nb"), [true, true]);
    assert_eq!(
        rust_scan::line_ends_in_code("let s = \"a\nb\";\nc"),
        [false, true, true]
    );
    assert_eq!(
        rust_scan::line_ends_in_code("/* a\nb */ x\ny"),
        [false, true, true]
    );
    assert_eq!(rust_scan::line_ends_in_code("// a\nb"), [true, true]);
}

#[test]
fn templates_parse_from_the_public_module() {
    let t = template::parse("<p>{a}</p>{#if b}<i>{c}</i>{/if}").unwrap();
    assert_eq!(t.chunks.concat(), "<p></p><i></i>");
    assert!(!t.uses_children && t.props.is_none() && t.script.is_none());
    let t = template::parse("{@props a: u8, b: &str = \"x\"}\n{@render children()}").unwrap();
    assert!(t.uses_children);
    let props = t.props.unwrap().0;
    assert_eq!(
        props.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(props[1].default.as_deref(), Some("\"x\""));
    let e = template::parse("\n  {/if}").unwrap_err();
    assert_eq!((e.line, e.col), (2, 3));
    assert_eq!(e.to_string(), format!("2:3: {}", e.msg));
    for (name, ok) in [
        ("Card", true),
        ("Ui", true),
        ("A1_b", true),
        ("card", false),
        ("UI", false),
        ("A-b", false),
        ("", false),
        ("1A", false),
    ] {
        assert_eq!(template::is_component_name(name), ok, "{name}");
    }
    let client = template::parse("<p :hidden=\"a\">x</p><script>let a</script>").unwrap();
    assert!(client.is_live() && client.script.is_some());
    assert!(!template::parse("<p>x</p>").unwrap().is_live());
}

#[test]
fn a_package_import_needs_the_package() {
    let page = (
        "src/routes/+page.wisp",
        "<button on:click=\"go()\">x</button>\n<script>\n  import confetti from 'canvas-confetti'\n  function go() { confetti() }\n</script>",
    );
    let lib = (
        "src/lib/fx.js",
        "export { default } from 'canvas-confetti/x'\n",
    );
    let p = Project::new(&[page, lib]);
    let e = wisp_build::check(p.root()).unwrap_err();
    assert!(
        e.contains("src/lib/fx.js") && e.contains("`wisp add canvas-confetti`"),
        "{e}"
    );
    let json = (
        "package.json",
        "{\"dependencies\": {\"canvas-confetti\": \"1.9.3\"}}",
    );
    let p = Project::new(&[page, lib, json]);
    assert_eq!(
        wisp_build::check(p.root()).unwrap(),
        [
            "/canvas-confetti@1.9.3/x?target=es2022",
            "/canvas-confetti@1.9.3?target=es2022"
        ]
    );
}
