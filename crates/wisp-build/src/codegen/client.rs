//! Browser code: the ES module of a template with a client script or
//! directives.

use super::*;

// ---- browser code -----------------------------------------------------------
//
// A template with a client script or directives compiles to an ES module
// that the page loads (`/_app/c/ID.js`). Its script runs once per instance
// (per render of the template), inside a function whose first parameter
// holds the server values the file's browser code reads, and it returns a
// list of bindings per element (per `Group`). The runtime (`live.js`)
// matches elements to groups by their `data-w`.
//
// Names in browser code are JavaScript's, except the server's: `data` in a
// page or layout that loads, a component's props, and on an element, the
// Rust loop, `if let`, `{:case}` and `{@const}` names around it. Only what
// the code reads is sent: `data.user.name` sends that one field.

/// JSON as a render function writes it: fixed text, and Rust values.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Piece {
    Text(String),
    /// A place such as `data.user.name`, written with `wisp::Json`; `line`
    /// is where the browser code reads it, for rustc's errors.
    Value {
        expr: String,
        line: u32,
    },
}

/// A template's browser module, and what each render of it sends.
pub(super) struct Client {
    /// `t3`, from the template's id.
    pub(super) id: String,
    /// What it imports statically, all the way down, but the runtime: the
    /// page preloads it with the module (see `Project::browser`).
    pub(super) preload: Vec<String>,
    pub(super) source: String,
    /// Where each line of `source` came from in the file, for its map.
    pub(super) lines: Vec<sourcemap::Line>,
    /// Of `source`, for the module's URL.
    pub(super) hash: String,
    /// The keys its `t('key')` calls show, by index (see `i18n`).
    pub(super) texts: Vec<usize>,
    /// The instance's server values: a JSON object.
    pub(super) blob: Vec<Piece>,
    /// Per group: the loop values its directives read (a JSON object), or
    /// nothing.
    pub(super) locals: Vec<Vec<Piece>>,
    /// The components it renders in the browser, whose modules it imports.
    pub(super) uses: Vec<usize>,
    /// A component some page renders in the browser: it gets a `paint` fn.
    pub(super) paints: bool,
    // For the first paint (see `resolve`): the server values it knows, the
    // script's top-level names and what each is first set to, and per
    // group the Rust names around it.
    pub(super) server: Vec<String>,
    /// Server values are props, read whole (their types need not have the
    /// fields the browser code reads), not `data`, read a field at a time.
    pub(super) whole: bool,
    pub(super) declared: Vec<String>,
    pub(super) lets: Vec<(String, String)>,
    pub(super) scopes: Vec<Vec<String>>,
}

impl Client {
    pub(super) fn path(&self) -> String {
        format!("{MODULES}{}.js", self.id)
    }
}

/// A JavaScript file served as it is, but for its imports: `src/lib/**.js`
/// and each `+page.js`; or a file of `.wisp/npm`, embedded as it is.
pub(super) struct JsFile {
    pub(super) path: String,
    pub(super) hash: String,
    pub(super) source: String,
    /// The `.wisp/npm` file: its path names its package's version, so its
    /// URL needs no `?v=`.
    pub(super) file: Option<PathBuf>,
}

/// The helpers every module's function takes. Most are scoped to the
/// instance; the rest are live.js's exports, handed over so a script needs
/// no import for them.
pub(super) const HELPERS: &str = "tick, flushSync, onError, tweened, spring, crossfade, untrack, setTimeout, setInterval, requestAnimationFrame, addEventListener, listen, onMount, onDestroy, effect, watch, \
                       derived, store, persisted, emit, setContext, getContext, goto, invalidate, matches, page, navigating, enhance, \
                       pushState, replaceState, context, portal,__wisp_s, __wisp_r, __wisp_d, __wisp_e, __wisp_ep, __wisp_er, __wisp_et, __wisp_snap, __wisp_props, __wisp_eq, __wisp_t";

/// extra.js's helpers, handed over only to a module whose code names them
/// (each name costs every module that takes it).
const MORE: [&str; 8] = [
    "announce",
    "optimistic",
    "outside",
    "inview",
    "shortcut",
    "modal",
    "preload",
    "keepscroll",
];

/// What `client` needs to know beyond the template.
pub(super) struct ClientCx<'a> {
    pub(super) comps: &'a [Comp],
    pub(super) templates: &'a [Tpl],
    /// Some page renders this component in the browser.
    pub(super) as_client: bool,
    pub(super) specs: &'a Specs,
    /// The page's `+page.js`, served at this URL.
    pub(super) load: Option<String>,
    /// A release build: `$inspect` goes.
    pub(super) release: bool,
    /// Source maps: the module names its map, not its file.
    pub(super) maps: bool,
    /// The `PUBLIC_*` variables.
    pub(super) env: &'a [(String, String)],
    /// `src/locales`, for `t('key')`.
    pub(super) i18n: Option<&'a i18n::Locales>,
    /// The URL of the runtime's less used half (`extra.js`).
    pub(super) extra: &'a str,
    /// The URL of `more.js`, the `MORE` helpers.
    pub(super) more: &'a str,
    /// The `#[remote]` functions, which a script calls without an import.
    pub(super) remotes: &'a [String],
}

/// Whether a directive needs `extra.js` (so does a module whose code makes
/// a Map or a Set, or uses `enhance`, `$state.snapshot`, `persisted`,
/// `tweened`, `spring`, `crossfade`, `announce`, `optimistic` or the actions
/// `outside`, `inview`, `shortcut`, `modal`, `preload` and `keepscroll`).
pub(super) fn is_extra(d: &Directive) -> bool {
    matches!(
        d.kind,
        Dir::Transition
            | Dir::Animate
            | Dir::Await
            | Dir::Try
            | Dir::Tag
            | Dir::Spread
            | Dir::Wait
            | Dir::Comp
    ) || (d.kind == Dir::Bind && !matches!(d.name.as_str(), "value" | "checked" | "this"))
        || (d.kind == Dir::Hole && !d.name.is_empty())
}

/// A place in a script, as a line and column of its file.
pub(super) fn script_pos(s: &template::Script, off: usize) -> (u32, u32) {
    let before = &s.src[..off];
    let col = match before.rfind('\n') {
        Some(n) => before[n + 1..].chars().count() as u32 + 1,
        None => s.col + before.chars().count() as u32,
    };
    (s.line + before.matches('\n').count() as u32, col)
}

/// The components `nodes` has the browser render.
pub(super) fn client_uses(t: &Template) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in t.groups.iter().flat_map(|g| &g.directives) {
        if d.kind == Dir::Comp && !out.contains(&d.name) {
            out.push(d.name.clone());
        }
    }
    out
}

/// The browser module of a template with browser code, or `None`.
pub(super) fn client(t: &Tpl, cx: &ClientCx) -> Result<Option<Client>, String> {
    let tt = &t.t;
    if !tt.is_live() && !cx.as_client && cx.load.is_none() {
        return Ok(None);
    }
    let at = |line: u32, col: u32, msg: String| format!("{}:{line}:{col}: {msg}", t.rel);
    let server_load = matches!(t.user, Some((_, true)));
    let mut server: Vec<String> = match t.kind {
        // `data`, and each of its names alone (`items` for `data.items`),
        // but when a `+page.js` makes `data` in the browser.
        Kind::Page | Kind::Layout if server_load => {
            let mut names = vec!["data".to_string()];
            if cx.load.is_none() {
                let lets = t.stmts.as_ref().map(|(s, binds)| {
                    let mut n = rust_scan::let_names(s);
                    n.extend(rust_scan::let_names(&binds.join("\n")));
                    n
                });
                let fields = t.data.iter().map(|(n, _)| n.clone());
                for n in lets.unwrap_or_default().into_iter().chain(fields) {
                    let own = n.starts_with("__") || matches!(n.as_str(), "cx" | "children");
                    let js_own = js::is_reserved(&n) || js::is_global(&n);
                    if !own && !js_own && !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
            names
        }
        Kind::Component => tt
            .props
            .iter()
            .flat_map(|(ds, _)| ds)
            .map(|d| d.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let script = tt.script.as_ref();
    let original = script.map_or("", |s| s.src.as_str());
    let script_at = |off: usize| script_pos(script.expect("an offset in the script"), off);
    let script_err = |(off, msg): (usize, String)| {
        let (line, col) = script_at(off);
        at(line, col, msg)
    };
    // A component's `let { a, b = 1 } = $props()` names props it reads,
    // with their browser defaults. Blanked, it declares nothing.
    let rune = js::props_rune(original).map_err(script_err)?;
    let blanked;
    let src = match &rune {
        Some(p) => {
            let (line, col) = script_at(p.span.0);
            if t.kind != Kind::Component {
                return Err(at(
                    line,
                    col,
                    "`$props()` is for components; a page's server values are `data`".into(),
                ));
            }
            if let Some(name) = p
                .props
                .iter()
                .map(|x| &x.name)
                .find(|n| !server.contains(n))
            {
                return Err(at(
                    line,
                    col,
                    format!(
                        "`{name}` is not a prop of this component: declare it in {{@props …}}, with its Rust type{}",
                        if server.is_empty() {
                            String::new()
                        } else {
                            format!(" (it has {})", server.join(", "))
                        }
                    ),
                ));
            }
            blanked = js::blank(original, &[p.span]);
            blanked.as_str()
        }
        None => original,
    };
    // `export const snapshot = { capture, restore }`: state kept with the
    // history entry (extra.js). Its `export` blanked, it is a declaration.
    let mut snap = false;
    let exported;
    let src = {
        let t = js::tokens(src);
        let word = |k: usize| t.get(k).map(|x: &js::Token| x.text(src));
        let mut spans = Vec::new();
        for k in
            (0..t.len()).filter(|&k| t[k].depth == 0 && !t[k].member && word(k) == Some("export"))
        {
            if !(matches!(word(k + 1), Some("const" | "let")) && word(k + 2) == Some("snapshot")) {
                return Err(script_err((
                    t[k].start,
                    "a script exports only `export const snapshot = { capture, restore }`".into(),
                )));
            }
            snap = true;
            spans.push((t[k].start, t[k].end));
        }
        exported = js::blank(src, &spans);
        exported.as_str()
    };
    let declared = js::declarations(src);
    let imported = js::import_names(src, &js::imports(src));
    // A name a page's script declares is the script's, as a name its Rust
    // has too: `let guess = data.guess`.
    if t.kind != Kind::Component {
        server.retain(|n| {
            n == "data" || !(declared.iter().any(|(d, _)| d == n) || imported.contains(n))
        });
    }
    // A server name the script declares too could mean either.
    let clash = |name: &str| -> Result<(), String> {
        match declared.iter().find(|(n, _)| n == name) {
            Some((_, off)) => {
                let (line, col) = script_at(*off);
                Err(at(
                    line,
                    col,
                    format!("`{name}` is both a server value and a script variable; rename one"),
                ))
            }
            None => Ok(()),
        }
    };
    // What a component sends is each prop it reads, whole; a page's `data`
    // is sent a field at a time, or whole when a `+page.js` loads from it
    // (which the browser cannot know in advance).
    let sent = |path: &[String]| -> Vec<String> {
        if t.kind == Kind::Component || cx.load.is_some() {
            path[..1].to_vec()
        } else {
            server_path(path)
        }
    };

    // The server values read, in order, and where.
    let mut used: Vec<(Vec<String>, u32)> = Vec::new();
    for (path, off) in js::chains(src) {
        if server.contains(&path[0]) {
            clash(&path[0])?;
            used.push((sent(&path), script_at(off).0));
        }
    }
    if let Some(p) = &rune {
        let line = script_at(p.span.0).0;
        used.extend(p.props.iter().map(|x| (vec![x.name.clone()], line)));
        if p.rest.is_some() && server.iter().any(|s| s == REST) {
            used.push((vec![REST.into()], line));
        }
    }
    if cx.load.is_some() && server_load {
        used.push((
            vec!["data".into()],
            tt.script.as_ref().map_or(1, |s| s.line),
        ));
    }

    let mut scopes = vec![Vec::new(); tt.groups.len()];
    rust_scopes(&tt.nodes, &mut Vec::new(), &mut scopes);
    let own = cx.comps.iter().position(|c| c.module == t.module);
    let mut groups = Vec::new();
    let mut locals = Vec::new();
    let mut uses: Vec<usize> = Vec::new();
    for (g, scope) in tt.groups.iter().zip(&scopes) {
        let mut rust: Vec<(Vec<String>, u32)> = Vec::new();
        // The names a closure over `expr` takes from the element's locals:
        // client `each` names, then Rust ones.
        let mut names = |expr: &str, line: u32| -> Result<Vec<String>, String> {
            let mut names: Vec<String> = Vec::new();
            for (path, _) in js::chains(expr) {
                let root = &path[0];
                let local = if g.locals.contains(root) {
                    true
                } else if scope.contains(root) {
                    clash(root)?;
                    rust.push((server_path(&path), line));
                    true
                } else {
                    if server.contains(root) {
                        clash(root)?;
                        used.push((sent(&path), line));
                    }
                    false
                };
                if local && !names.contains(root) {
                    names.push(root.clone());
                }
            }
            Ok(names)
        };
        let mut bindings = Vec::new();
        for d in &g.directives {
            if d.kind == Dir::Comp {
                let (b, ci) = comp_binding(d, &mut names, cx).map_err(|m| at(d.line, d.col, m))?;
                // Outside any block that could stop it, it would never end.
                if Some(ci) == own && !g.nested {
                    return Err(at(
                        d.line,
                        d.col,
                        format!(
                            "<{0}> renders itself here with nothing to stop it; put it in a {{:#if}} or {{:#each}} that ends, \
                         such as {{:#each node.children as child}}<{0} node={{:child}} />{{/each}}",
                            d.name
                        ),
                    ));
                }
                if !uses.contains(&ci) && Some(ci) != own {
                    uses.push(ci);
                }
                bindings.push(b);
                continue;
            }
            bindings.push(binding(d, &mut names)?);
        }
        groups.push(bindings);
        locals.push(if rust.is_empty() {
            Vec::new()
        } else {
            json_tree(&rust)
        });
    }
    let imports: Vec<String> = uses.iter().map(|&ci| comp_placeholder(ci)).collect();

    // The server values or props the module takes, each with the variable
    // it is read as (`$props()` may rename one), and `...rest`'s.
    let local = |p: &str| {
        rune.as_ref()
            .and_then(|r| r.props.iter().find(|x| x.name == p))
            .map_or_else(|| p.to_string(), |x| x.local.clone())
    };
    let mut params: Vec<(String, String)> = Vec::new();
    let mut take = |name: &str| {
        if name != REST && !js::is_reserved(name) && !params.iter().any(|p| p.0 == name) {
            params.push((name.to_string(), local(name)));
        }
    };
    for (path, _) in &used {
        take(&path[0]);
    }
    // A component the browser renders takes every prop, from the page's
    // code; a `+page.js` hands the page its `data`.
    if cx.as_client {
        for name in &server {
            take(name);
        }
    }
    if cx.load.is_some() {
        take("data");
    }
    let rest = rune.as_ref().and_then(|r| r.rest.clone());
    // What the module runs: the script with its state as signals, and the
    // directives reading them as it does.
    let owned: Vec<String> = params
        .iter()
        .map(|p| p.1.clone())
        .chain(rest.clone())
        .collect();
    // `bind:value="q"` with no `q` anywhere declares it: `let q` at the end
    // of the script (or as the whole script), so offsets in it hold. So
    // does a handler that only toggles (`open = !open`: false) or counts
    // (`n++`: 0) a name nothing else has.
    let mut src = src.to_string();
    let mut bound: Vec<&str> = Vec::new();
    let mut started: Vec<(String, String)> = Vec::new();
    for (g, scope) in tt.groups.iter().zip(&scopes) {
        for d in g
            .directives
            .iter()
            .filter(|d| matches!(d.kind, Dir::Bind | Dir::On))
        {
            let Some(v) = d.value.as_ref().map(|c| c.src.trim()) else {
                continue;
            };
            let (v, start) = if d.kind == Dir::On {
                let Some(state) = state_of(v) else { continue };
                state
            } else {
                (v, "")
            };
            let free = ty::is_ident(v)
                && !js::is_reserved(v)
                && !bound.contains(&v)
                && !declared.iter().any(|(n, _)| n == v)
                && !imported.iter().any(|n| n == v)
                && !server.iter().any(|n| n == v)
                && !owned.iter().any(|n| n == v)
                && !g.locals.iter().any(|n| n == v)
                && !scope.iter().any(|n| n == v);
            if free {
                bound.push(v);
                src.push_str("\nlet ");
                if !start.is_empty() {
                    started.push((v.to_string(), start[3..].to_string()));
                }
                src.push_str(v);
                src.push_str(start);
            }
        }
    }
    let src = src.as_str();
    let written: Vec<&str> = groups.iter().flatten().map(String::as_str).collect();
    let (runs, reactive) = js::script(src, &owned, cx.release, &written).map_err(script_err)?;
    for (bindings, g) in groups.iter_mut().zip(&tt.groups) {
        for b in bindings.iter_mut() {
            *b = js::rewrite(b, &reactive)
                .map_err(|(_, msg)| format!("{}:{}: {msg}", t.rel, g.line))?;
        }
    }
    // The snapshot as the script reads it (a `let` is a signal).
    let snap = snap
        .then(|| js::rewrite("snapshot", &reactive))
        .transpose()
        .map_err(script_err)?;
    // A dev module hands the devtools its file, its state's signals and
    // the line each is declared on.
    let dev = (!cx.release).then(|| {
        let names: Vec<&str> = (reactive.state.iter())
            .filter(|n| !owned.contains(n))
            .map(String::as_str)
            .collect();
        let line = |n: &str| {
            (declared.iter())
                .find(|(d, _)| d == n)
                .map_or(1, |&(_, off)| script_at(off).0)
        };
        let lines: Vec<String> = names.iter().map(|n| format!("{n}: {}", line(n))).collect();
        format!(
            "globalThis.__wisp_dev?.state({}, {{ {} }}, {{ {} }});\n",
            js_str(&t.rel),
            names.join(", "),
            lines.join(", ")
        )
    });
    // And the line of its first top-level statement that may not be run
    // twice, if any: `wisp dev` swaps such a module whole.
    let effect = (!cx.release)
        .then(|| js::top_effect(src))
        .flatten()
        .map(|off| script_at(off).0);
    let defaults: Vec<(String, String)> = rune
        .iter()
        .flat_map(|p| &p.props)
        .filter_map(|x| Some((x.name.clone(), x.default.clone()?)))
        .collect();
    // The `#[remote]` functions it calls, by a name it does not have.
    let called: Vec<&str> = {
        let mut words: Vec<&str> = Vec::new();
        for c in std::iter::once(runs.as_str()).chain(groups.iter().flatten().map(String::as_str)) {
            for tok in js::tokens(c)
                .iter()
                .filter(|t| !t.member && t.kind == JsKind::Ident)
            {
                words.push(tok.text(c));
            }
        }
        (cx.remotes.iter().map(String::as_str))
            .filter(|n| {
                words.contains(n)
                    && !declared.iter().any(|(d, _)| d == n)
                    && !imported.iter().any(|i| i == n)
                    && !owned.iter().any(|o| o == n)
            })
            .collect()
    };
    let id = format!("t{}", t.id);
    let base = src_dir(&t.rel);
    let html = cx.as_client.then(|| {
        let mut s = String::new();
        client_html(&tt.nodes, tt, &mut s);
        s
    });
    // The MORE names its code uses (a `use:` directive's name is code too)
    // and does not declare itself (`let optimistic = []` is the page's own).
    let codes: Vec<&str> = std::iter::once(runs.as_str())
        .chain(groups.iter().flatten().map(String::as_str))
        .collect();
    let more: String = MORE
        .iter()
        .filter(|n| {
            let (mut used, mut own) = (false, false);
            for c in &codes {
                let t = js::tokens(c);
                for (i, tok) in t.iter().enumerate() {
                    if tok.member || tok.text(c) != **n {
                        continue;
                    }
                    used = true;
                    own |= i > 0
                        && matches!(
                            t[i - 1].text(c),
                            "let" | "const" | "var" | "function" | "class"
                        );
                }
            }
            used && !own
        })
        .map(|n| format!(", {n}"))
        .collect();
    let m = Module {
        more: &more,
        more_url: (!more.is_empty()).then_some(cx.more),
        id: &id,
        params: &params,
        rest: rest.as_deref(),
        defaults: &defaults,
        script: (script.is_some() || !bound.is_empty()).then(|| {
            let (line, col) = script.map_or((1, 1), |s| (s.line, s.col));
            let own = script.map_or(0, |s| s.src.matches('\n').count() + 1);
            (runs.as_str(), line, col, own)
        }),
        groups: &groups,
        group_lines: &tt.groups.iter().map(|g| g.line).collect::<Vec<_>>(),
        imports: &imports,
        remote: (!called.is_empty())
            .then_some(cx.specs.remote.as_deref())
            .flatten()
            .map(|url| (url, called.as_slice())),
        load: cx.load.as_deref(),
        extra: (snap.is_some()
            || tt.groups.iter().flat_map(|g| &g.directives).any(is_extra)
            || std::iter::once(runs.as_str())
                .chain(groups.iter().flatten().map(String::as_str))
                .any(|c| {
                    js::tokens(c).iter().any(|t| {
                        !t.member
                            && matches!(
                                t.text(c),
                                "Map"
                                    | "Set"
                                    | "enhance"
                                    | "__wisp_snap"
                                    | "persisted"
                                    | "tweened"
                                    | "spring"
                                    | "crossfade"
                            )
                    })
                }))
        .then_some(cx.extra),
        html: html.as_deref(),
        specs: cx.specs,
        base: &base,
        dev: dev.as_deref(),
        file: (!cx.release).then_some(t.rel.as_str()),
        effect,
        snap: snap.as_deref(),
    };
    let (source, lines) = module_source(&m).map_err(|e| format!("{}: {e}", t.rel))?;
    // At the line of the file the module's line came from.
    let at = |source: &str, (off, msg): (usize, String)| {
        let k = source[..off].matches('\n').count();
        match lines.get(k).copied().flatten() {
            Some((line, _)) => format!("{}:{}: {msg}", t.rel, line + 1),
            None => format!("{}: {msg}", t.rel),
        }
    };
    let mut source = js::public_env(&source, &|n| var(cx.env, n)).map_err(|e| at(&source, e))?;
    let mut texts = Vec::new();
    if let Some(l) = cx.i18n {
        let args = |k: &str| l.args(k).ok_or_else(|| l.unknown(k));
        let (code, keys) = js::translate(&source, &args).map_err(|e| at(&source, e))?;
        texts = keys.iter().filter_map(|k| l.key(k)).collect();
        source = code;
    }
    if cx.maps {
        source.push_str(&sourcemap::comment(&format!("{id}.js")));
    } else {
        let _ = writeln!(source, "//# sourceURL=wisp:///{}", t.rel);
    }
    let hash = image::hash(source.as_bytes());
    let blob = if used.is_empty() {
        vec![Piece::Text("{}".into())]
    } else {
        json_tree(&used)
    };
    // What the first paint may read: a `+page.js` changes `data` in the
    // browser, so the server does not know it.
    let mut known = server;
    known.retain(|n| !(cx.load.is_some() && n == "data"));
    let mut lets: Vec<(String, String)> = declared
        .iter()
        .filter_map(|(n, _)| {
            Some((
                n.clone(),
                js::plain_init(js::initializer(src, n)?)?.to_string(),
            ))
        })
        .collect();
    // A prop `$props()` renames is its prop, to the first paint.
    let mut declared: Vec<String> = declared.into_iter().map(|(n, _)| n).collect();
    declared.extend(started.iter().map(|(n, _)| n.clone()));
    lets.extend(started);
    for p in rune
        .iter()
        .flat_map(|r| &r.props)
        .filter(|p| p.local != p.name)
    {
        lets.push((p.local.clone(), p.name.clone()));
        declared.push(p.local.clone());
    }
    if let Some(r) = &rest {
        lets.push((r.clone(), REST.into()));
        declared.push(r.clone());
    }
    Ok(Some(Client {
        preload: Vec::new(),
        id,
        source,
        lines,
        hash,
        texts,
        blob,
        locals,
        uses,
        paints: cx.as_client,
        server: known,
        whole: t.kind == Kind::Component,
        declared,
        lets,
        scopes,
    }))
}

/// The module of the `#[remote]` functions: each a `fetch` of its path,
/// its arguments as a JSON object by name (a GET's in the query, each as
/// JSON). It answers the value, `undefined` for a 204, or throws an
/// `Error` with the `status` (and a 422's `errors`); a redirect is
/// followed, as wisp.js follows a form's.
pub(super) fn remote_js(remotes: &[RemoteFn]) -> String {
    let mut s = String::from(
        "// The app's #[remote] functions, written by wisp-build.
const call = async (path, get, names, args) => {
  const a = {};
  names.forEach((n, i) => { if (args[i] !== undefined) a[n] = args[i]; });
  const headers = { 'x-wisp': '1', accept: 'application/json' };
  const r = get
    ? await fetch(path + '?' + new URLSearchParams(Object.keys(a).map((k) => [k, JSON.stringify(a[k])])), { headers })
    : await fetch(path, { method: 'POST', headers: { ...headers, 'content-type': 'application/json' }, body: JSON.stringify(a) });
  const to = r.headers.get('x-wisp-location');
  if (to) {
    return new Promise((done) => document.dispatchEvent(new CustomEvent('wisp:goto', { detail: { url: to, done: () => done() } })));
  }
  const text = await r.text();
  let v = text;
  if (text && (r.headers.get('content-type') || '').includes('json')) v = JSON.parse(text);
  if (!r.ok) throw Object.assign(new Error((v && v.error) || r.statusText), { status: r.status, errors: v && v.errors });
  return text ? v : undefined;
};
",
    );
    for r in remotes {
        let names: Vec<String> = (r.f.inputs().unwrap_or_default().iter())
            .map(|(n, _)| js_str(n))
            .collect();
        let _ = writeln!(
            s,
            "export const {} = (...a) => call({}, {}, [{}], a);",
            r.f.name,
            js_str(&r.path()),
            u8::from(r.get()),
            names.join(", ")
        );
    }
    s
}

/// How a custom element reads an attribute of a prop of Rust type `ty`
/// (element.js): `n` a number, `b` a bool, `s` text, `j` JSON. An
/// `Option` is its inner type's.
pub(super) fn element_kind(ty: &str) -> &'static str {
    let ty = ty
        .trim()
        .trim_start_matches('&')
        .trim_start_matches("'static ");
    let ty = (ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>'))).unwrap_or(ty);
    let ty = ty.trim().trim_start_matches('&').trim();
    match ty {
        "bool" => "b",
        "str" | "String" | "char" | "Cow<str>" | "Cow<'static, str>" => "s",
        "f32" | "f64" => "n",
        t if ty::int_range(t).is_some() => "n",
        _ => "j",
    }
}

/// A prop's Rust default as the JSON a custom element starts from: a
/// literal (`fold::literal`), `None`, or an empty `String` or `Vec`.
pub(super) fn element_default(src: &str) -> Option<String> {
    let src = src.trim();
    if let Some(l) = fold::literal(src) {
        return Some(match l {
            fold::Lit::Str(s) => js_str(&s),
            fold::Lit::Int(n) => n.to_string(),
            fold::Lit::Bool(b) => b.to_string(),
        });
    }
    match src {
        "None" => Some("null".into()),
        "String::new()" | "\"\".into()" | "\"\".to_string()" => Some("\"\"".into()),
        "Vec::new()" | "vec![]" => Some("[]".into()),
        _ => src
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && src.contains('.'))
            .map(|_| src.to_string()),
    }
}

/// Where a module's import of component `ci`'s module goes, until the
/// URLs are known (see `generate`).
pub(super) fn comp_placeholder(ci: usize) -> String {
    format!("@wisp/comp/{ci}")
}

/// `src` with the placeholder of each component `url` knows (`"@wisp/comp/3"`)
/// replaced by its module's URL, in one pass.
pub(super) fn link_comps(src: &str, url: impl Fn(usize) -> Option<String>) -> String {
    const MARK: &str = "\"@wisp/comp/";
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find(MARK) {
        let after = &rest[i + MARK.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let found = after[..digits].parse().ok().and_then(&url);
        match found.filter(|_| after[digits..].starts_with('"')) {
            Some(u) => {
                out.push_str(&rest[..i]);
                out.push_str(&js_str(&u));
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..i + MARK.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The markup of a component the browser renders: its text, with every
/// directive element marked by its group alone.
pub(super) fn client_html(nodes: &[Node], t: &Template, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(i) => out.push_str(&t.chunks[*i]),
            Node::Live { group } => {
                let _ = write!(out, " {GROUP_ATTR}=\"{group}\"");
            }
            Node::Client(branches) => {
                for (g, body) in branches {
                    let _ = write!(out, "<template {GROUP_ATTR}=\"{g}\">");
                    client_html(body, t, out);
                    out.push_str("</template>");
                }
            }
            Node::Render => {
                let _ = write!(out, "<template {SLOT_ATTR}></template>");
            }
            Node::Tag { .. } => out.push_str("wisp-element"),
            _ => {}
        }
    }
}

/// A module's parts, for `module_source`.
pub(super) struct Module<'a> {
    pub(super) id: &'a str,
    /// The server values or props, and the variable each is read as.
    pub(super) params: &'a [(String, String)],
    /// `...rest`'s variable: the props not named.
    pub(super) rest: Option<&'a str>,
    /// `$props()` defaults: a prop's JavaScript when it is not given.
    pub(super) defaults: &'a [(String, String)],
    /// The script, as it runs, the line and column of the file it starts
    /// on, and how many of its lines are the file's (`bind:` may add some).
    pub(super) script: Option<(&'a str, u32, u32, usize)>,
    pub(super) groups: &'a [Vec<String>],
    /// The line of each group's element.
    pub(super) group_lines: &'a [u32],
    /// Modules of the components it renders.
    pub(super) imports: &'a [String],
    /// The `#[remote]` functions' module and the ones it calls.
    pub(super) remote: Option<(&'a str, &'a [&'a str])>,
    pub(super) load: Option<&'a str>,
    /// The `MORE` helpers it uses, each as `, name`.
    pub(super) more: &'a str,
    /// `more.js`'s URL, when it uses one.
    pub(super) more_url: Option<&'a str>,
    /// `extra.js`'s URL, when the module uses it.
    pub(super) extra: Option<&'a str>,
    pub(super) html: Option<&'a str>,
    pub(super) specs: &'a Specs,
    /// Its file's directory under `src`, which a relative import is from.
    pub(super) base: &'a str,
    /// A dev build's call that tells the devtools about the instance.
    pub(super) dev: Option<&'a str>,
    /// A dev build's: the file, by which `wisp dev` swaps the module in
    /// place, and the line of a top-level statement that keeps it from that.
    pub(super) file: Option<&'a str>,
    pub(super) effect: Option<u32>,
    /// `export const snapshot`, as the script reads it.
    pub(super) snap: Option<&'a str>,
}

/// The module's text, and where each of its lines came from in the file
/// (for its source map). The script keeps its line numbers too, as far as
/// the lines before it allow.
///
/// The script runs in blocks of its own, inside the helpers and then the
/// server values (signals, which the runtime sets again when a morph or a
/// parent brings new ones), so it may reuse a helper's name and a server
/// value may too. Its function returns the binding groups.
pub(super) fn module_source(m: &Module) -> Result<(String, Vec<sourcemap::Line>), String> {
    let mut s = String::new();
    // Each line of `s` so far, and the length of `s` it has counted.
    let mut map: (Vec<sourcemap::Line>, usize) = (Vec::new(), 0);
    // The lines of `s` added since came from `at(k)`, the k-th of them.
    let upto =
        |s: &str, map: &mut (Vec<sourcemap::Line>, usize), at: &dyn Fn(u32) -> sourcemap::Line| {
            let new = s[map.1..].matches('\n').count() as u32;
            map.0.extend((0..new).map(at));
            map.1 = s.len();
        };
    // Line `k` of the script (0-based), as a line and column of the file.
    let script_line = |k: u32| {
        let (_, line, col, own) = m.script?;
        ((k as usize) < own).then(|| (line - 1 + k, if k == 0 { col - 1 } else { 0 }))
    };
    let _ = writeln!(
        s,
        "import {{ define }} from \"{LIVE_JS_PATH}?v={}\";",
        crate::runtime_version()
    );
    for url in m.imports {
        let _ = writeln!(s, "import {};", js_str(url));
    }
    if let Some((url, names)) = m.remote {
        let _ = writeln!(s, "import {{ {} }} from {};", names.join(", "), js_str(url));
    }
    if let Some(url) = m.load {
        let _ = writeln!(s, "import * as __wisp_u from {};", js_str(url));
    }
    for url in m.extra.iter().chain(&m.more_url) {
        let _ = writeln!(s, "import {};", js_str(url));
    }
    upto(&s, &mut map, &|_| None);
    let mut body = String::new();
    if let Some((src, ..)) = m.script {
        // Imports go first, as a module's must; they leave blank lines.
        let spans = js::imports(src);
        for &(a, b) in &spans {
            s.push_str(&rewrite_specifiers(&src[a..b], m.specs, Some(m.base))?);
            s.push('\n');
            let first = src[..a].matches('\n').count() as u32;
            upto(&s, &mut map, &|k| script_line(first + k));
        }
        // `import('…')` stays where it is, resolved: it loads on demand.
        body = rewrite_specifiers(&js::blank(src, &spans), m.specs, Some(m.base))?;
    }
    let _ = write!(
        s,
        "define({}, function (__wisp_p, __wisp_h) {{ const {{ {HELPERS}{} }} = __wisp_h; {{ ",
        js_str(m.id),
        m.more
    );
    if !m.params.is_empty() || m.rest.is_some() {
        let list: Vec<String> = m.params.iter().map(|p| js_str(&p.0)).collect();
        let mut names: Vec<String> = m
            .params
            .iter()
            .map(|(p, l)| {
                if p == l {
                    p.clone()
                } else {
                    format!("{p}: {l}")
                }
            })
            .collect();
        names.extend(m.rest.map(|r| format!("{REST}: {r}")));
        let defaults: Vec<String> = m
            .defaults
            .iter()
            .map(|(n, d)| format!("{n}: () => ({d})"))
            .collect();
        let tail = match (defaults.is_empty(), m.rest.is_some()) {
            (true, false) => String::new(),
            (_, rest) => format!(
                ", {{ {} }}{}",
                defaults.join(", "),
                if rest { ", 1" } else { "" }
            ),
        };
        let _ = write!(
            s,
            "const {{ {} }} = __wisp_props(__wisp_p, [{}]{tail}); ",
            names.join(", "),
            list.join(", "),
        );
    }
    s.push_str("{\n");
    if let Some((_, line, ..)) = m.script {
        let next = s.matches('\n').count() as u32 + 1;
        for _ in next..line {
            s.push('\n');
        }
        upto(&s, &mut map, &|_| None);
        s.push_str(&body);
        if !body.ends_with('\n') {
            s.push('\n');
        }
        upto(&s, &mut map, &script_line);
    }
    if let Some(d) = m.dev {
        s.push_str(d);
    }
    s.push_str("return { g: [\n");
    upto(&s, &mut map, &|_| None);
    for (g, &line) in m.groups.iter().zip(m.group_lines) {
        let g = rewrite_specifiers(&g.join(", "), m.specs, Some(m.base))?;
        let _ = writeln!(s, "  [{g}],");
        upto(&s, &mut map, &|k| Some((line - 1 + k, 0)));
    }
    s.push(']');
    if let Some(x) = m.snap {
        let _ = write!(s, ", snap: {x}");
    }
    s.push_str(" };\n} } }");
    let mut opts = Vec::new();
    if let Some(h) = m.html {
        opts.push(format!("html: {}", js_str(h)));
    }
    if m.load.is_some() {
        opts.push("load: __wisp_u.load".into());
    }
    if let Some(f) = m.file {
        opts.push(format!("file: {}", js_str(f)));
    }
    if let Some(l) = m.effect {
        opts.push(format!("effect: {l}"));
    }
    if !opts.is_empty() {
        let _ = write!(s, ", {{ {} }}", opts.join(", "));
    }
    s.push_str(");\n");
    upto(&s, &mut map, &|_| None);
    Ok((s, map.0))
}

/// The source map of the module served at `path`, named `name` there,
/// whose lines came from the file `rel` as `lines` says.
pub(super) fn map_file(
    path: &str,
    name: &str,
    rel: &str,
    src: &str,
    lines: &[sourcemap::Line],
) -> JsFile {
    let source = sourcemap::encode(name, &format!("wisp:///{rel}"), src, lines);
    JsFile {
        path: format!("{path}.map"),
        hash: image::hash(source.as_bytes()),
        source,
        file: None,
    }
}

/// `src` as JavaScript: a `.ts` file's types blanked out (see
/// `js::strip_types`), so its lines and columns stay.
pub(super) fn javascript(src: &str, rel: &str) -> Result<String, String> {
    if !rel.ends_with(".ts") {
        return Ok(src.to_string());
    }
    js::strip_types(src).map_err(|(off, msg)| format!("{rel}:{}: {msg}", place(src, off)))
}

/// Offset `off` of `src` as `line:col`.
pub(super) fn place(src: &str, off: usize) -> String {
    let before = &src[..off];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{line}:{col}")
}

/// Variable `name` of `vars`.
pub(super) fn var(vars: &[(String, String)], name: &str) -> Option<String> {
    vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

/// What an import's module name resolves against: `src/lib`'s files (all
/// under one hash) and the app's npm packages.
pub(super) struct Specs {
    /// The `#[remote]` functions' module, `wisp:remote`, if there are any.
    pub(super) remote: Option<String>,
    /// `src/lib`'s files, as `x.js` or `dir/y.ts`.
    pub(super) lib: Vec<String>,
    pub(super) lib_hash: String,
    pub(super) npm: Npm,
}

/// The URL the browser loads for `spec` in an import, static or `import()`:
/// `wisp` is the runtime, `wisp:remote` the `#[remote]` functions,
/// `$lib/x.js` is `src/lib/x.js`, and so is a relative path from a file
/// whose directory under `src` is `base` (`lib/sub`, `routes/blog`); a
/// package name is that npm package. Anything else (a full URL) stays as
/// written. A path to no file of `src/lib` is an error.
pub(super) fn resolve_spec(
    spec: &str,
    cx: &Specs,
    base: Option<&str>,
) -> Result<Option<String>, String> {
    if spec == "wisp" {
        let v = crate::runtime_version();
        return Ok(Some(format!("{LIVE_JS_PATH}?v={v}")));
    }
    if spec == "wisp:remote" {
        return match &cx.remote {
            Some(url) => Ok(Some(url.clone())),
            None => Err(
                "`wisp:remote` has the app's #[remote] functions, and it has none: mark one in a page's block or src/remote.rs".into(),
            ),
        };
    }
    if npm::is_bare(spec) {
        return cx.npm.url(spec).map(Some);
    }
    let path = if let Some(p) = spec.strip_prefix("$lib/") {
        format!("lib/{p}")
    } else {
        let Some(b) = base.filter(|_| spec.starts_with("./") || spec.starts_with("../")) else {
            return Ok(None);
        };
        let mut parts: Vec<&str> = b.split('/').filter(|p| !p.is_empty()).collect();
        for p in spec.split('/') {
            match p {
                "." | "" => {}
                ".." if parts.pop().is_none() => {
                    return Err(format!("`{spec}` is outside src"));
                }
                ".." => {}
                p => parts.push(p),
            }
        }
        parts.join("/")
    };
    let Some(rel) = path.strip_prefix("lib/") else {
        return Err(format!(
            "`{spec}` is src/{path}; the browser loads only src/lib's files, so move it there and import it as `$lib/…`"
        ));
    };
    // `$lib/x` is `x.js` or `x.ts`, whichever there is; `x.js` may be
    // `x.ts`, as TypeScript writes it.
    let ext = rel.rsplit('/').next().unwrap_or("").contains('.');
    let ts = rel.strip_suffix(".js").map(|r| format!("{r}.ts"));
    let found = match ext {
        false => (["js", "ts"].iter())
            .map(|e| format!("{rel}.{e}"))
            .find(|f| cx.lib.contains(f)),
        true => std::iter::once(rel.to_string())
            .chain(ts)
            .find(|f| cx.lib.contains(f)),
    };
    let Some(rel) = found else {
        return Err(format!("`{spec}`: there is no src/{path}"));
    };
    Ok(Some(format!("{MODULES}lib/{rel}?v={}", cx.lib_hash)))
}

/// The directory under `src` of the file at `rel` (from the project root):
/// `routes/blog` for `src/routes/blog/+page.wisp`.
pub(super) fn src_dir(rel: &str) -> String {
    let rel = rel.strip_prefix("src/").unwrap_or(rel);
    rel.rfind('/').map_or("", |i| &rel[..i]).to_string()
}

/// `src` with the module names of its imports replaced as `resolve_spec`
/// says. Every file reaches a lib file by the same URL, so a store in it
/// is one store.
pub(super) fn rewrite_specifiers(
    src: &str,
    cx: &Specs,
    base: Option<&str>,
) -> Result<String, String> {
    if !src.contains("import") && !src.contains("from") {
        return Ok(src.to_string());
    }
    js::specifiers(src, |spec| resolve_spec(spec, cx, base))
}

/// `&["/_app/c/lib/x.js?v=…", …]`: a module's preloads, as Rust.
pub(super) fn preload_list(urls: &[String]) -> String {
    let all: Vec<String> = urls.iter().map(|u| lit(u)).collect();
    format!("&[{}]", all.join(", "))
}

/// What a module of `source` imports statically, all the way down, by
/// URL, in the order found: through the other modules served (`sources`,
/// by URL), not into `import()`, and never the runtime, which the page
/// loads itself. Code that two pages import is one module of one URL,
/// which each page preloads and the browser fetches once.
pub(super) fn static_imports(source: &str, sources: &[(String, &str)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![source];
    while let Some(src) = stack.pop() {
        for url in js::static_specs(src) {
            if url.starts_with(LIVE_JS_PATH) || out.contains(&url) {
                continue;
            }
            if let Some((_, next)) = sources.iter().find(|(u, _)| *u == url) {
                stack.push(next);
            }
            out.push(url);
        }
    }
    out
}

/// Resolves the names an expression (with the line it is on) reads: the
/// element's locals, which its closure destructures, are returned.
pub(super) type Names<'a> = dyn FnMut(&str, u32) -> Result<Vec<String>, String> + 'a;

/// An `on:` directive's modifiers as live.js takes them: bits, in the order
/// of `ON_FLAGS`, and `ON_ROOT` for an event handled by one listener at the root
/// (one that bubbles, and need not be where it is); then, when there are,
/// the keys (`KeyboardEvent.key` in lower case) and the debounce time in ms.
pub(super) fn on_mods(event: &str, mods: &[String]) -> (u32, String) {
    const ROOTED: [&str; 14] = [
        "click",
        "dblclick",
        "input",
        "change",
        "keydown",
        "keyup",
        "pointerdown",
        "pointerup",
        "pointermove",
        "mousedown",
        "mouseup",
        "contextmenu",
        "focusin",
        "focusout",
    ];
    let (mut bits, mut keys, mut ms) = (0u32, Vec::new(), 0u32);
    let mut k = 0;
    while k < mods.len() {
        let m = mods[k].as_str();
        if let Some(i) = ON_FLAGS.iter().position(|f| *f == m) {
            bits |= 1 << i;
        } else if m == "debounce" {
            ms = 250;
            let time = mods.get(k + 1).and_then(|t| match t.strip_suffix("ms") {
                Some(n) => n.parse::<u32>().ok(),
                None => t.strip_suffix('s')?.parse::<u32>().ok().map(|n| n * 1000),
            });
            if let Some(t) = time {
                ms = t;
                k += 1;
            }
        } else {
            keys.push(js_str(match m {
                "space" => " ",
                "up" => "arrowup",
                "down" => "arrowdown",
                "left" => "arrowleft",
                "right" => "arrowright",
                key => key,
            }));
        }
        k += 1;
    }
    // capture, passive, window, document, outside: where it is.
    if ROOTED.contains(&event) && bits & ON_PLACED == 0 {
        bits |= ON_ROOT;
    }
    let rest = match (keys.is_empty(), ms) {
        (true, 0) => String::new(),
        (true, ms) => format!(", null, {ms}"),
        (false, 0) => format!(", [{}]", keys.join(", ")),
        (false, ms) => format!(", [{}], {ms}", keys.join(", ")),
    };
    (bits, rest)
}

/// `(L) =>` and `(L, x) =>`, destructuring what the expression reads.
pub(super) fn one(n: &[String]) -> String {
    if n.is_empty() {
        "()".to_string()
    } else {
        format!("({{ {} }})", n.join(", "))
    }
}

pub(super) fn two(n: &[String], x: &str) -> String {
    if n.is_empty() {
        format!("(_, {x})")
    } else {
        format!("({{ {} }}, {x})", n.join(", "))
    }
}

/// An expression as an arrow's body. The newline keeps a trailing `//`
/// comment from swallowing the `)`.
pub(super) fn paren(e: &str) -> String {
    format!(
        "({e}{})",
        if js::ends_in_line_comment(e) {
            "\n"
        } else {
            ""
        }
    )
}

/// An event handler's body: a plain path is called with the event, one
/// expression is returned (so a promise is seen), and statements are a block.
pub(super) fn handler_body(src: &str) -> String {
    if js::is_path(src) {
        format!("{src}(event)")
    } else if js::is_statements(src) {
        format!(
            "{{ {src}{} }}",
            if js::ends_in_line_comment(src) {
                "\n"
            } else {
                ""
            }
        )
    } else {
        paren(src)
    }
}

/// One directive as the runtime takes it (see the contract in `live.js`).
/// `names` resolves an expression's locals, which its closure destructures.
pub(super) fn binding(d: &Directive, names: &mut Names) -> Result<String, String> {
    let getter = |c: &Code, names: &mut Names| -> Result<String, String> {
        Ok(format!(
            "{} => {}",
            one(&names(&c.src, c.line)?),
            paren(&c.src)
        ))
    };
    let optional = |c: Option<&Code>, names: &mut Names| -> Result<String, String> {
        match c {
            Some(c) => getter(c, names),
            None => Ok("null".into()),
        }
    };
    let value = || {
        d.value
            .as_ref()
            .expect("the parser checked that it has a value")
    };
    let name = js_str(&d.name);
    Ok(match d.kind {
        Dir::On => {
            let c = value();
            let n = names(&c.src, c.line)?;
            let (bits, rest) = on_mods(&d.name, &d.mods);
            format!(
                "[\"on\", {name}, {bits}, {} => {}{rest}]",
                two(&n, "event"),
                handler_body(&c.src)
            )
        }
        Dir::Bind => {
            let c = value();
            let n = names(&c.src, c.line)?;
            let get = if d.name == "this" {
                "null".to_string()
            } else {
                format!("{} => {}", one(&n), paren(&c.src))
            };
            format!(
                "[\"bind\", {name}, {get}, {} => {{ {} = __wisp_v }}]",
                two(&n, "__wisp_v"),
                paren(&c.src)
            )
        }
        Dir::Attr => format!("[\"attr\", {name}, {}]", getter(value(), names)?),
        Dir::Text => format!("[\"text\", {}]", getter(value(), names)?),
        Dir::Hole if d.name == "html" => format!("[\"html\", {}]", getter(value(), names)?),
        // The prop's snippet, and the array of the arguments to draw it with.
        // The prop is the browser's own: the server sends no snippet.
        Dir::Hole if d.name == "draw" => format!(
            "[\"draw\", {} => {}, {}]",
            one(&[]),
            paren(&value().src),
            optional(d.key.as_ref(), names)?
        ),
        Dir::Snip => format!(
            "[\"snip\", {name}, [{}]]",
            d.mods
                .iter()
                .map(|m| js_str(m))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Dir::Hole => format!("[\"hole\", {}]", getter(value(), names)?),
        Dir::Class => format!("[\"class\", {name}, {}]", getter(value(), names)?),
        Dir::Style => format!("[\"style\", {name}, {}]", getter(value(), names)?),
        // A built-in by its name, else the script's function; then 0 both
        // ways, 1 in, 2 out.
        Dir::Transition => {
            let kind = if matches!(d.name.as_str(), "fade" | "slide" | "scale" | "fly" | "blur") {
                name
            } else {
                format!("{} => {}", one(&names(&d.name, d.line)?), d.name)
            };
            let dir = match d.mods.first().map(String::as_str) {
                Some("in") => 1,
                Some("out") => 2,
                _ => 0,
            };
            format!(
                "[\"transition\", {kind}, {}, {dir}]",
                optional(d.value.as_ref(), names)?
            )
        }
        Dir::Animate => format!(
            "[\"animate\", {name}, {}]",
            optional(d.value.as_ref(), names)?
        ),
        Dir::Use => {
            let f = format!("{} => {}", one(&names(&d.name, d.line)?), d.name);
            // use:modal="open" with `open` a variable (or a field of one):
            // `[open, set]`, so every close of the dialog sets it false.
            let path = |s: &str| {
                !s.is_empty()
                    && s.split('.').all(|p| {
                        p.chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                            && p.chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                    })
            };
            if d.name == "modal" && d.value.as_ref().is_some_and(|v| path(v.src.trim())) {
                let c = value();
                let n = names(&c.src, c.line)?;
                let v = c.src.trim();
                return Ok(format!(
                    "[\"use\", {f}, {} => [{v}, __wisp_v => {{ {v} = __wisp_v }}]]",
                    one(&n)
                ));
            }
            format!("[\"use\", {f}, {}]", optional(d.value.as_ref(), names)?)
        }
        Dir::Each => {
            let own: Vec<String> = std::iter::once(&d.name).chain(&d.mods).cloned().collect();
            let key = match &d.key {
                Some(k) => {
                    let mut n = own.clone();
                    for x in names(&k.src, k.line)? {
                        if !n.contains(&x) {
                            n.push(x);
                        }
                    }
                    format!("{} => {}", one(&n), paren(&k.src))
                }
                None => "null".into(),
            };
            let own: Vec<String> = own.iter().map(|n| js_str(n)).collect();
            format!(
                "[\"each\", {}, [{}], {key}]",
                getter(value(), names)?,
                own.join(", ")
            )
        }
        Dir::If => format!("[\"if\", {}]", getter(value(), names)?),
        Dir::Key => format!("[\"key\", {}]", getter(value(), names)?),
        Dir::Await => format!("[\"await\", {}]", getter(value(), names)?),
        Dir::Try => "[\"try\"]".into(),
        Dir::Spread => format!("[\"spread\", {}]", getter(value(), names)?),
        Dir::Tag => format!("[\"tag\", {}]", getter(value(), names)?),
        Dir::At | Dir::Wait => format!(
            "[\"{}\", {name}]",
            if d.kind == Dir::At { "at" } else { "wait" }
        ),
        Dir::Comp => unreachable!("comp_binding builds it"),
    })
}

/// `<Card title={:x} bind:open="o" on:select="pick">` where the browser
/// renders it: `["comp", ID, (L) => props, binds, events]`, and the
/// component's index, whose module the page's module imports.
pub(super) fn comp_binding(
    d: &Directive,
    names: &mut Names,
    cx: &ClientCx,
) -> Result<(String, usize), String> {
    let name = &d.name;
    let Some(ci) = cx.comps.iter().position(|c| c.name == *name) else {
        let known: Vec<&str> = cx.comps.iter().map(|c| c.name.as_str()).collect();
        return Err(format!(
            "no component `{name}`: components are the .wisp files in src/components, and there are {}",
            if known.is_empty() {
                "none yet".into()
            } else {
                known.join(", ")
            }
        ));
    };
    let c = &cx.comps[ci];
    if !template::client_renderable(&cx.templates[ci].t.nodes) {
        return Err(format!(
            "<{name}> is rendered in the browser here, but its markup has server code ({{…}} or a {{#…}} block). \
             In a component the browser renders, show props with {{:prop}} and use {{:#if}} and {{:#each}}"
        ));
    }
    let mut all: Vec<String> = Vec::new();
    let mut add = |n: Vec<String>| {
        for x in n {
            if !all.contains(&x) {
                all.push(x);
            }
        }
    };
    let (mut props, mut binds, mut events) = (Vec::new(), Vec::new(), Vec::new());
    for p in &d.props {
        // `{:...obj}`: its keys, where it stands among the others.
        if let (true, PropValue::Live(code)) = (p.name == "...", &p.value) {
            add(names(&code.src, code.line)?);
            props.push(format!("...{}", paren(&code.src)));
            continue;
        }
        let key = js_str(&p.name);
        let declared = c.props.iter().any(|x| x.name == p.name);
        if !declared && !c.rest && !matches!(p.value, PropValue::On(_)) {
            let takes: Vec<&str> = c.props.iter().map(|x| x.name.as_str()).collect();
            return Err(format!(
                "<{name}> has no prop `{}`; it takes {}",
                p.name,
                if takes.is_empty() {
                    "none".into()
                } else {
                    takes.join(", ")
                }
            ));
        }
        match &p.value {
            PropValue::Text(t) => props.push(format!("{key}: {}", js_str(t))),
            PropValue::Flag => props.push(format!("{key}: true")),
            PropValue::Live(code) => {
                add(names(&code.src, code.line)?);
                props.push(format!("{key}: {}", paren(&code.src)));
            }
            PropValue::Bind(code) => {
                if c.bindable.as_ref().is_some_and(|b| !b.contains(&p.name)) {
                    return Err(format!(
                        "<{name}>'s `{0}` is not bindable: its script marks the props a parent may bind, \
                         `let {{ {0} = $bindable() }} = $props()`",
                        p.name
                    ));
                }
                let n = names(&code.src, code.line)?;
                binds.push(format!(
                    "[{key}, {} => {}, {} => {{ {} = __wisp_v }}]",
                    one(&n),
                    paren(&code.src),
                    two(&n, "__wisp_v"),
                    paren(&code.src)
                ));
                add(n);
                props.push(format!("{key}: {}", paren(&code.src)));
            }
            PropValue::On(code) => {
                let n = names(&code.src, code.line)?;
                events.push(format!(
                    "[{key}, {} => {}]",
                    two(&n, "event"),
                    handler_body(&code.src)
                ));
            }
            // Drawn from the block before the tag (`snip`), not passed.
            PropValue::Snippet { .. } => {}
            PropValue::Expr(_) => unreachable!("the parser refuses server props here"),
        }
    }
    let b = format!(
        "[\"comp\", {}, {} => ({{ {} }}), [{}], [{}]]",
        js_str(&format!("t{}", cx.templates[ci].id)),
        one(&all),
        props.join(", "),
        binds.join(", "),
        events.join(", ")
    );
    Ok((b, ci))
}
