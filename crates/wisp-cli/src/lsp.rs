//! `wisp lsp`: a language server for `.wisp` files over stdio. Problems as
//! the file is typed (the build's own parser and checks, on the buffer) and
//! as `wisp check` finds them on save; hovers, go to definition and
//! completion from the text at the cursor. A bad file or a bad message is
//! an answer, never the end of the server.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use wisp_build::fmt;
use wisp_build::ide::{self, Component, Diag};
use wisp_build::routes::{self, Seg};
use wisp_shared::json::{self, Json};

pub fn run() -> Result<(), String> {
    serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
}

/// Answers the messages of `input` on `out` until `exit` or the end.
pub fn serve(input: &mut impl BufRead, out: &mut impl Write) -> Result<(), String> {
    let mut s = Server::default();
    while let Some(body) = read_message(input)? {
        let Ok(msg) = json::parse(&body) else {
            send(out, &reply("null", Err((-32700, "not JSON"))))?;
            continue;
        };
        let method = msg.str("method").unwrap_or("");
        if method == "exit" {
            break;
        }
        let none = Json::Null;
        let params = msg.get("params").unwrap_or(&none);
        let answer = catch_unwind(AssertUnwindSafe(|| s.handle(method, params)));
        let answer = answer.unwrap_or(Err((-32603, "the server failed on this; it goes on")));
        if let Some(id) = msg.get("id") {
            send(out, &reply(&to_json(id), answer))?;
        }
        for note in s.outbox.drain(..) {
            send(out, &note)?;
        }
    }
    Ok(())
}

/// One message's body, after its headers; `None` at the end of the input.
fn read_message(r: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() && len.is_some() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse::<usize>().ok().filter(|&n| n < 1 << 26);
        }
    }
    let mut body = vec![0; len.unwrap_or(0)];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

fn send(out: &mut impl Write, body: &str) -> Result<(), String> {
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

type Answer = Result<String, (i32, &'static str)>;

fn reply(id: &str, answer: Answer) -> String {
    match answer {
        Ok(r) => format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{r}}}"#),
        Err((code, msg)) => format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{}}}}}"#,
            q(msg)
        ),
    }
}

/// A JSON string.
fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A request's id as it came.
fn to_json(v: &Json) -> String {
    match v {
        Json::Num(n) => n.clone(),
        Json::Str(s) => q(s),
        _ => "null".into(),
    }
}

/// An app: what of it was read from disk, last open or save.
struct App {
    /// `None` when they could not be read: component uses go unchecked.
    comps: Option<Vec<Component>>,
    /// Each route's segments and the file that serves it.
    routes: Vec<(String, Vec<Seg>, PathBuf)>,
    /// The project's problem, as `wisp check` finds it.
    problem: Option<(Option<String>, Diag)>,
    /// The file (URI) its problem was shown on, to clear.
    shown: Option<String>,
}

impl App {
    fn load(root: &Path) -> App {
        let tree = routes::scan(&root.join("src").join("routes")).unwrap_or_default();
        let routes = (tree.routes.iter())
            .map(|r| {
                let file = ["+page.wisp", "+page.rs", "+server.rs"]
                    .map(|f| r.dir.join(f))
                    .into_iter()
                    .find(|f| f.is_file())
                    .unwrap_or_else(|| r.dir.clone());
                (r.pattern(), r.segs.clone(), file)
            })
            .collect();
        App {
            comps: ide::components(root).ok(),
            routes,
            problem: ide::check_project(root),
            shown: None,
        }
    }

    fn comp(&self, name: &str) -> Option<&Component> {
        self.comps.iter().flatten().find(|c| c.name == name)
    }
}

struct Doc {
    text: String,
    /// The app it is in (the folder with Cargo.toml and build.rs above it).
    root: Option<PathBuf>,
    /// From the root, `/`-separated.
    rel: String,
}

#[derive(Default)]
struct Server {
    docs: HashMap<String, Doc>,
    apps: HashMap<PathBuf, App>,
    /// Notifications to send after the answer.
    outbox: Vec<String>,
}

impl Server {
    fn handle(&mut self, method: &str, p: &Json) -> Answer {
        let uri = || {
            (p.get("textDocument").and_then(|d| d.str("uri")))
                .unwrap_or("")
                .to_string()
        };
        match method {
            "initialize" => Ok(r##"{"capabilities":{"textDocumentSync":{"openClose":true,"change":1,"save":true},"hoverProvider":true,"definitionProvider":true,"documentFormattingProvider":true,"completionProvider":{"triggerCharacters":["<",":",".","{","#","@","/","\""]}},"serverInfo":{"name":"wisp"}}"##.into()),
            "shutdown" => Ok("null".into()),
            "textDocument/didOpen" => {
                let text = (p.get("textDocument").and_then(|d| d.str("text"))).unwrap_or("");
                self.open(uri(), text.to_string());
                Ok("null".into())
            }
            "textDocument/didChange" => {
                let changes = p.get("contentChanges");
                let text = changes.and_then(|c| c.items().last()).and_then(|c| c.str("text"));
                let uri = uri();
                if let (Some(doc), Some(text)) = (self.docs.get_mut(&uri), text) {
                    doc.text = text.to_string();
                    self.publish(&uri);
                }
                Ok("null".into())
            }
            "textDocument/didSave" => {
                let uri = uri();
                if let Some(root) = self.docs.get(&uri).and_then(|d| d.root.clone()) {
                    self.reload(&root);
                }
                Ok("null".into())
            }
            "textDocument/didClose" => {
                let uri = uri();
                self.docs.remove(&uri);
                self.outbox.push(diagnostics(&uri, "", &[]));
                Ok("null".into())
            }
            "textDocument/formatting" => {
                let uri = uri();
                let Some(doc) = self.docs.get(&uri) else {
                    return Ok("null".into());
                };
                let new = fmt::format(&doc.text, &fmt::edition(&uri_path(&uri)));
                Ok(edit_all(&doc.text, &new))
            }
            "textDocument/hover" | "textDocument/definition" | "textDocument/completion" => {
                let uri = uri();
                let Some(doc) = self.docs.get(&uri) else {
                    return Ok("null".into());
                };
                let pos = p.get("position");
                let num = |k| pos.and_then(|p| p.get(k)).map_or(0, |n| match n {
                    Json::Num(n) => n.parse().unwrap_or(0),
                    _ => 0,
                });
                let off = offset(&doc.text, num("line"), num("character"));
                let app = doc.root.as_ref().and_then(|r| self.apps.get(r));
                let at = At::new(doc, app, off);
                Ok(match method {
                    "textDocument/hover" => at.hover(),
                    "textDocument/definition" => at.definition(),
                    _ => at.completion(),
                }
                .unwrap_or_else(|| "null".into()))
            }
            _ => Err((-32601, "not a method wisp lsp has")),
        }
    }

    fn open(&mut self, uri: String, text: String) {
        let path = uri_path(&uri);
        let root = path
            .ancestors()
            .skip(1)
            .find(|d| d.join("Cargo.toml").is_file() && d.join("build.rs").is_file());
        let rel = match root {
            Some(r) => path
                .strip_prefix(r)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"),
            None => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let root = root.map(Path::to_path_buf);
        let fresh = root.as_ref().is_some_and(|r| !self.apps.contains_key(r));
        self.docs.insert(
            uri.clone(),
            Doc {
                text,
                root: root.clone(),
                rel,
            },
        );
        match root {
            Some(r) if fresh => self.reload(&r),
            _ => self.publish(&uri),
        }
    }

    /// Reads the app at `root` again, and shows its problems anew.
    fn reload(&mut self, root: &Path) {
        let mut app = App::load(root);
        app.shown = self.apps.remove(root).and_then(|a| a.shown);
        let is_open = |rel: &str| {
            (self.docs.values()).any(|d| d.root.as_deref() == Some(root) && d.rel == rel)
        };
        if let Some(old) = app.shown.take() {
            self.outbox.push(diagnostics(&old, "", &[]));
        }
        // A problem in a file not open shows there too (the Problems list).
        if let Some((Some(file), d)) = &app.problem
            && !is_open(file)
        {
            let uri = path_uri(&root.join(file));
            let text = wisp_build::read_source(&root.join(file)).unwrap_or_default();
            self.outbox
                .push(diagnostics(&uri, &text, std::slice::from_ref(d)));
            app.shown = Some(uri);
        }
        self.apps.insert(root.to_path_buf(), app);
        let open: Vec<String> = (self.docs.iter())
            .filter(|(_, d)| d.root.as_deref() == Some(root))
            .map(|(u, _)| u.clone())
            .collect();
        for uri in open {
            self.publish(&uri);
        }
    }

    /// The problems of open document `uri`: its own as typed, else the
    /// project's in it.
    fn publish(&mut self, uri: &str) {
        let Some(doc) = self.docs.get(uri) else {
            return;
        };
        let app = doc.root.as_ref().and_then(|r| self.apps.get(r));
        let comps = app.and_then(|a| a.comps.as_deref());
        let own = (doc.rel.ends_with(".wisp"))
            .then(|| ide::check_file(&doc.rel, &doc.text, comps))
            .flatten();
        let project = app
            .and_then(|a| a.problem.as_ref())
            .filter(|(f, _)| f.as_ref().is_none_or(|f| *f == doc.rel))
            .map(|(_, d)| d.clone());
        let diags: Vec<Diag> = own.or(project).into_iter().collect();
        self.outbox.push(diagnostics(uri, &doc.text, &diags));
    }
}

fn diagnostics(uri: &str, text: &str, diags: &[Diag]) -> String {
    let items: Vec<String> = (diags.iter())
        .map(|d| {
            let line = d.line.saturating_sub(1) as usize;
            let src = text.split('\n').nth(line).unwrap_or("");
            let (start, end) = match d.col {
                0 => (0, src.len()),
                c => {
                    let s = src
                        .char_indices()
                        .nth(c as usize - 1)
                        .map_or(src.len(), |(i, _)| i);
                    let e = s + src[s..]
                        .find(|c: char| !is_word(c))
                        .unwrap_or(src.len() - s);
                    (
                        s,
                        e.max(s + src[s..].chars().next().map_or(0, char::len_utf8)),
                    )
                }
            };
            format!(
                r#"{{"range":{},"severity":1,"source":"wisp","message":{}}}"#,
                range(line, src, start, end),
                q(&d.msg)
            )
        })
        .collect();
    format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":{},"diagnostics":[{}]}}}}"#,
        q(uri),
        items.join(",")
    )
}

/// The text edits that turn `old` into `new`: none, or one of the whole text.
fn edit_all(old: &str, new: &str) -> String {
    if old == new {
        return "[]".into();
    }
    let lines = old.matches('\n').count();
    let last = &old[old.rfind('\n').map_or(0, |i| i + 1)..];
    format!(
        r#"[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":{lines},"character":{}}}}},"newText":{}}}]"#,
        last.encode_utf16().count(),
        q(new)
    )
}

/// An LSP range on `line` (whose text is `src`) from byte `a` to `b`.
fn range(line: usize, src: &str, a: usize, b: usize) -> String {
    let col = |i: usize| src[..i.min(src.len())].encode_utf16().count();
    format!(
        r#"{{"start":{{"line":{line},"character":{}}},"end":{{"line":{line},"character":{}}}}}"#,
        col(a),
        col(b)
    )
}

/// The byte offset of an LSP position (UTF-16 columns), clamped to the text.
fn offset(text: &str, line: usize, ch: usize) -> usize {
    let start = match line {
        0 => 0,
        n => match text.match_indices('\n').nth(n - 1) {
            Some((i, _)) => i + 1,
            None => return text.len(),
        },
    };
    let mut units = 0;
    for (i, c) in text[start..].char_indices() {
        if units >= ch || c == '\n' {
            return start + i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What can go in an attribute's name: `on:click.prevent`, `style:--x`.
fn is_attr(c: char) -> bool {
    is_word(c) || matches!(c, ':' | '.' | '-' | '|')
}

/// `file:///c%3A/x/y.wisp` → `c:/x/y.wisp` (`/x/y.wisp` elsewhere).
fn uri_path(uri: &str) -> PathBuf {
    let raw = uri.strip_prefix("file://").unwrap_or(uri).as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = raw
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) if raw[i] == b'%' => {
                out.push(b);
                i += 3;
            }
            _ => {
                out.push(raw[i]);
                i += 1;
            }
        }
    }
    let path = String::from_utf8_lossy(&out).into_owned();
    // `/c:/x` is `c:/x` on Windows.
    let b = path.as_bytes();
    match b.len() > 2 && b[0] == b'/' && b[2] == b':' {
        true => PathBuf::from(&path[1..]),
        false => PathBuf::from(path),
    }
}

fn path_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if s.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'/'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b':'
            | b'+'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'=' => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The directives and their docs (hover, completion).
const DIRECTIVES: [(&str, &str); 9] = [
    (
        "on:",
        "`on:click=\"count++\"`: runs JavaScript on the event. Modifiers: `.prevent .stop .once .self .capture .passive .window .document .outside .debounce.300ms .enter .escape .ctrl .shift .alt .meta`.",
    ),
    (
        "bind:",
        "`bind:value=\"q\"`, `bind:checked`, `bind:this=\"el\"`: the element and a script variable, both ways. With no `let q` anywhere, it declares it.",
    ),
    (
        "class:",
        "`class:on={bool}` toggles a class on the server; `class:on=\"js\"` in the browser.",
    ),
    (
        "style:",
        "`style:--x=\"js\"`: a style property from JavaScript.",
    ),
    (
        "use:",
        "`use:action=\"arg\"`: calls `action(el, arg)` when the element is added.",
    ),
    (
        "transition:",
        "`transition:fade|slide|scale|fly`: plays when the element comes or goes.",
    ),
    (
        "animate:",
        "`animate:flip`: moves an element of a keyed `{:#each}` smoothly.",
    ),
    (
        "client:",
        "`client:visible|idle|interaction|media=\"(…)\"|none`: when an island's code loads.",
    ),
    (
        ":",
        "`:attr=\"js\"`: an attribute from JavaScript (`:hidden=\"!open\"`); `:text=\"js\"` sets the text.",
    ),
];

/// What may follow a directive's `:`.
const NAMES: [(&str, &[&str]); 5] = [
    (
        "on:",
        &[
            "click",
            "input",
            "change",
            "submit",
            "keydown",
            "keyup",
            "focus",
            "blur",
            "pointerdown",
            "pointermove",
            "pointerup",
            "dblclick",
            "contextmenu",
            "scroll",
            "animationend",
        ],
    ),
    ("bind:", &["value", "checked", "this", "group"]),
    ("transition:", &["fade", "slide", "scale", "fly"]),
    ("animate:", &["flip"]),
    (
        "client:",
        &["visible", "idle", "interaction", "media", "none"],
    ),
];

/// The template blocks: what to write, and what it does.
const BLOCKS: [(&str, &str, &str); 20] = [
    (
        "{#if",
        "{#if ${1:cond}}\n\t$0\n{/if}",
        "`{#if c}…{:else if c}…{:else}…{/if}`; `if let Some(x) = y` works.",
    ),
    (
        "{#each",
        "{#each ${1:list} as ${2:item}}\n\t$0\n{/each}",
        "`{#each list as item, i}…{:else}…{/each}`: a for loop; `{:else}` when it is empty.",
    ),
    (
        "{#match",
        "{#match ${1:value}}\n{:case ${2:pattern}}\n\t$0\n{/match}",
        "`{#match e}{:case P}…{/match}`: a Rust match.",
    ),
    (
        "{#await",
        "{#await ${1:future}}\n\t$0\n{:then ${2:value}}\n{:catch e}\n{/await}",
        "`{#await f}…{:then v}…{:catch e}…{/await}`: a page sends the rest first, then `v` or `e`.",
    ),
    (
        "{#snippet",
        "{#snippet ${1:name}(${2})}\n\t$0\n{/snippet}",
        "`{#snippet row(a, b)}…{/snippet}`: markup to `{@render row(x, 1)}` later.",
    ),
    (
        "{:else if",
        "{:else if ${1:cond}}",
        "Another branch of `{#if}`.",
    ),
    (
        "{:else",
        "{:else}",
        "The branch of `{#if}` when none is true, or of `{#each}` when the list is empty.",
    ),
    ("{:case", "{:case ${1:pattern}}", "An arm of `{#match}`."),
    (
        "{@html",
        "{@html ${1:expr}}",
        "`{@html expr}`: not escaped. Trusted HTML only.",
    ),
    (
        "{@const",
        "{@const ${1:x} = ${2:expr}}",
        "`{@const x = expr}`: a `let` in the markup.",
    ),
    (
        "{@render",
        "{@render ${1:children}()}",
        "`{@render children()}` shows a layout's or component's children; `{@render row(x)}` a snippet.",
    ),
    (
        "{@props",
        "{@props ${1:title}: ${2:&str}}",
        "`{@props title: &str, n: u32 = 0}`: what a component takes, typed and checked at build.",
    ),
    (
        "{:#if",
        "{:#if ${1:cond}}\n\t$0\n{:/if}",
        "`{:#if js}…{:/if}`: an if the browser keeps up to date.",
    ),
    (
        "{:#each",
        "{:#each ${1:items} as ${2:it} (${2:it}.id)}\n\t$0\n{:/each}",
        "`{:#each items as it, i (it.id)}…{:/each}`: a keyed list the browser keeps up to date.",
    ),
    (
        "{:#key",
        "{:#key ${1:expr}}\n\t$0\n{:/key}",
        "`{:#key expr}`: its content drawn afresh when `expr` changes.",
    ),
    (
        "{:#await",
        "{:#await ${1:promise}}\n\t$0\n{:/await}",
        "`{:#await promise}`: pending, then the value or the error.",
    ),
    (
        "{:#try",
        "{:#try}\n\t$0\n{:/try}",
        "`{:#try}`: what fails inside it shows its `{:catch}` part.",
    ),
    (
        "{:@render",
        "{:@render ${1:snippet}(${2})}",
        "`{:@render s(x)}`: a snippet the browser renders.",
    ),
    ("{/", "", "Closes the block."),
    ("{:/", "", "Closes the browser block."),
];

/// The place a request is about.
struct At<'a> {
    doc: &'a Doc,
    app: Option<&'a App>,
    off: usize,
    /// The tag the cursor is in, `<Card …`: its name's start and the name.
    tag: Option<(usize, &'a str)>,
    /// The attribute-like word at the cursor: its start and end.
    word: (usize, usize),
}

impl<'a> At<'a> {
    fn new(doc: &'a Doc, app: Option<&'a App>, off: usize) -> At<'a> {
        let t = &doc.text;
        let off = (0..=off.min(t.len()))
            .rev()
            .find(|&i| t.is_char_boundary(i))
            .unwrap_or(0);
        let before = &t[..off];
        let tag = match (before.rfind('<'), before.rfind('>')) {
            (Some(lt), gt) if gt.is_none_or(|gt| gt < lt) => {
                let name = &t[lt + 1..];
                let end = name.find(|c: char| !is_attr(c)).unwrap_or(name.len());
                Some((lt + 1, &name[..end]))
            }
            _ => None,
        };
        let start = before.rfind(|c: char| !is_attr(c)).map_or(0, |i| {
            i + before[i..].chars().next().map_or(1, char::len_utf8)
        });
        let end = off
            + t[off..]
                .find(|c: char| !is_attr(c))
                .unwrap_or(t.len() - off);
        At {
            doc,
            app,
            off,
            tag,
            word: (start, end),
        }
    }

    fn text(&self) -> &'a str {
        &self.doc.text
    }

    /// The component whose tag the cursor is in.
    fn comp(&self) -> Option<&'a Component> {
        self.app?.comp(self.tag?.1)
    }

    /// The cursor is on the tag's name.
    fn on_tag_name(&self) -> bool {
        self.tag.is_some_and(|(s, _)| s == self.word.0)
    }

    /// The `{` of the block keyword the cursor is in (`{#ea|ch`).
    fn brace(&self) -> Option<usize> {
        let t = self.text();
        let line_start = t[..self.off].rfind('\n').map_or(0, |i| i + 1);
        let open = line_start + t[line_start..self.off].rfind('{')?;
        let head = t[open + 1..]
            .find(|c: char| !(matches!(c, '#' | ':' | '/' | '@') || c.is_ascii_alphabetic()))
            .unwrap_or(t.len() - open - 1);
        (self.off <= open + 1 + head).then_some(open)
    }

    /// The template block the cursor is on: its entry in `BLOCKS`.
    fn block(&self) -> Option<&'static (&'static str, &'static str, &'static str)> {
        let rest = &self.text()[self.brace()?..];
        let word_ends = |b: &&(&str, &str, &str)| {
            rest.starts_with(b.0)
                && !rest[b.0.len()..].starts_with(|c: char| c.is_ascii_alphabetic())
        };
        (BLOCKS.iter().find(word_ends)).or_else(|| {
            BLOCKS
                .iter()
                .find(|b| b.1.is_empty() && rest.starts_with(b.0))
        })
    }

    fn hover(&self) -> Option<String> {
        let word = &self.text()[self.word.0..self.word.1];
        let md = if let Some((_, _, doc)) = self.block() {
            doc.to_string()
        } else if let (true, Some(c)) = (self.on_tag_name(), self.comp()) {
            comp_doc(c)
        } else if let Some(p) = self
            .comp()
            .and_then(|c| c.props.iter().find(|p| p.name == word))
        {
            format!(
                "```rust\n{}\n```\nA prop of <{}>.",
                prop_sig(p),
                self.tag?.1
            )
        } else if let (Some(_), Some((_, doc))) =
            (self.tag, DIRECTIVES.iter().find(|d| word.starts_with(d.0)))
        {
            doc.to_string()
        } else {
            let params = ide::route_params(&self.doc.rel);
            let (name, ty) = params.iter().find(|(n, _)| n == word)?;
            format!("```rust\n{name}: {ty}\n```\nA route param of this page.")
        };
        Some(format!(
            r#"{{"contents":{{"kind":"markdown","value":{}}}}}"#,
            q(&md)
        ))
    }

    fn definition(&self) -> Option<String> {
        let root = self.doc.root.as_ref()?;
        let file = if let (true, Some(c)) = (self.on_tag_name(), self.comp()) {
            root.join(&c.rel)
        } else {
            let (_, value, attr) = self.value()?;
            if let Some(lib) = value.strip_prefix("$lib/") {
                root.join("src").join("lib").join(lib)
            } else if attr == "href" && value.starts_with('/') && !value.contains('{') {
                let path = value.split(['?', '#']).next().unwrap_or("");
                let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                let app = self.app?;
                app.routes.iter().find(|r| matches(&r.1, &segs))?.2.clone()
            } else {
                return None;
            }
        };
        file.exists().then(|| {
            let zero = r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}"#;
            format!(r#"{{"uri":{},"range":{zero}}}"#, q(&path_uri(&file)))
        })
    }

    /// The quoted value the cursor is in: where it starts, it, and its
    /// attribute's name (or `""` outside a tag): `href="/about"`,
    /// `import x from '$lib/x.js'`.
    fn value(&self) -> Option<(usize, &'a str, &'a str)> {
        let t = self.text();
        let line_start = t[..self.off].rfind('\n').map_or(0, |i| i + 1);
        let from = self.tag.map_or(line_start, |(s, _)| s);
        let mut open = None;
        for (i, c) in t[from..self.off].char_indices() {
            match open {
                None if c == '"' || c == '\'' => open = Some((from + i, c)),
                Some((_, q)) if c == q => open = None,
                _ => {}
            }
        }
        let (open, quote) = open?;
        let close = self.off + t[self.off..].find([quote, '\n'])?;
        if !t[close..].starts_with(quote) {
            return None;
        }
        let name = t[..open].strip_suffix('=').unwrap_or("");
        let attr = &name[name.rfind(|c: char| !is_attr(c)).map_or(0, |i| i + 1)..];
        Some((open + 1, &t[open + 1..close], attr))
    }

    fn completion(&self) -> Option<String> {
        let t = self.text();
        let mut items = Vec::new();
        let mut item = |label: &str, kind: u8, start: usize, text: &str, doc: &str| {
            items.push(format!(
                r#"{{"label":{},"kind":{kind},"filterText":{},"insertTextFormat":2,"textEdit":{{"range":{},"newText":{}}},"documentation":{{"kind":"markdown","value":{}}}}}"#,
                q(label),
                q(&t[start..self.off]),
                self.range(start),
                q(text),
                q(doc)
            ));
        };
        let in_value = self.value().filter(|_| self.tag.is_some());
        if let Some(open) = self.brace().filter(|_| in_value.is_none()) {
            let typed = &t[open..self.off];
            for (key, snip, doc) in BLOCKS
                .iter()
                .filter(|b| !b.1.is_empty() && b.0.starts_with(typed))
            {
                item(&format!("{key}}}"), 15, open, snip, doc);
            }
            if typed.starts_with("{/") || typed.starts_with("{:/") {
                let sigil = &typed[..typed.find('/').unwrap_or(0) + 1];
                for b in ["if", "each", "match", "snippet", "key", "await", "try"] {
                    item(
                        &format!("{sigil}{b}}}"),
                        15,
                        open,
                        &format!("{sigil}{b}}}"),
                        "Closes the block.",
                    );
                }
            }
        } else if let Some((start, _, "href")) = in_value {
            for (pattern, ..) in self.app.iter().flat_map(|a| &a.routes) {
                item(pattern, 17, start, pattern, "A route.");
            }
        } else if in_value.is_some() || self.tag.is_none() {
            return Some("[]".into());
        } else if self.on_tag_name() {
            for c in self.app.iter().flat_map(|a| a.comps.iter().flatten()) {
                let mut snip = c.name.clone();
                let needed = c.props.iter().filter(|p| p.default.is_none());
                for (k, p) in needed.enumerate() {
                    snip.push_str(&format!(" {}={{${}}}", p.name, k + 1));
                }
                item(&c.name, 7, self.word.0, &snip, &comp_doc(c));
            }
        } else {
            let typed = &t[self.word.0..self.off];
            let cut = typed.rfind([':', '.']).map_or(0, |i| i + 1);
            let start = self.word.0 + cut;
            let head = &typed[..cut];
            if head.is_empty() {
                for (d, doc) in DIRECTIVES {
                    item(d, 14, start, d, doc);
                }
                for p in self.comp().iter().flat_map(|c| &c.props) {
                    item(
                        &p.name,
                        10,
                        start,
                        &format!("{}={{$1}}", p.name),
                        &format!("```rust\n{}\n```", prop_sig(p)),
                    );
                }
            } else if head.starts_with("on:") && head.contains('.') {
                let more = [
                    "debounce", "enter", "escape", "space", "up", "down", "left", "right",
                ];
                for m in wisp_shared::protocol::ON_FLAGS.iter().chain(&more) {
                    item(m, 14, start, m, "A modifier of `on:`.");
                }
            } else if let Some((d, names)) = NAMES.iter().find(|(d, _)| head == *d) {
                let doc = DIRECTIVES.iter().find(|x| x.0 == *d).map_or("", |x| x.1);
                for n in *names {
                    item(n, 12, start, n, doc);
                }
            }
        }
        Some(format!("[{}]", items.join(",")))
    }

    /// An LSP range from byte `start` to the cursor (on one line).
    fn range(&self, start: usize) -> String {
        let t = self.text();
        let line_start = t[..start].rfind('\n').map_or(0, |i| i + 1);
        let line = t[..start].matches('\n').count();
        let line_end = line_start + t[line_start..].find('\n').unwrap_or(t.len() - line_start);
        range(
            line,
            &t[line_start..line_end],
            start - line_start,
            self.off - line_start,
        )
    }
}

/// A component's signature, for hovers and completion.
fn comp_doc(c: &Component) -> String {
    let props: Vec<String> = c.props.iter().map(prop_sig).collect();
    format!(
        "```rust\n{{@props {}}}\n```\n<{}>{}: {}",
        props.join(", "),
        c.name,
        if c.children { " shows children" } else { "" },
        c.rel
    )
}

fn prop_sig(p: &ide::PropDecl) -> String {
    match &p.default {
        Some(d) => format!("{}: {} = {d}", p.name, p.ty),
        None => format!("{}: {}", p.name, p.ty),
    }
}

/// Whether a route of `segs` matches the path `/a/b` split as `path`.
fn matches(segs: &[Seg], path: &[&str]) -> bool {
    match (segs.first(), path.first()) {
        (None, _) => path.is_empty(),
        (Some(Seg::Rest(_)), _) => (0..=path.len()).any(|k| matches(&segs[1..], &path[k..])),
        (Some(Seg::Optional(..)), _) => {
            matches(&segs[1..], path) || (!path.is_empty() && matches(&segs[1..], &path[1..]))
        }
        (Some(_), None) => false,
        (Some(Seg::Static(s)), Some(p)) => s == p && matches(&segs[1..], &path[1..]),
        (Some(_), Some(_)) => matches(&segs[1..], &path[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    /// The messages the server sent, in order.
    fn messages(out: &[u8]) -> Vec<Json> {
        let mut r = out;
        let mut all = Vec::new();
        while let Some(m) = read_message(&mut r).unwrap() {
            all.push(json::parse(&m).unwrap());
        }
        all
    }

    #[test]
    fn talks_lsp() {
        let root = std::env::temp_dir().join(format!("wisp-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let files = [
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("build.rs", "fn main() { wisp_build::run() }"),
            (
                "src/components/Card.wisp",
                "{@props title: &str, n: u32 = 0}\n<h2>{title}</h2>",
            ),
            ("src/routes/about/+page.wisp", "<p>About</p>"),
            ("src/routes/blog/[slug]/+page.wisp", "<h1>{slug}</h1>"),
            ("src/lib/x.js", "export const x = 1"),
        ];
        for (f, text) in files {
            std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            std::fs::write(root.join(f), text).unwrap();
        }
        let uri = path_uri(&root.join("src/routes/blog/[slug]/+page.wisp"));
        let page = "<Card />\n<a href=\"/about\" on:click.prevent=\"go\">{slug}</a>\n<";
        let pos = |id: u32, method: &str, line: u32, ch: u32| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/{method}","params":{{"textDocument":{{"uri":{}}},"position":{{"line":{line},"character":{ch}}}}}}}"#,
                q(&uri)
            )
        };
        let input: String = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string(),
            "not json".into(),
            format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":{},"languageId":"wisp","version":1,"text":{}}}}}}}"#,
                q(&uri),
                q(page)
            ),
            pos(2, "hover", 0, 2),
            pos(3, "hover", 1, 25),
            pos(4, "hover", 1, 42),
            pos(5, "definition", 1, 11),
            pos(6, "definition", 0, 2),
            pos(7, "completion", 2, 1),
            pos(8, "completion", 1, 31),
            r#"{"jsonrpc":"2.0","id":9,"method":"nope"}"#.into(),
            r#"{"jsonrpc":"2.0","method":"exit"}"#.into(),
        ]
        .iter()
        .map(|m| frame(m))
        .collect();
        let mut out = Vec::new();
        serve(&mut input.as_bytes(), &mut out).unwrap();
        let msgs = messages(&out);
        let by_id = |id: &str| {
            let m = msgs
                .iter()
                .find(|m| m.get("id") == Some(&Json::Num(id.into())));
            let m = m.unwrap_or_else(|| panic!("no answer {id}"));
            format!("{:?}", m.get("result").or(m.get("error")).unwrap())
        };
        let has = |id: &str, want: &str| {
            let got = by_id(id);
            assert!(got.contains(want), "{id}: {want} not in {got}");
        };
        has("1", "hoverProvider");
        assert!(msgs.iter().any(|m| m.get("id") == Some(&Json::Null)));
        let diags = msgs
            .iter()
            .find(|m| m.str("method") == Some("textDocument/publishDiagnostics"));
        let diags = format!("{:?}", diags.unwrap());
        assert!(diags.contains("needs `title`"), "{diags}");
        has("2", "title: &str");
        has("3", "debounce");
        has("4", "slug: String");
        has("5", "about/+page.wisp");
        has("6", "Card.wisp");
        has("7", "title={$1}");
        has("8", "prevent");
        has("9", "-32601");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn formats() {
        let open = |text: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///nowhere/x.wisp","languageId":"wisp","version":1,"text":{}}}}}}}"#,
                q(text)
            )
        };
        let format = r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///nowhere/x.wisp"},"options":{"tabSize":2,"insertSpaces":true}}}"#;
        let input: String = [
            &open("<div>\n<p>é</p>\n</div>"),
            format,
            &open("<p>a</p>\n"),
            format,
        ]
        .iter()
        .map(|m| frame(m))
        .collect();
        let mut out = Vec::new();
        serve(&mut input.as_bytes(), &mut out).unwrap();
        let answers: Vec<String> = (messages(&out).iter())
            .filter_map(|m| m.get("result").map(|r| format!("{r:?}")))
            .collect();
        assert_eq!(answers.len(), 2, "{answers:?}");
        let want = r#"("end", Obj([("line", Num("2")), ("character", Num("6"))]))])), ("newText", Str("<div>\n  <p>é</p>\n</div>\n"))"#;
        assert!(answers[0].contains(want), "{}", answers[0]);
        assert_eq!(answers[1], "Arr([])");
    }

    #[test]
    fn positions() {
        let t = "aé😀b\nxy";
        assert_eq!(offset(t, 0, 4), "aé😀".len());
        assert_eq!(offset(t, 1, 1), t.len() - 1);
        assert_eq!(offset(t, 9, 0), t.len());
        assert_eq!(
            uri_path("file:///c%3A/a%20b/x.wisp"),
            PathBuf::from("c:/a b/x.wisp")
        );
        assert!(matches(
            &[Seg::Static("a".into()), Seg::Rest("r".into())],
            &["a", "b", "c"]
        ));
    }
}
