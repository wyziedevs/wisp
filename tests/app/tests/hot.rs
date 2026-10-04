//! `wisp dev`'s hot swaps in a headless Chrome or Edge (`cargo test -p
//! wisp-test-app --features browser`; skipped where there is none): what
//! the CLI sends the app, sent here, and what wisp-dev.js then does, done
//! here. A page's count survives a new script and new text.
#![cfg(not(target_arch = "wasm32"))]
#![cfg(all(feature = "browser", debug_assertions))]

use wisp::App;
use wisp::Value;
use wisp_test_app::Site;

const REL: &str = "src/routes/hot/+page.wisp";

#[test]
fn a_swap_keeps_state() {
    let mut b = wisp::browser!(Site);
    b.goto("/hot");
    for _ in 0..3 {
        b.click("text=Plus one");
    }
    assert_eq!(b.text("output"), "3");
    b.fill("input", "kept");

    // New text, the shape the app has: its part of the page morphs in.
    let src = std::fs::read_to_string(format!("{}/{REL}", Site::ROOT)).unwrap();
    let root = std::env::temp_dir().join(format!("wisp-hot-{}", std::process::id()));
    let file = root.join(REL);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, src.replace("Hot page", "Hotter page")).unwrap();
    let (chunks, new, _) = wisp_build::hot_chunks(&root, REL).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let shape = Site::TEMPLATES.iter().find(|t| t.0 == REL).unwrap().1;
    assert_eq!(new, shape, "text alone");
    let mut body = format!("{REL}\n{shape:016x}\n{}\n", chunks.len());
    for c in &chunks {
        body.push_str(&format!("{}\n{c}", c.len()));
    }
    let post = |path: &str, body: &str| {
        format!(
            "fetch({path:?}, {{ method: 'POST', body: {body:?} }}).then((r) => r.status)",
            body = body
        )
    };
    assert_eq!(
        b.eval(&post("/_wisp/dev/swap", &body)).as_f64(),
        Some(200.0)
    );
    b.eval(&format!(
        "new Promise((done) => document.dispatchEvent(new CustomEvent('wisp:region', {{ detail: {{ files: [{REL:?}], done }} }})))"
    ));
    assert_eq!(b.text("h1"), "Hotter page");
    assert_eq!(b.text("output"), "3");

    // A new script: its module swapped in place, its state kept.
    let url =
        b.eval("Object.values(JSON.parse(document.getElementById('wisp-live').textContent).m)[0]");
    let url = url.as_str().unwrap().to_string();
    let Value::String(source) = b.eval(&format!("fetch({url:?}).then((r) => r.text())")) else {
        panic!("no module at {url}");
    };
    assert!(source.contains("count.v++"), "{source}");
    let version = source
        .split("live.js?v=")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let path = url.split('?').next().unwrap();
    let hot = format!("{path}?v=hot1");
    let body = format!(
        "{path}\n{hot}\n{version}\n{}",
        source.replace("count.v++", "count.v += 10")
    );
    assert_eq!(
        b.eval(&post("/_wisp/dev/module", &body)).as_f64(),
        Some(200.0)
    );
    b.eval(&format!("import({hot:?}).then(() => 0)"));
    assert_eq!(b.text("output"), "3", "the count is kept");
    assert_eq!(
        b.eval("document.querySelector('input').value").as_str(),
        Some("kept")
    );
    b.click("text=Plus one");
    assert_eq!(b.text("output"), "13", "the new script runs");
    assert_eq!(b.text("h1"), "Hotter page");

    // A reload names the new module.
    b.goto("/hot");
    b.click("text=Plus one");
    assert_eq!(b.text("output"), "10");
}
