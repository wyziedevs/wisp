//! Each Rust and HTML snippet of AGENTS.md is in this app as written, so
//! building the workspace compiles every one: the reference AI agents read
//! cannot show code that does not build.

use std::fs;
use std::path::Path;

fn files(dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(fs::read_to_string(&path).unwrap().replace("\r\n", "\n"));
        }
    }
}

#[test]
fn every_snippet_is_compiled_here() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let agents = fs::read_to_string(root.join("../../llms/AGENTS.md"))
        .unwrap()
        .replace("\r\n", "\n");
    let mut app = Vec::new();
    files(&root.join("src"), &mut app);
    let mut n = 0;
    for block in agents.split("\n```").skip(1).step_by(2) {
        let (lang, code) = block.split_once('\n').unwrap();
        if lang == "rust" || lang == "html" {
            n += 1;
            assert!(
                app.iter().any(|f| f.contains(code.trim_end())),
                "This AGENTS.md snippet is in no file of tests/agents/src:\n{code}"
            );
        }
    }
    assert_eq!(n, 9, "a snippet was added: put it in tests/agents/src");
}
