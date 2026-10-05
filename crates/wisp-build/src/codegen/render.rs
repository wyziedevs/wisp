//! One render function per template.

use super::*;

/// The code of a template's file that names Rust (see `auto::wisp_code`).
pub(super) fn tpl_src(p: &Project, t: &Tpl) -> Result<(String, String), String> {
    let src = crate::read_source(&p.root.join(&t.rel)).map_err(|e| format!("{}: {e}", t.rel))?;
    Ok((crate::auto::wisp_code(&src), t.rel.clone()))
}

pub(super) struct Gen {
    pub(super) out: String,
    pub(super) release: bool,
    /// Dev builds: the templates' modules with a `__wisp_types`, which
    /// only `wisp check --types` compiles (`wisp::__ts!`).
    pub(super) types: Option<Vec<String>>,
    /// The users table `init` names, else the one account table of `src/db.rs`
    /// (`db::USERS`), which `cx.user()`, `login` and `signup` take.
    pub(super) users: Option<String>,
    /// There is a `src/db.rs`, whose `pub` items every route file sees.
    pub(super) db: bool,
    /// The names files may use with no `use` (see `crate::auto`).
    pub(super) auto: crate::auto::Auto,
    /// What each file got auto-imported, for `wisp check --explain-imports`.
    pub(super) imports: crate::auto::FileImports,
}

impl Gen {
    pub(super) fn line(&mut self, indent: usize, s: &str) {
        for _ in 0..indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Opens `pub mod NAME {` with an app file in it: its own `//!` docs
    /// and `#![…]` attributes first (only possible with the file written
    /// into the module), `use {glob};`, the file (included, so errors point
    /// at it, unless it has inner attributes or a `Table::saved()` to name; a `---` block's items line by
    /// line, each marked with its line), then the prelude.
    pub(super) fn user_mod(&mut self, m: &UserMod, rel: &str, glob: &str) -> Result<(), String> {
        self.line(0, &format!("pub mod {} {{", m.name));
        let src = match &m.inline {
            Some(s) => s.clone(),
            None => crate::read_source(&m.file).map_err(|e| format!("{rel}: {e}"))?,
        };
        let top = rust_scan::inner_end(&src);
        if top > 0 {
            self.out.push_str(&src[..top]);
            self.out.push('\n');
        }
        self.line(1, "#[allow(unused_imports)]");
        self.line(1, &format!("use {glob};"));
        self.db_items(glob);
        let named = rust_scan::name_saved(&src[top..]);
        let tail = named.as_deref().unwrap_or(&src[top..]);
        if m.inline.is_some() {
            let first = src[..top].matches('\n').count() + 1;
            self.rust_lines(1, tail, first, rel)?;
        } else if let Some(code) = self.bound(tail, rel)? {
            self.out.push_str(&code);
            self.out.push('\n');
        } else if top > 0 || named.is_some() {
            self.out.push_str(tail);
            self.out.push('\n');
        } else {
            self.line(1, &format!("include!({});", lit(&m.file.to_string_lossy())));
        }
        // After the file: of two imports of the same names rustc counts the
        // first as used, so a file's own `use wisp::prelude::*` is not
        // reported unused, and this one is allowed to be.
        self.line(1, "#[allow(unused_imports)]");
        self.line(1, "use ::wisp::prelude::*;");
        Ok(())
    }

    /// The `__call` module of `m`, inside its module: where the generated
    /// code calls its functions.
    pub(super) fn call_mod(&mut self, m: &UserMod, twins: &[String]) {
        self.line(1, "#[doc(hidden)]");
        self.line(1, "#[allow(unused_variables, clippy::all)]");
        self.line(1, "pub mod __call {");
        // The file's names and the prelude, for what `#[validate(…)]`
        // rules name: `max_size = 1 * MB`, `max_len = MAX`.
        self.line(2, "#[allow(unused_imports)]");
        self.line(2, "use super::*;");
        for s in m.calls() {
            for line in s.lines() {
                let twin = (line.strip_prefix("pub fn "))
                    .and_then(|l| l.split_once("_now(cx: &mut ::wisp::Cx)"));
                if let Some((name, _)) = twin
                    && !twins.contains(&format!("{}::{name}", m.name))
                {
                    continue;
                }
                self.line(2, line);
            }
        }
        self.line(1, "}");
    }

    /// Rust from a `.wisp` file, whose first line is line `first` of it,
    /// each line marked with where it is (`// file.wisp:7`) so that rustc's
    /// errors are told against the file. Blank lines are left out, and a
    /// line inside a string is written as it is.
    pub(super) fn rust_lines(
        &mut self,
        ind: usize,
        code: &str,
        first: usize,
        rel: &str,
    ) -> Result<(), String> {
        let bound = self.bound(code, rel)?;
        let code = bound.as_deref().unwrap_or(code);
        let ends = rust_scan::line_ends_in_code(code);
        let mut inside = false;
        for (k, l) in code.split('\n').enumerate() {
            let safe = ends.get(k).copied().unwrap_or(true);
            if !inside && l.trim().is_empty() {
                continue;
            }
            if inside {
                self.out.push_str(l);
            } else {
                self.out.push_str(&"    ".repeat(ind));
                self.out.push_str(l.trim_end());
            }
            if safe {
                let _ = write!(self.out, " // {rel}:{}", first + k);
            }
            self.out.push('\n');
            inside = !safe;
        }
        Ok(())
    }

    /// `use super::__mods::db::*;` for a file that sees the app's modules
    /// (`glob`): what `src/db.rs` makes `pub` needs no `db::`. Its own
    /// names win over these.
    pub(super) fn db_items(&mut self, glob: &str) {
        if self.db && glob == "super::__mods::*" {
            self.line(1, "#[allow(unused_imports)]");
            self.line(1, "use super::__mods::db::*;");
        }
    }

    /// The auto-imports of the module written since `start` (from its
    /// `pub mod` line, its `}` not yet), for the code of `srcs` (text,
    /// file): pushed at its end. `module`: the app module it is; `base`:
    /// how it reaches `__mods`.
    pub(super) fn auto_uses(
        &mut self,
        start: usize,
        srcs: &[(String, String)],
        module: Option<&str>,
        base: &str,
    ) -> Result<(), String> {
        let used: Vec<crate::auto::Src> = (srcs.iter())
            .map(|(text, rel)| crate::auto::Src { text, rel })
            .collect();
        let found = self.auto.uses(&used, &self.out[start..], module, base)?;
        if found.is_empty() {
            return Ok(());
        }
        self.out.push_str(&crate::auto::lines(&found));
        let rel = srcs.first().map_or_else(String::new, |s| s.1.clone());
        self.imports.push((rel, found));
        Ok(())
    }

    /// The code of the app file `m` names Rust in, and of its templates.
    pub(super) fn auto_srcs(
        &self,
        p: &Project,
        m: &UserMod,
        tpls: &[&Tpl],
    ) -> Result<Vec<(String, String)>, String> {
        let rel = p.rel(&m.file);
        let text = match &m.inline {
            Some(s) => s.clone(),
            None => crate::read_source(&m.file).map_err(|e| format!("{rel}: {e}"))?,
        };
        let mut out = vec![(text, rel)];
        for t in tpls {
            out.push(tpl_src(p, t)?);
        }
        Ok(out)
    }

    /// `code` with its `cx.user()`s given the users table, if it has any.
    pub(super) fn bound(&self, code: &str, rel: &str) -> Result<Option<String>, String> {
        rust_scan::bind_user(code, self.users.as_deref()).map_err(|e| format!("{rel}: {e}"))
    }

    pub(super) fn template(
        &mut self,
        t: &Tpl,
        comps: &[Comp],
        client: Option<&Client>,
    ) -> Result<(), String> {
        if t.kind != Kind::Page && awaits_in(&t.t.nodes) {
            return Err(format!(
                "{}: `{{#await}}` goes in a page; a layout, component or error page renders whole",
                t.rel
            ));
        }
        self.line(0, &format!("// {}", t.rel));
        self.line(0, "#[doc(hidden)]");
        self.line(0, "#[allow(unused_imports, unused_variables, unused_mut, unused_parens, unused_braces, unused_macros, dead_code, clippy::all)]");
        self.line(0, &format!("pub mod {} {{", t.module));
        // How `{expr}` is written: see `wisp::rt::Text`.
        self.line(
            1,
            "use ::wisp::rt::{Always as _, Direct as _, Formatted as _, Maybe as _};",
        );
        // Inside the module of its `+page.rs` (or `+layout.rs`), it sees
        // what the file sees, private items and the prelude too.
        // Otherwise it sees the app's modules (`db::find`).
        if t.user.is_some() {
            self.line(1, "use super::*;");
        } else {
            self.line(1, "use super::__mods::*;");
            self.db_items("super::__mods::*");
        }
        // The messages its `t("key")` calls read (see `i18n`).
        if t.i18n || client.is_some_and(|c| !c.texts.is_empty()) {
            let up = if t.user.is_some() {
                "super::super"
            } else {
                "super"
            };
            self.line(1, &format!("use {up}::__i18n as __wisp_i18n;"));
        }
        if !self.release {
            let chunks: Vec<String> = t.t.chunks.iter().map(|c| lit(c)).collect();
            // Named so that no name in the app's code can collide with them.
            self.line(
                1,
                &format!(
                    "static __WISP_S: [&str; {}] = [{}];",
                    chunks.len(),
                    chunks.join(", ")
                ),
            );
            self.line(1, "#[inline(always)]");
            self.line(1, &format!("fn __wisp_s(i: usize) -> &'static str {{ ::wisp::rt::chunk({}, i, __WISP_S[i]) }}", t.id));
        }
        if let Some(c) = client {
            let (path, url) = (c.path(), format!("{}?v={}", c.path(), c.hash));
            self.line(1, &format!(
                "pub static __WISP_CLIENT: ::wisp::ClientModule = ::wisp::ClientModule {{ id: {}, path: {}, url: {}, etag: {}, source: {}, preload: {}, texts: &[{}] }};",
                lit(&c.id),
                lit(crate::protocol::unbased(&path)),
                lit(&url),
                lit(&format!("\"{}\"", c.hash)),
                lit(&c.source),
                preload_list(&c.preload),
                c.texts.iter().map(|k| format!("&__wisp_i18n::J{k}")).collect::<Vec<_>>().join(", ")
            ));
        }
        // A load hands its `Data` over; statements are in the render itself.
        let loaded = t.stmts.is_none() && matches!(t.user, Some((_, true)));
        let data = if loaded {
            ", __d: &super::__call::Loaded"
        } else {
            ""
        };
        let sig = match t.kind {
            // The statements run first, and may await and use `cx` mutably;
            // then `__wrap` renders the layouts around the page.
            Kind::Page if t.stmts.is_some() => "pub async fn render(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, \
                 __wrap: impl FnOnce(&mut ::wisp::Out, &::wisp::Cx, &dyn Fn(&mut ::wisp::Out))) -> ::wisp::Result<()>"
                .into(),
            Kind::Page => format!("pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx{data})"),
            Kind::Layout => {
                let slots: String = (t.slots.iter())
                    .map(|s| format!(", {s}: &dyn Fn(&mut ::wisp::Out)"))
                    .collect();
                // `TITLE`: no page or layout inside writes a `<title>`.
                let title = if t.t.has_title() { "<const TITLE: bool>" } else { "" };
                format!(
                    "pub fn render{title}(__o: &mut ::wisp::Out, cx: &::wisp::Cx{data}, children: &dyn Fn(&mut ::wisp::Out){slots})"
                )
            }
            Kind::Error => {
                "pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx, status: u16, message: &str)"
                    .into()
            }
            Kind::Component => {
                let props: String =
                    t.t.props
                        .iter()
                        .flat_map(|(ds, _)| ds)
                        .map(|d| format!(", {}: {}", d.name, d.ty))
                        .collect();
                format!(
                    "pub fn render(__o: &mut ::wisp::Out{props}, children: &dyn Fn(&mut ::wisp::Out))"
                )
            }
        };
        match &t.t.props {
            Some((_, line)) => self.line(1, &format!("{sig} {{ // {}:{line}", t.rel)),
            None => self.line(1, &format!("{sig} {{")),
        }
        if t.i18n {
            self.line(2, "let __wisp_l: u8 = __o.lang;");
        }
        // `data.count` is also `count`: `Copy` fields by value, the rest by
        // reference. A local of the same name shadows it.
        if loaded {
            self.line(2, "let data: &super::Data = &__d.0;");
            for (name, t) in &t.data {
                if !matches!(name.as_str(), "data" | "children") {
                    let by = if ty::is_copy(t) { "" } else { "&" };
                    self.line(2, &format!("let {name} = {by}data.{name};"));
                }
            }
        }
        if let Some((stmts, binds)) = &t.stmts {
            for b in binds {
                self.line(2, b);
            }
            self.rust_lines(2, stmts, 1, &t.rel)?;
            if t.kind == Kind::Page {
                self.line(2, "let cx: &::wisp::Cx = cx;");
                self.line(2, "__wrap(__o, cx, &|__o: &mut ::wisp::Out| {");
            }
            let names = data_names(client);
            if !names.is_empty() {
                let params: Vec<String> = (0..names.len()).map(|k| format!("T{k}")).collect();
                let fields: Vec<String> = names
                    .iter()
                    .zip(&params)
                    .map(|(n, p)| format!("{n}: {p}"))
                    .collect();
                let values: Vec<String> = names.iter().map(|n| format!("{n}: &{n}")).collect();
                self.line(
                    2,
                    &format!(
                        "struct __WispData<{}> {{ {} }}",
                        params.join(", "),
                        fields.join(", ")
                    ),
                );
                self.line(
                    2,
                    &format!("let data = __WispData {{ {} }};", values.join(", ")),
                );
            }
        }
        if let Some(c) = client {
            // Each render is an instance: its id marks its elements, and its
            // record carries the server values its code reads.
            self.line(2, "let __wisp_i = {");
            self.line(
                3,
                "let (__wisp_i, __b) = ::wisp::rt::live(__o, &__WISP_CLIENT);",
            );
            // The runtime closes the record after it.
            self.json(3, "__b", &c.blob, &t.rel);
            self.line(3, "__wisp_i");
            self.line(2, "};");
        }
        let mut cx = Emit {
            rel: &t.rel,
            template: &t.t,
            comps,
            // Components are at the top of the generated file.
            top: if t.user.is_some() {
                "super::super::"
            } else {
                "super::"
            },
            target: "body",
            each_depth: 0,
            client,
            env: Vec::new(),
            props: 0,
            inert: false,
            paint: false,
            has_cx: t.kind != Kind::Component,
            gated: t.kind == Kind::Layout && t.t.has_title(),
            locals: t
                .stmts
                .as_ref()
                .map(|(s, _)| rust_scan::let_names(s))
                .unwrap_or_default(),
        };
        // Under `wisp dev`, what it renders goes between marks, so that a
        // change to its text morphs that alone in (wisp.js).
        let mark = (!self.release
            && matches!(t.kind, Kind::Page | Kind::Layout | Kind::Component)
            && !t.rel.contains("--"))
        .then(|| {
            let mark = |m: String| {
                format!(
                    "if ::wisp::rt::marks() {{ __o.body.push_str({}); }}",
                    lit(&m)
                )
            };
            (
                mark(format!("<!--w:{}-->", t.rel)),
                mark(format!("<!--/w:{}-->", t.rel)),
            )
        });
        if let Some((open, _)) = &mark {
            self.line(2, open);
        }
        let at = self.out.len();
        self.nodes(&t.t.nodes, 2, &mut cx);
        if let Some((_, close)) = &mark {
            self.line(2, close);
        }
        // Its form's fields read what an action refused: once a render.
        if self.out[at..].contains("__refused") {
            self.out
                .insert_str(at, "        let __refused = ::wisp::rt::refused(cx);\n");
        }
        if client.is_some() {
            self.line(2, "::wisp::rt::live_end(__o);");
        }
        if t.kind == Kind::Page && t.stmts.is_some() {
            self.line(2, "});");
            self.line(2, "Ok(())");
        }
        self.line(1, "}");
        if t.kind == Kind::Component {
            let props = t.t.props.as_ref().map_or(&[][..], |(p, _)| p);
            self.out.push_str(&crate::render::html_fn(props));
        }
        if self.types.is_some() {
            self.probe(t, client)?;
        }
        // A component the browser renders: its markup as the browser's copy
        // of it, painted from its props' JSON, for a page to show first.
        // Deep enough, a component rendering itself leaves the rest to the
        // browser.
        if let Some(c) = client.filter(|c| c.paints) {
            self.line(1, "pub fn paint(__o: &mut ::wisp::Out, __p: &[::wisp::rt::Js<'_>], children: &dyn Fn(&mut ::wisp::Out), __wisp_d: u32) {");
            self.line(2, "if __wisp_d > 32 { return; }");
            if t.i18n {
                self.line(2, "let __wisp_l: u8 = __o.lang;");
            }
            self.line(2, &format!("__o.body.push_str({});", lit(COPY_START)));
            let env: Vec<(String, Pv)> =
                t.t.props
                    .iter()
                    .flat_map(|(ds, _)| ds)
                    .enumerate()
                    .map(|(k, d)| (d.name.clone(), Pv::Val(format!("__p[{k}]"))))
                    .collect();
            let mut cx = Emit {
                props: env.len(),
                env,
                inert: false,
                paint: true,
                client: Some(c),
                ..cx
            };
            self.nodes(&t.t.nodes, 2, &mut cx);
            self.line(2, &format!("__o.body.push_str({});", lit(COPY_END)));
            self.line(1, "}");
        }
        self.line(0, "}");
        self.line(0, "");
        Ok(())
    }

    /// `wisp check --types`: `__wisp_types`, which reads the types of the
    /// block's values the browser code reads (`items` or `data.items`) from
    /// a closure of its statements that is never called (see `wisp::ts`).
    pub(super) fn probe(&mut self, t: &Tpl, client: Option<&Client>) -> Result<(), String> {
        let (Some((stmts, binds)), Some(mods)) = (&t.stmts, &mut self.types) else {
            return Ok(());
        };
        let lets = rust_scan::let_names(&format!("{}\n{stmts}", binds.join("\n")));
        let mut names: Vec<&str> = data_names(client);
        for p in client.iter().flat_map(|c| &c.blob) {
            if let Piece::Value { expr, .. } = p
                && lets.contains(expr)
                && !names.contains(&expr.as_str())
            {
                names.push(expr);
            }
        }
        names.retain(|n| lets.iter().any(|l| l == n));
        let (closure, pick) = match t.kind {
            _ if names.is_empty() => return Ok(()),
            Kind::Page => ("|cx: &'static mut ::wisp::Cx| async move {", "page"),
            Kind::Layout => ("|cx: &'static ::wisp::Cx| {", "layout"),
            _ => return Ok(()),
        };
        mods.push(match &t.user {
            Some((u, _)) => format!("{u}::{}", t.module),
            None => t.module.clone(),
        });
        let mut tail = String::from("()");
        for n in names.iter().rev() {
            tail = format!("((&&::wisp::ts::probe(&{n})).pick(), {tail})");
        }
        if t.kind == Kind::Page {
            tail = format!("::wisp::Result::Ok({tail})");
        }
        self.line(1, "::wisp::__ts! {");
        self.line(1, "#[allow(unreachable_code)]");
        self.line(1, "pub fn __wisp_types(__out: &mut String) {");
        self.line(2, &format!("let __f = {closure}"));
        for b in binds {
            self.line(3, b);
        }
        self.rust_lines(3, stmts, 1, &t.rel)?;
        self.line(3, "use ::wisp::ts::{ViaAny as _, ViaTs as _};");
        self.line(3, &tail);
        self.line(2, "};");
        let names: Vec<String> = names.iter().map(|n| lit(n)).collect();
        self.line(
            2,
            &format!(
                "::wisp::ts::{pick}(&__f, {}, &[{}], __out);",
                lit(&t.rel),
                names.join(", ")
            ),
        );
        self.line(1, "}");
        self.line(1, "}");
        Ok(())
    }

    /// Writes `pieces`, JSON with Rust values in it, to `buf` (a `&mut String`).
    pub(super) fn json(&mut self, ind: usize, buf: &str, pieces: &[Piece], rel: &str) {
        for p in pieces {
            match p {
                Piece::Text(t) => self.line(ind, &format!("{buf}.push_str({});", lit(t))),
                Piece::Value { expr, line } => self.line(
                    ind,
                    &format!("::wisp::rt::json({buf}, &({expr})); // {rel}:{line}"),
                ),
            }
        }
    }

    /// In a release build, a run of text and holes the build can write
    /// (literals) goes out as one `push_str`. A dev build keeps each text
    /// apart: `wisp dev` swaps them by index.
    pub(super) fn nodes<'n>(
        &mut self,
        nodes: impl IntoIterator<Item = &'n Node>,
        ind: usize,
        cx: &mut Emit,
    ) {
        let mut run = String::new();
        for n in nodes {
            match fold::fixed(n, cx.template).filter(|_| self.release) {
                Some(s) => run.push_str(&s),
                None => {
                    self.text(&mut run, ind, cx);
                    self.node(n, ind, cx);
                }
            }
        }
        self.text(&mut run, ind, cx);
    }

    /// Writes out the run `nodes` gathered, if any, and empties it.
    pub(super) fn text(&mut self, run: &mut String, ind: usize, cx: &Emit) {
        if !run.is_empty() {
            self.line(ind, &format!("__o.{}.push_str({});", cx.target, lit(run)));
            run.clear();
        }
    }

    pub(super) fn code_line(&mut self, ind: usize, s: &str, code: &Code, cx: &Emit) {
        self.line(ind, &format!("{s} // {}:{}", cx.rel, code.line));
    }

    /// `{#await future}`: its pending markup now; its `{:then}` or
    /// `{:catch}` in a closure the runtime runs once the future is done,
    /// after the page has gone (see `wisp::rt::defer`).
    pub(super) fn await_block(
        &mut self,
        future: &Code,
        pending: &[Node],
        then: &Option<(String, Vec<Node>)>,
        catch: &Option<(String, Vec<Node>)>,
        ind: usize,
        cx: &mut Emit,
    ) {
        self.line(ind, "{");
        self.line(
            ind + 1,
            "use ::wisp::rt::{AnyResult as _, Value as _, WispResult as _};",
        );
        let call = format!(
            "::wisp::rt::defer(__o, {}, move |__o: &mut ::wisp::Out, __v| {{",
            future.src
        );
        self.code_line(ind + 1, &call, future, cx);
        self.line(ind + 2, "let __r = match __v { Some(__v) => (&&&::wisp::rt::Settled::new(__v)).settle(), None => Err(::wisp::rt::failed()) };");
        self.line(ind + 2, "match __r {");
        // Rendered after the request, as a component is: a form's fields
        // write their own values (`Node::Kept`), and no problems.
        let has_cx = std::mem::replace(&mut cx.has_cx, false);
        for (how, branch) in [("Ok", then), ("Err", catch)] {
            match branch {
                Some((pat, body)) => {
                    self.line(ind + 3, &format!("{how}({pat}) => {{"));
                    self.nodes(body, ind + 4, cx);
                    self.line(ind + 3, "}");
                }
                None if how == "Ok" => self.line(ind + 3, "Ok(_) => {}"),
                None => self.line(ind + 3, "Err(_) => ::wisp::rt::await_failed(__o),"),
            }
        }
        cx.has_cx = has_cx;
        self.line(ind + 2, "}");
        self.line(ind + 1, "});");
        self.nodes(pending, ind + 1, cx);
        self.line(
            ind + 1,
            &format!("__o.body.push_str({});", lit(AWAIT_CLOSE)),
        );
        self.line(ind, "}");
    }

    pub(super) fn node(&mut self, n: &Node, ind: usize, cx: &mut Emit) {
        let buf = format!("__o.{}", cx.target);
        match n {
            // A release build writes text in runs (see `nodes`).
            Node::Text(i) => {
                debug_assert!(!self.release);
                self.line(ind, &format!("{buf}.push_str(__wisp_s({i}));"));
            }
            Node::Expr(code) => {
                self.code_line(
                    ind,
                    &format!("(&::wisp::rt::Text(&({}))).put(&mut {buf});", code.src),
                    code,
                    cx,
                );
            }
            Node::UrlStart { prefix } => self.line(
                ind,
                &format!("let __wisp_url = {buf}.len() - {};", prefix.len()),
            ),
            Node::UrlEnd => self.line(
                ind,
                &format!("::wisp::rt::guard_url(&mut {buf}, __wisp_url);"),
            ),
            Node::Attr { name, code, url } => {
                let push = |s: &str| format!("{buf}.push_str({});", lit(s));
                // `{if c { "a" } else { "b" }}`: both written at build time.
                if let Some((cond, yes, no)) = fold::either(&code.src)
                    && !(*url
                        && (crate::contexts::runs_script(&yes)
                            || crate::contexts::runs_script(&no)))
                {
                    let (yes, no) = (format!(" {name}=\"{yes}\""), format!(" {name}=\"{no}\""));
                    let line = format!("if {cond} {{ {} }} else {{ {} }}", push(&yes), push(&no));
                    self.code_line(ind, &line, code, cx);
                    return;
                }
                self.code_line(
                    ind,
                    &format!(
                        "if let Some(__v) = (&::wisp::rt::Attr(&({}))).get() {{",
                        code.src
                    ),
                    code,
                    cx,
                );
                self.line(ind + 1, &push(&format!(" {name}=\"")));
                if *url {
                    self.line(ind + 1, &format!("let __wisp_url = {buf}.len();"));
                }
                self.line(
                    ind + 1,
                    &format!("(&::wisp::rt::Text(__v)).put(&mut {buf});"),
                );
                if *url {
                    self.line(
                        ind + 1,
                        &format!("::wisp::rt::guard_url(&mut {buf}, __wisp_url);"),
                    );
                }
                self.line(ind + 1, &push("\""));
                self.line(ind, "}");
            }
            Node::Bool { name, code, class } => {
                // A class name first in its value (`class="` or `class='`
                // just written) takes no space before it.
                let push = if *class {
                    format!(
                        r#"if !{buf}.ends_with(['"', '\'']) {{ {buf}.push(' '); }} {buf}.push_str({});"#,
                        lit(name)
                    )
                } else {
                    format!("{buf}.push_str({});", lit(&format!(" {name}")))
                };
                self.code_line(ind, &format!("if ({}) {{ {push} }}", code.src), code, cx);
            }
            Node::Html(code) => self.code_line(
                ind,
                &format!("::wisp::rt::html(&mut {buf}, &({}));", code.src),
                code,
                cx,
            ),
            Node::Selected(code) => self.code_line(
                ind,
                &format!(
                    "if ::wisp::rt::is(&__wisp_sel, {}) {{ {buf}.push_str(\" selected\"); }}",
                    code.src
                ),
                code,
                cx,
            ),
            Node::Const(code) => self.code_line(ind, &format!("let {};", code.src), code, cx),
            Node::Kept {
                name,
                sent,
                own,
                line,
                tick,
            } => {
                let sent_as = match tick {
                    Some(v) if cx.has_cx => {
                        format!("::wisp::rt::ticked(cx, __refused, {v})")
                    }
                    Some(_) => "None::<bool>".into(),
                    None => kept(name, cx.has_cx),
                };
                let cond = Code {
                    src: format!("let Some(__k) = {sent_as}"),
                    line: *line,
                };
                self.code_line(
                    ind,
                    &format!("if {} {{", if_condition(&cond.src, &cx.locals)),
                    &cond,
                    cx,
                );
                self.nodes(sent, ind + 1, cx);
                if let Some(o) = own {
                    self.line(ind, "} else {");
                    self.nodes(o, ind + 1, cx);
                }
                self.line(ind, "}");
            }
            Node::Chosen {
                name,
                own,
                line,
                action,
            } => {
                let own = match own {
                    Some(own) => format!("(&::wisp::rt::Attr(&({own}))).get()"),
                    None => "None::<&str>".into(),
                };
                let code = Code {
                    src: format!(
                        "__wisp_sel = ::wisp::rt::chosen({own}, {})",
                        match cx.has_cx {
                            true =>
                                format!("::wisp::rt::sent(cx, __refused, {action:?}, {name:?})"),
                            false => "None".into(),
                        }
                    ),
                    line: *line,
                };
                self.code_line(ind, &format!("let {};", code.src), &code, cx);
            }
            Node::Problem { .. } if !cx.has_cx => {}
            Node::Problem { name, line, .. } => {
                let code = Code {
                    src: format!("::wisp::rt::problem(&mut {buf}, __refused, {name:?})"),
                    line: *line,
                };
                self.code_line(ind, &format!("{};", code.src), &code, cx);
            }
            Node::Render if cx.paint => {
                self.line(
                    ind,
                    &format!(
                        "{buf}.push_str({});",
                        lit(&format!("<template {SLOT_ATTR}></template>"))
                    ),
                );
                self.line(ind, "children(__o);");
            }
            Node::Render => self.line(ind, "children(__o);"),
            // A macro rather than a closure: each render gives the
            // parameters their types, and the body still sees the names
            // around the definition.
            Node::Snippet {
                name,
                params,
                body,
                line,
            } => {
                let args: String = (0..params.len()).map(|k| format!(", $a{k}:expr")).collect();
                self.line(
                    ind,
                    &format!(
                        "macro_rules! {} {{ ($__o:expr{args}) => {{{{ // {}:{line}",
                        snippet_macro(name),
                        cx.rel
                    ),
                );
                self.line(ind + 1, "let __o: &mut ::wisp::Out = $__o;");
                for (k, p) in params.iter().enumerate() {
                    self.line(ind + 1, &format!("let {p} = $a{k};"));
                }
                self.nodes(body, ind + 1, cx);
                self.line(ind, "}} }");
            }
            Node::RenderSnippet { name, args, local } => {
                let rest = if args.src.is_empty() {
                    String::new()
                } else {
                    format!(", {}", args.src)
                };
                let call = if *local {
                    format!("{}!(__o{rest});", snippet_macro(name))
                } else {
                    format!("{name}(__o{rest});")
                };
                self.code_line(ind, &call, args, cx);
            }
            Node::If {
                branches,
                otherwise,
            } => {
                for (k, (cond, body)) in branches.iter().enumerate() {
                    let kw = if k == 0 { "if" } else { "} else if" };
                    self.code_line(
                        ind,
                        &format!("{kw} {} {{", if_condition(&cond.src, &cx.locals)),
                        cond,
                        cx,
                    );
                    self.nodes(body, ind + 1, cx);
                }
                if let Some(o) = otherwise {
                    self.line(ind, "} else {");
                    self.nodes(o, ind + 1, cx);
                }
                self.line(ind, "}");
            }
            Node::Each {
                iter,
                pat,
                index,
                body,
                otherwise,
            } => {
                let empty = format!("__empty{}", cx.each_depth);
                cx.each_depth += 1;
                // A field path is iterated by reference. Method-call syntax lets
                // autoderef find the impl whether the field is a Vec, a slice
                // reference or a map.
                let amp = if is_place(iter.src.trim(), &cx.locals) {
                    "&"
                } else {
                    ""
                };
                let src = format!("({amp}({})).into_iter()", iter.src);
                let head = match index {
                    Some(i) => format!("for ({i}, {pat}) in {src}.enumerate() {{"),
                    None => format!("for {pat} in {src} {{"),
                };
                if let Some(o) = otherwise {
                    self.line(ind, "{");
                    self.line(ind + 1, &format!("let mut {empty} = true;"));
                    self.code_line(ind + 1, &head, iter, cx);
                    self.line(ind + 2, &format!("{empty} = false;"));
                    self.nodes(body, ind + 2, cx);
                    self.line(ind + 1, "}");
                    self.line(ind + 1, &format!("if {empty} {{"));
                    self.nodes(o, ind + 2, cx);
                    self.line(ind + 1, "}");
                    self.line(ind, "}");
                } else {
                    self.code_line(ind, &head, iter, cx);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind, "}");
                }
                cx.each_depth -= 1;
            }
            Node::Match { scrutinee, arms } => {
                self.code_line(
                    ind,
                    &format!("match {} {{", borrow_place(&scrutinee.src, &cx.locals)),
                    scrutinee,
                    cx,
                );
                for (pat, body) in arms {
                    self.code_line(ind + 1, &format!("{} => {{", pat.src), pat, cx);
                    self.nodes(body, ind + 2, cx);
                    self.line(ind + 1, "}");
                }
                self.line(ind, "}");
            }
            Node::Await {
                future,
                pending,
                then,
                catch,
            } => self.await_block(future, pending, then, catch, ind, cx),
            Node::Head(body) if cx.gated && cx.template.is_title(n) => {
                let prev = cx.target;
                cx.target = "head";
                self.line(ind, "if TITLE {");
                self.nodes(body, ind + 1, cx);
                self.line(ind, "}");
                cx.target = prev;
            }
            Node::Head(body) => {
                let prev = cx.target;
                cx.target = "head";
                self.nodes(body, ind, cx);
                cx.target = prev;
            }
            Node::Component {
                name,
                props,
                children,
                line,
            } => self.component(name, props, children, *line, ind, cx),
            Node::Client(branches) => self.client_block(branches, ind, cx),
            // `<wisp:element this="…">`'s name: the tag, when the server
            // knows it.
            Node::Tag { group } => {
                let g = &cx.template.groups[*group];
                let js = g
                    .directives
                    .iter()
                    .find(|d| d.kind == Dir::Tag)
                    .and_then(|d| d.value.as_ref())
                    .map_or("", |v| v.src.as_str());
                match paint_value(cx, *group, js).and_then(|v| v.val()) {
                    Some(v) => self.line(
                        ind,
                        &format!(
                            "::wisp::rt::tag_name(&mut {buf}, {v}); // {}:{}",
                            cx.rel, g.line
                        ),
                    ),
                    None => self.line(ind, &format!("{buf}.push_str(\"wisp-element\");")),
                }
            }
            // `{:x}`: the value, when the server knows it.
            Node::Hole { group } => {
                let g = &cx.template.groups[*group];
                let js = g.directives[0]
                    .value
                    .as_ref()
                    .map_or("", |v| v.src.as_str());
                let html = !g.directives[0].name.is_empty();
                if let Some(put) = paint_value(cx, *group, js)
                    .filter(|_| !html)
                    .and_then(|v| v.text(&buf))
                {
                    self.line(ind, &format!("{put} // {}:{}", cx.rel, g.line));
                }
            }
            Node::Live { group } => self.live(*group, ind, cx),
        }
    }

    /// A component the server renders: its `render`, given its props in the
    /// order it declares them, then its children.
    pub(super) fn component(
        &mut self,
        name: &str,
        props: &[template::Prop],
        children: &Option<Vec<Node>>,
        line: u32,
        ind: usize,
        cx: &mut Emit,
    ) {
        let c = cx
            .comps
            .iter()
            .find(|c| c.name == *name)
            .expect("check_components found it");
        // Props in the order the component declares them. A reference
        // type takes a borrow of the expression, so `title={post.title}`
        // passes a `&String` where the prop is a `&str`.
        let mut args = String::new();
        // A browser value (a prop only `$props()` names) is any
        // `Json`, text and flags too.
        let any = |v: &PropValue| match v {
            PropValue::Text(text) => Some(format!("&{}", lit(text))),
            PropValue::Flag => Some("&true".into()),
            _ => None,
        };
        for d in &c.props {
            let by_ref = d.ty.starts_with('&');
            let given = props.iter().find(|p| p.name == d.name).map(|p| &p.value);
            if d.name == REST && c.rest {
                let rest: Vec<String> = props
                    .iter()
                    .filter(|p| {
                        !p.name.starts_with("client:") && !c.props.iter().any(|x| x.name == p.name)
                    })
                    .map(|p| {
                        let v = match &p.value {
                            PropValue::Expr(code) => format!("&({})", code.src),
                            v => any(v).unwrap_or_else(|| "&()".into()),
                        };
                        format!("({}, {v} as {DYN})", lit(&p.name))
                    })
                    .collect();
                args.push_str(&format!(", &[{}]", rest.join(", ")));
                continue;
            }
            if d.ty == DYN
                && let Some(v) = given.and_then(any)
            {
                args.push_str(", ");
                args.push_str(&v);
                continue;
            }
            let arg = match given {
                Some(PropValue::Expr(code)) if by_ref => format!("&({})", code.src),
                Some(PropValue::Expr(code)) => format!("({})", code.src),
                Some(PropValue::Text(text)) if by_ref || d.ty.starts_with("impl") => lit(text),
                Some(PropValue::Text(text)) => {
                    format!("::core::convert::Into::into({})", lit(text))
                }
                Some(PropValue::Flag) => "true".into(),
                // A closure over the macro, typed by the prop's `&dyn Fn`.
                Some(PropValue::Snippet { name, arity }) => {
                    let a: String = (0..*arity).map(|k| format!(", __a{k}")).collect();
                    format!(
                        "&|__o: &mut ::wisp::Out{a}| {}!(__o{a})",
                        snippet_macro(name)
                    )
                }
                Some(PropValue::Live(_) | PropValue::Bind(_) | PropValue::On(_)) => {
                    unreachable!("the parser makes such a component a client one")
                }
                None => format!(
                    "({})",
                    d.default
                        .as_deref()
                        .expect("check_components found every required prop")
                ),
            };
            args.push_str(", ");
            args.push_str(&arg);
        }
        let call = format!("{}{}::render(__o{args}", cx.top, c.module);
        // `client:visible` and the like: an island, which starts late.
        let how = props.iter().find_map(|p| {
            let how = template::how(p.name.strip_prefix("client:")?)?;
            match &p.value {
                _ if how.is_empty() => None,
                PropValue::Text(q) if how == ISLAND_MEDIA => Some(template::island(how, q)),
                _ if how == ISLAND_MEDIA => None,
                _ => Some(how.to_string()),
            }
        });
        if let Some(how) = how {
            self.line(ind, &format!("::wisp::rt::live_how(__o, {});", lit(&how)));
        }
        // Snippets among the children are props: defined before the
        // call, in a block of their own.
        let is_snippet = |n: &&Node| matches!(n, Node::Snippet { .. });
        let defs: Vec<&Node> = children.iter().flatten().filter(is_snippet).collect();
        let ind = if defs.is_empty() {
            ind
        } else {
            self.line(ind, "{");
            for d in &defs {
                self.node(d, ind + 1, cx);
            }
            ind + 1
        };
        match children {
            Some(body) => {
                self.line(
                    ind,
                    &format!("{call}, &|__o: &mut ::wisp::Out| {{ // {}:{line}", cx.rel),
                );
                self.nodes(body.iter().filter(|n| !is_snippet(n)), ind + 1, cx);
                self.line(ind, "});");
            }
            None => self.line(
                ind,
                &format!("{call}, &|_: &mut ::wisp::Out| {{}}); // {}:{line}", cx.rel),
            ),
        }
        if !defs.is_empty() {
            self.line(ind - 1, "}");
        }
    }

    /// A directive element's marks: its group, the loop values its
    /// directives read, and the attributes whose values the server knows.
    pub(super) fn live(&mut self, group: usize, ind: usize, cx: &Emit) {
        let buf = format!("__o.{}", cx.target);
        let c = cx.client.expect("a template with directives has a module");
        let push = |s: String| format!("{buf}.push_str({});", lit(&s));
        if cx.template.groups[group].nested || cx.paint {
            self.line(ind, &push(format!(" {GROUP_ATTR}=\"{group}\"")));
        } else {
            self.line(ind, &push(format!(" {GROUP_ATTR}=\"")));
            self.line(
                ind,
                &format!("(&::wisp::rt::Text(&__wisp_i)).put(&mut {buf});"),
            );
            self.line(ind, &push(format!(".{group}\"")));
        }
        // The loop values its directives read, as JSON.
        let locals = &c.locals[group];
        if !locals.is_empty() {
            self.line(ind, &push(format!(" {LOOP_ATTR}=\"")));
            self.line(ind, "{");
            self.line(ind + 1, "let __j = &mut ::std::string::String::new();");
            self.json(ind + 1, "__j", locals, cx.rel);
            self.line(ind + 1, &format!("::wisp::rt::escape(&mut {buf}, __j);"));
            self.line(ind, "}");
            self.line(ind, &push("\"".into()));
        }
        // `name={:x}` attributes whose value the server knows, and boolean
        // ones such as `:hidden="!open"` (with `let open = false`): the
        // browser would set them first thing, so the page starts so.
        let g = &cx.template.groups[group];
        for d in g.directives.iter().filter(|d| d.kind == Dir::Spread) {
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            if let Some(v) = paint_value(cx, group, js).and_then(|v| v.val()) {
                self.line(
                    ind,
                    &format!(
                        "::wisp::rt::js_attrs(&mut {buf}, {:?}, {v}); // {}:{}",
                        d.name, cx.rel, d.line
                    ),
                );
            }
        }
        for d in g.directives.iter().filter(|d| {
            d.kind == Dir::Attr
                && (d.mods == ["{}"] || template::BOOLEAN_ATTRS.contains(&d.name.as_str()))
        }) {
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            if let Some(v) = paint_value(cx, group, js).and_then(|v| v.val()) {
                self.line(
                    ind,
                    &format!(
                        "::wisp::rt::js_attr(&mut {buf}, {}, {v}); // {}:{}",
                        lit(&d.name),
                        cx.rel,
                        d.line
                    ),
                );
            }
        }
    }

    /// A client block or component: each branch's `<template>`, whose
    /// content the browser copies (so nothing in it is painted), and after
    /// it, when the server knows the values, the copies the browser would
    /// make, each between `<!--[-->` and `<!--]-->`, which it takes over.
    pub(super) fn client_block(
        &mut self,
        branches: &[(usize, Vec<Node>)],
        ind: usize,
        cx: &mut Emit,
    ) {
        let (target, tpl) = (cx.target, cx.template);
        let push = |s: &str| format!("__o.{target}.push_str({});", lit(s));
        let (open, close, start, end) = (
            push("<template"),
            push(">"),
            push(COPY_START),
            push(COPY_END),
        );
        // Painted copies of an `{:#each}`, which its `{:else}` needs none of.
        let mut count: Option<String> = None;
        for (k, (group, body)) in branches.iter().enumerate() {
            self.line(ind, &open);
            self.node(&Node::Live { group: *group }, ind, cx);
            self.line(ind, &close);
            let inert = std::mem::replace(&mut cx.inert, true);
            self.nodes(body, ind, cx);
            cx.inert = inert;
            self.line(ind, &push("</template>"));
            // A page the browser draws: nothing painted, whatever is known.
            if tpl.drawn == Some(*group) {
                continue;
            }
            let g = &tpl.groups[*group];
            let d = &g.directives[0];
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            let at = format!("// {}:{}", cx.rel, g.line);
            match d.kind {
                Dir::Each => {
                    let n = cx.each_depth;
                    let (item, index) = (format!("__wisp_e{n}"), format!("__wisp_k{n}"));
                    let head = match paint_value(cx, *group, js) {
                        Some(Pv::Val(list)) => {
                            format!("for ({index}, {item}) in {list}.items().enumerate() {{ {at}")
                        }
                        // `[x]`, as `{:@render}` passes an argument: one item, `x`.
                        _ => match js
                            .strip_prefix('[')
                            .and_then(|s| s.strip_suffix(']'))
                            .and_then(|x| paint_value(cx, *group, x)?.val())
                        {
                            Some(x) => format!("{{ let ({index}, {item}) = (0usize, {x}); {at}"),
                            None => continue,
                        },
                    };
                    if branches.len() > 1 {
                        count = Some(format!("__wisp_n{n}"));
                        self.line(ind, &format!("let mut __wisp_n{n} = 0usize;"));
                    }
                    self.line(ind, &head);
                    if count.is_some() {
                        self.line(ind + 1, &format!("__wisp_n{n} += 1;"));
                    }
                    let outer = cx.env.len();
                    cx.env.push((d.name.clone(), Pv::Val(item)));
                    if let Some(i) = d.mods.first() {
                        cx.env.push((i.clone(), Pv::Num(format!("Some({index})"))));
                    }
                    cx.each_depth += 1;
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    cx.each_depth -= 1;
                    cx.env.truncate(outer);
                    self.line(ind, "}");
                }
                Dir::If if k > 0 && count.is_some() => {
                    let n = count.as_deref().unwrap_or_default();
                    self.line(ind, &format!("if {n} == 0 {{ {at}"));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                }
                Dir::If => {
                    let Some(v) = paint_value(cx, *group, js) else {
                        continue;
                    };
                    self.line(ind, &format!("if {} {{ {at}", v.test()));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                }
                Dir::Comp => self.paint_comp(*group, body, ind, cx),
                // One copy, always: an await as pending, a try as not failed.
                Dir::Key | Dir::Await | Dir::Try => {
                    let outer = cx.env.len();
                    let state = match d.kind {
                        Dir::Await => Some(("__aw", "{\"k\":0}")),
                        Dir::Try => Some(("__tr", "{}")),
                        _ => None,
                    };
                    if let Some((name, json)) = state {
                        cx.env.push((
                            name.into(),
                            Pv::Val(format!("::wisp::rt::Js({})", lit(json))),
                        ));
                    }
                    self.line(ind, &format!("{{ {at}"));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                    cx.env.truncate(outer);
                }
                _ => {}
            }
        }
    }

    /// A component the browser renders, painted by its `paint` when every
    /// prop is known. What it is given as children is painted here.
    pub(super) fn paint_comp(&mut self, group: usize, body: &[Node], ind: usize, cx: &mut Emit) {
        let (target, tpl, comps) = (cx.target, cx.template, cx.comps);
        let g = &tpl.groups[group];
        let d = &g.directives[0];
        let Some(comp) = comps.iter().find(|c| c.name == d.name) else {
            return;
        };
        // What a spread gives is the browser's to work out.
        if d.props
            .iter()
            .any(|p| p.name == "..." || matches!(p.value, PropValue::Snippet { .. }))
        {
            return;
        }
        let mut args = Vec::new();
        for p in &comp.props {
            let arg = match d.props.iter().find(|x| x.name == p.name).map(|x| &x.value) {
                Some(PropValue::Text(s)) => format!("::wisp::rt::Js({})", lit(&js_str(s))),
                Some(PropValue::Flag) => "::wisp::rt::Js(\"true\")".into(),
                Some(PropValue::Live(c) | PropValue::Bind(c)) => {
                    match paint_value(cx, group, &c.src).and_then(|v| v.val()) {
                        Some(v) => v,
                        None => return,
                    }
                }
                _ => "::wisp::rt::Js(\"null\")".into(),
            };
            args.push(arg);
        }
        let depth = if cx.paint { "__wisp_d + 1" } else { "0" };
        self.line(
            ind,
            &format!(
                "{}{}::paint(__o, &[{}], &|__o: &mut ::wisp::Out| {{ // {}:{}",
                cx.top,
                comp.module,
                args.join(", "),
                cx.rel,
                g.line
            ),
        );
        let push = |s: &str| format!("__o.{target}.push_str({});", lit(s));
        self.line(ind + 1, &push(COPY_START));
        self.nodes(body, ind + 1, cx);
        self.line(ind + 1, &push(COPY_END));
        self.line(ind, &format!("}}, {depth});"));
    }
}
