//! The live-page protocol end to end (`wisp/src/protocol.rs`): the page the
//! app renders for `/golden` against its fixture, `golden.html`, and then
//! wisp.js and live.js, as the app serves them, run under Node against
//! that fixture (`golden.mjs`): the page hydrates, and its instance's
//! record reaches the elements. Skipped quietly with no Node. A change to
//! what pages carry is a change to the fixture: `WISP_BLESS=1` writes it.

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn golden_live_page() {
    let mut app = client::<Site>();
    let page = app.get("/golden").text().to_string();
    let url = |from: &str| {
        let at = page.find(from).unwrap();
        page[at..at + page[at..].find('"').unwrap()].to_string()
    };
    let (module_url, live_url) = (url("/_app/c/"), url("/_app/live.js"));
    let id = &module_url["/_app/c/".len()..module_url.find('.').unwrap()];

    // Versions and the module's number change with any other edit: the
    // fixture has `V` and `t0`.
    let mut fixture = String::new();
    let mut rest = page.replace(&format!("\"{id}\""), "\"t0\"");
    rest = rest.replace(&format!("/{id}.js"), "/t0.js");
    while let Some(at) = rest.find("?v=") {
        fixture.push_str(&rest[..at + 3]);
        fixture.push('V');
        let end = rest[at..].find('"').unwrap() + at;
        rest = rest[end..].to_string();
    }
    fixture.push_str(&rest);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden.html");
    if std::env::var_os("WISP_BLESS").is_some() {
        std::fs::write(path, &fixture).unwrap();
    }
    let want = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    assert_eq!(fixture, want, "the page differs from tests/golden.html");

    let module = app.get(&module_url).text().to_string();
    let dir = std::env::temp_dir().join(format!("wisp-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (file, body) in [
        ("golden.html", fixture.clone()),
        ("wisp.js", app.get("/_app/wisp.js").text().to_string()),
        ("live.mjs", app.get(&live_url).text().to_string()),
        ("module.js", module.replace(&format!("\"{id}\""), "\"t0\"")),
    ] {
        std::fs::write(dir.join(file), body).unwrap();
    }
    let run = std::process::Command::new("node")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden.mjs"))
        .arg(&dir)
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let Ok(run) = run else {
        return; // no Node here
    };
    let out = String::from_utf8_lossy(&run.stdout);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success() && out.trim() == "ok", "{out}{err}");
}
