//! `wisp mcp`: a Model Context Protocol server on stdin and stdout, so an AI
//! agent can read Wisp's docs, see the app's routes, components and
//! problems, and add a route. JSON-RPC 2.0, one message a line; the app is
//! the current directory, read again on every call.

use std::io::{self, BufRead, Read, Write};
use std::path::Path;
use wisp_shared::json::{self, Json};

/// AGENTS.md and the docs site pages, as one file (the repository's llms-full.txt).
const DOCS: &str = include_str!("../templates/vendor/llms-full.txt");

/// What the protocol's `tools/list` answers: each tool and its input,
/// on one line.
const TOOLS: &str = concat!(
    "[",
    r#"{"name":"wisp_docs","description":"Wisp's reference: the AGENTS.md or docs sections about a topic (like 'actions', 'Rest filters', 'npm', 'islands'). No topic lists the sections.","inputSchema":{"type":"object","properties":{"topic":{"type":"string"}}}},"#,
    r#"{"name":"wisp_check","description":"Checks the app's routes and templates without compiling, as wisp check does: {ok} or {ok:false, errors:[{file,line,col,message}]}.","inputSchema":{"type":"object","properties":{}}},"#,
    r#"{"name":"wisp_routes","description":"The app's routes: pattern, folder, params, page, its #[action]s, and the endpoints its +server.rs answers.","inputSchema":{"type":"object","properties":{}}},"#,
    r#"{"name":"wisp_components","description":"The app's components (src/components): name, file and props with types and defaults.","inputSchema":{"type":"object","properties":{}}},"#,
    r#"{"name":"wisp_new_route","description":"Writes a starting file for a route: path like /blog/[slug], kind page (+page.wisp, the default), layout, error or server (+server.rs). Never overwrites.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"kind":{"type":"string","enum":["page","layout","error","server"]}},"required":["path"]}}"#,
    "]"
);

pub fn run() -> Result<(), String> {
    serve(&mut io::stdin().lock(), &mut io::stdout().lock())
}

/// Longest message read; a longer line is answered with an error and skipped.
const MAX_LINE: u64 = 1 << 24;

/// Answers each line of `input` on `out` until the end.
fn serve(input: &mut impl BufRead, out: &mut impl Write) -> Result<(), String> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = input
            .by_ref()
            .take(MAX_LINE)
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("Could not read stdin: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        let reply = if buf.last() != Some(&b'\n') && n as u64 == MAX_LINE {
            // Skip the rest of the oversized line without keeping it.
            loop {
                let chunk = input.fill_buf().map_err(|e| e.to_string())?;
                if chunk.is_empty() {
                    break;
                }
                let (used, end) = match chunk.iter().position(|&b| b == b'\n') {
                    Some(i) => (i + 1, true),
                    None => (chunk.len(), false),
                };
                input.consume(used);
                if end {
                    break;
                }
            }
            Some(error(&Json::Null, -32600, "Message too long."))
        } else {
            let line = String::from_utf8_lossy(&buf);
            if line.trim().is_empty() {
                continue;
            }
            answer(&line)
        };
        if let Some(reply) = reply {
            writeln!(out, "{reply}")
                .and_then(|()| out.flush())
                .map_err(|e| format!("Could not write stdout: {e}"))?;
        }
    }
}

/// The reply to one message; none to a notification.
fn answer(line: &str) -> Option<String> {
    let msg = match json::parse(line) {
        Ok(m @ Json::Obj(_)) => m,
        Ok(_) => return Some(error(&Json::Null, -32600, "Expected one JSON-RPC object.")),
        Err(e) => return Some(error(&Json::Null, -32700, &format!("Not JSON: {e}"))),
    };
    let id = msg.get("id")?;
    let params = msg.get("params").unwrap_or(&Json::Null);
    let result = match msg.str("method").unwrap_or("") {
        "initialize" => {
            let version = params.str("protocolVersion").unwrap_or("2025-06-18");
            format!(
                r#"{{"protocolVersion":{},"capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"wisp","version":"{}"}},"instructions":"Wisp is a Rust web framework. Read AGENTS.md first; wisp_docs finds more."}}"#,
                Json::Str(version.into()),
                env!("CARGO_PKG_VERSION")
            )
        }
        "ping" => "{}".into(),
        "tools/list" => format!(r#"{{"tools":{TOOLS}}}"#),
        "tools/call" => {
            let name = params.str("name").unwrap_or("");
            let args = params.get("arguments").unwrap_or(&Json::Null);
            // A tool that panics answers with an error; the server goes on.
            let (text, failed) = std::panic::catch_unwind(|| call(name, args))
                .unwrap_or_else(|_| Err("The tool failed unexpectedly.".into()))
                .map_or_else(|e| (e, true), |t| (t, false));
            format!(
                r#"{{"content":[{{"type":"text","text":{}}}],"isError":{failed}}}"#,
                Json::Str(text)
            )
        }
        m => return Some(error(id, -32601, &format!("There is no method {m}."))),
    };
    Some(format!(
        r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
    ))
}

fn error(id: &Json, code: i32, message: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{}}}}}"#,
        Json::Str(message.into())
    )
}

fn call(name: &str, args: &Json) -> Result<String, String> {
    let root = Path::new(".");
    let app = || {
        if root.join("build.rs").exists() && root.join("src").is_dir() {
            Ok(root)
        } else {
            Err("There is no Wisp app here: start wisp mcp in the app's folder.".to_string())
        }
    };
    let s = |v: &str| Json::Str(v.into());
    let strs = |v: &[String]| Json::Arr(v.iter().map(|x| s(x)).collect());
    match name {
        "wisp_docs" => Ok(docs(args.str("topic").unwrap_or(""))),
        "wisp_check" => Ok(match wisp_build::check(app()?) {
            Ok((_, w)) if w.is_empty() => r#"{"ok":true}"#.into(),
            Ok((_, w)) => format!(r#"{{"ok":true,"warnings":{}}}"#, strs(&w)),
            Err(e) => {
                let (file, line, col, message) = located(&e);
                let num = |n: Option<u32>| n.map_or(Json::Null, |n| Json::Num(n.to_string()));
                let err = Json::Obj(vec![
                    ("file".into(), file.map_or(Json::Null, s)),
                    ("line".into(), num(line)),
                    ("col".into(), num(col)),
                    ("message".into(), s(message)),
                ]);
                format!(r#"{{"ok":false,"errors":[{err}]}}"#)
            }
        }),
        "wisp_routes" => {
            let routes = wisp_build::inspect::routes(app()?)?;
            let rows = routes.iter().map(|r| {
                Json::Obj(vec![
                    ("pattern".into(), s(&r.pattern)),
                    ("dir".into(), s(&r.dir)),
                    ("params".into(), strs(&r.params)),
                    ("page".into(), Json::Bool(r.page)),
                    ("actions".into(), strs(&r.actions)),
                    ("endpoints".into(), strs(&r.endpoints)),
                ])
            });
            Ok(Json::Arr(rows.collect()).to_string())
        }
        "wisp_components" => {
            let comps = wisp_build::inspect::components(app()?)?;
            let rows = comps.iter().map(|(name, file, props)| {
                let props = props.iter().map(|p| {
                    Json::Obj(vec![
                        ("name".into(), s(&p.name)),
                        ("type".into(), s(&p.ty)),
                        ("default".into(), p.default.as_deref().map_or(Json::Null, s)),
                    ])
                });
                Json::Obj(vec![
                    ("name".into(), s(name)),
                    ("file".into(), s(file)),
                    ("props".into(), Json::Arr(props.collect())),
                ])
            });
            Ok(Json::Arr(rows.collect()).to_string())
        }
        "wisp_new_route" => new_route(
            app()?,
            args.str("path")
                .ok_or("wisp_new_route needs a path, like /blog/[slug].")?,
            args.str("kind").unwrap_or("page"),
        ),
        _ => Err(format!("There is no tool {name}.")),
    }
}

/// `src/x.wisp:3:5: msg` as its file, line, column and message.
fn located(e: &str) -> (Option<&str>, Option<u32>, Option<u32>, &str) {
    let mut parts = e.splitn(4, ':');
    let (Some(file), Some(line), Some(col), Some(msg)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return (None, None, None, e);
    };
    match (line.trim().parse(), col.trim().parse()) {
        (Ok(l), Ok(c)) if file.contains('.') => (Some(file), Some(l), Some(c), msg.trim()),
        _ => (None, None, None, e),
    }
}

/// The sections of the docs about `topic`, best first, or with no topic
/// their titles.
fn docs(topic: &str) -> String {
    let sections = sections(DOCS);
    let words: Vec<String> = (topic.split(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|w| w.len() > 1)
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        let titles: Vec<&str> = sections.iter().map(|s| s.0.as_str()).collect();
        return format!(
            "Sections (ask wisp_docs for one by its words):\n{}",
            titles.join("\n")
        );
    }
    let mut scored: Vec<(usize, usize)> = (sections.iter().enumerate())
        .map(|(i, (title, body))| {
            // A word in the title counts most; AGENTS.md, the short form,
            // beats a doc as good.
            let agents = title.starts_with("AGENTS.md");
            let (title, body) = (title.to_lowercase(), body.to_lowercase());
            let score: usize = (words.iter())
                .map(|w| {
                    100 * title.matches(w.as_str()).count()
                        + body.matches(w.as_str()).count().min(10)
                })
                .sum();
            (score + if score > 0 && agents { 10 } else { 0 }, i)
        })
        .filter(|&(score, _)| score > 0)
        .collect();
    // Best first; of equals, the earlier (AGENTS.md before the docs).
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = String::new();
    for (_, i) in scored.into_iter().take(3) {
        let (title, body) = &sections[i];
        if !out.is_empty() && out.len() + body.len() > 8000 {
            break;
        }
        out.push_str(&format!("## {title}\n{body}\n"));
    }
    if out.is_empty() {
        out = format!("Nothing about {topic}. Call wisp_docs with no topic for the sections.");
    }
    out
}

/// Each `##`/`###` section of the docs, titled by its file and heading
/// (`client.md > Directives`): its title and its text.
fn sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let (mut file, mut parent, mut fence) = ("AGENTS.md".to_string(), String::new(), false);
    for line in text.lines() {
        if line.starts_with("```") {
            fence = !fence;
        }
        let heading = if fence {
            None
        } else if let Some(f) = line
            .strip_prefix("<!-- docs/")
            .and_then(|l| l.strip_suffix(" -->"))
        {
            file = f.to_string();
            parent.clear();
            continue;
        } else if let Some(h) = line.strip_prefix("## ") {
            parent = h.to_string();
            Some(format!("{file} > {h}"))
        } else {
            (line.strip_prefix("### ")).map(|h| format!("{file} > {parent} > {h}"))
        };
        match (heading, out.last_mut()) {
            (Some(h), _) => out.push((h, String::new())),
            (None, Some(last)) => {
                last.1.push_str(line);
                last.1.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}

/// Writes the starting file of a `kind` of route file at `path`.
fn new_route(root: &Path, path: &str, kind: &str) -> Result<String, String> {
    let mut dir = root.join("src").join("routes");
    let mut rel = String::from("src/routes");
    let mut params = Vec::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if seg == "." || seg == ".." || seg.contains(['\\', ':']) {
            return Err(format!("{seg} cannot be part of a route's path."));
        }
        if let Some(p) = wisp_build::routes::parse_segment(seg)?
            .as_ref()
            .and_then(|s| s.param())
        {
            params.push(p.to_string());
        }
        dir.push(seg);
        rel.push('/');
        rel.push_str(seg);
    }
    let title = path.trim_matches('/').rsplit('/').next().unwrap_or("");
    let title = if title.is_empty() || title.starts_with(['[', '(']) {
        "Home"
    } else {
        title
    };
    let (file, text) = match kind {
        "page" => {
            let shown: String = params.iter().map(|p| format!("<p>{{{p}}}</p>\n")).collect();
            (
                "+page.wisp",
                format!("<title>{title}</title>\n<h1>{title}</h1>\n{shown}"),
            )
        }
        "layout" => ("+layout.wisp", "<slot />\n".to_string()),
        "error" => (
            "+error.wisp",
            "<h1>{status}</h1>\n<p>{message}</p>\n".to_string(),
        ),
        "server" => (
            "+server.rs",
            "fn get() -> Response {\n    Response::text(\"ok\")\n}\n".to_string(),
        ),
        _ => {
            return Err(format!(
                "There is no kind {kind}: use page, layout, error or server."
            ));
        }
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {rel}: {e}"))?;
    let rel = format!("{rel}/{file}");
    std::fs::File::create_new(dir.join(file))
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("Could not write {rel}: {e}"))?;
    Ok(format!("Wrote {rel}:\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line past the cap is refused without being kept; the next line
    /// is still answered. Invalid UTF-8 and deep nesting do not panic.
    #[test]
    fn hostile_input() {
        let mut input = vec![b'x'; MAX_LINE as usize + 10];
        input
            .extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\xff\xfe\n");
        input.extend_from_slice(&[b'['; 100_000]);
        input.extend_from_slice(b"\n{\"id\":2,\"method\":\"nope\"}\n");
        let mut out = Vec::new();
        serve(&mut &input[..], &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5, "{out}");
        assert!(lines[0].contains("too long"));
        assert!(lines[1].contains("\"id\":1") && lines[1].contains("result"));
        assert!(lines[4].contains("\"id\":2") && lines[4].contains("error"));
    }

    fn reply(line: &str) -> Json {
        json::parse(&answer(line).unwrap()).unwrap()
    }

    #[test]
    fn protocol() {
        // Every message is one line.
        assert!(json::parse(TOOLS).is_ok() && !TOOLS.contains('\n'));
        let init = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        );
        let result = init.get("result").unwrap();
        assert_eq!(result.str("protocolVersion"), Some("2025-03-26"));
        assert!(answer(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let tools = reply(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#);
        let names: Vec<_> = tools
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .items()
            .filter_map(|t| t.str("name"))
            .collect();
        assert_eq!(
            names,
            [
                "wisp_docs",
                "wisp_check",
                "wisp_routes",
                "wisp_components",
                "wisp_new_route"
            ]
        );
        let docs = reply(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"wisp_docs","arguments":{"topic":"hooks"}}}"#,
        );
        let text = docs
            .get("result")
            .unwrap()
            .get("content")
            .unwrap()
            .items()
            .next()
            .unwrap()
            .str("text")
            .unwrap()
            .to_string();
        // AGENTS.md alone, or the site's design page too (it ranks first when present).
        let title = text.lines().next().unwrap_or_default();
        assert!(
            title.starts_with("## ") && title.contains("hooks"),
            "{text}"
        );
        assert!(text.lines().count() > 2, "{text}");
        let bad = reply(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(
            bad.get("error").unwrap().get("code"),
            Some(&Json::Num("-32601".into()))
        );
        assert!(reply("{").get("error").is_some());
    }

    #[test]
    fn docs_sections() {
        let all = sections(DOCS);
        assert!(
            all.iter()
                .any(|s| s.0 == "AGENTS.md > Actions (form posts)")
        );
        assert!(all.iter().any(|s| s.0.ends_with("> Event Modifiers")));
        assert!(docs("").contains("> Webhooks"));
        assert!(docs("zzqq").starts_with("Nothing"));
    }

    #[test]
    fn errors_located() {
        assert_eq!(
            located("src/routes/+page.wisp:3:5: no"),
            (Some("src/routes/+page.wisp"), Some(3), Some(5), "no")
        );
        assert_eq!(located("plain: words").3, "plain: words");
    }

    #[test]
    fn writes_routes() {
        let root = std::env::temp_dir().join(format!("wisp-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let wrote = new_route(&root, "/blog/[slug]", "page").unwrap();
        assert!(wrote.starts_with("Wrote src/routes/blog/[slug]/+page.wisp"));
        assert!(wrote.contains("<p>{slug}</p>"));
        assert!(new_route(&root, "/blog/[slug]", "page").is_err());
        assert!(new_route(&root, "/../x", "page").is_err());
        assert!(new_route(&root, "/x", "nope").is_err());
        new_route(&root, "/api", "server").unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
