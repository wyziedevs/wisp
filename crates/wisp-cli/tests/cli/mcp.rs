//! `wisp mcp`: a client's whole exchange over stdin and stdout.

use crate::{Dir, command, new_app, read};
use std::io::Write;
use std::process::Stdio;

#[test]
fn an_agent_reads_docs_routes_components_and_adds_a_route() {
    let cwd = Dir::new("mcp");
    let app = new_app(&cwd, "app", &["--template", "minimal"]);
    let call = |id: u32, name: &str, args: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
        )
    };
    let lines = [
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.into(),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.into(),
        call(2, "wisp_docs", r#"{"topic":"npm packages"}"#),
        call(3, "wisp_new_route", r#"{"path":"/blog/[slug]"}"#),
        call(4, "wisp_routes", "{}"),
        call(5, "wisp_components", "{}"),
        call(6, "wisp_check", "{}"),
    ];
    let mut child = command(&app)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all((lines.join("\n") + "\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    let replies: Vec<&str> = out.lines().collect();
    // One reply a request; none to the notification.
    assert_eq!(replies.len(), 7, "{out}");
    for (i, reply) in replies.iter().enumerate() {
        assert!(
            reply.starts_with(&format!(r#"{{"jsonrpc":"2.0","id":{i},"result":"#)),
            "{reply}"
        );
        assert!(!reply.contains(r#""isError":true"#), "{reply}");
    }
    assert!(replies[0].contains(r#""protocolVersion":"2025-06-18""#));
    assert!(replies[1].contains("wisp_new_route"));
    assert!(replies[2].contains("wisp add"));
    assert!(replies[3].contains("Wrote src/routes/blog/[slug]/+page.wisp"));
    assert!(replies[4].contains(r#"\"pattern\":\"/blog/[slug]\""#));
    assert!(replies[4].contains(r#"\"params\":[\"slug\"]"#));
    assert_eq!(
        replies[5],
        r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"[]"}],"isError":false}}"#
    );
    assert!(replies[6].contains(r#"{\"ok\":true}"#));
    assert!(read(&app, "src/routes/blog/[slug]/+page.wisp").contains("<p>{slug}</p>"));
}
