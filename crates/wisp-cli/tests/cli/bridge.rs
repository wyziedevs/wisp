//! `bridge.js`, the JavaScript half of the edge targets, run by Node with a
//! fake wasm instance (`bridge.mjs`). Skipped when there is no `node`.

use super::Dir;
use std::fs;
use std::process::Command;

#[test]
fn bridge_answers_every_status_a_response_can_carry() {
    if Command::new("node").arg("--version").output().is_err() {
        crate::skip("bridge_answers_every_status_a_response_can_carry: no node");
        return;
    }
    let dir = Dir::new("bridge");
    let here = env!("CARGO_MANIFEST_DIR");
    fs::copy(
        format!("{here}/src/targets/bridge.js"),
        dir.join("bridge.mjs"),
    )
    .unwrap();
    let script = fs::read_to_string(format!("{here}/tests/cli/bridge.mjs"))
        .unwrap()
        .replace("../../src/targets/bridge.js", "./bridge.mjs");
    fs::write(dir.join("run.mjs"), script).unwrap();
    // The Workers entry, its wasm a stand-in the fake instance ignores.
    let worker = fs::read_to_string(format!("{here}/src/targets/worker.js"))
        .unwrap()
        .replace("import module from './app.wasm';", "const module = {};");
    fs::write(dir.join("worker.mjs"), worker).unwrap();
    let out = Command::new("node")
        .arg(dir.join("run.mjs"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
