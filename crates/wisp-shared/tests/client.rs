//! wisp.js run by Node against a stand-in for the browser (client.js): the
//! offline form queue, focus after a client navigation, the navigation hooks
//! and link attributes. Skipped quietly where there is no Node.

#[test]
fn wisp_js_in_node() {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/client.js");
    let Ok(run) = std::process::Command::new("node").arg(file).output() else {
        return; // no Node here
    };
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

/// live.js (signals, blocks, bindings) against a small DOM of its own.
#[test]
fn live_js_in_node() {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/live.js");
    let Ok(run) = std::process::Command::new("node").arg(file).output() else {
        return; // no Node here
    };
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
}
