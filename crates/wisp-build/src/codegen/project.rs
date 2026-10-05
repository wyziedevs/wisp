//! Reading the app: its routes, templates, layouts, components and the
//! browser half, into a `Project`.

use super::*;

/// The app, as far as it has been read.
pub(super) struct Project<'a> {
    pub(super) root: &'a Path,
    pub(super) release: bool,
    pub(super) maps: bool,
    pub(super) prerendered: Option<&'a Path>,
    pub(super) tree: crate::routes::Tree,
    /// `redirects`, `rewrites` and `headers` of Cargo.toml.
    pub(super) rules: crate::config::Rules,
    /// `src/app.html` (or the default) in its three pieces.
    pub(super) shell: [String; 3],
    pub(super) comps: Vec<Comp>,
    /// Every template but the shell, which is template 0: `templates[k]`
    /// is template `k + 1`.
    pub(super) templates: Vec<Tpl>,
    /// The modules of the app's route files, and the shims in them.
    pub(super) user_mods: Vec<UserMod>,
    /// The routes, layouts and error pages, as codegen prints them.
    pub(super) model: Model,
    pub(super) hooks: Option<UserMod>,
    /// `before` in hooks.rs is an `async fn`: every request may wait.
    pub(super) before_waits: bool,
    /// The app's own modules (`src/notes.rs`).
    pub(super) mods: Vec<UserMod>,
    /// The types of `src/*.rs`, which endpoints and action forms may name.
    pub(super) shared: Vec<rust_scan::TypeItem>,
    /// The `.live()` tables (static, channel) a page reading one listens to.
    pub(super) lives: Vec<(String, String)>,
    /// The `PUBLIC_*` variables, for browser code's `env.PUBLIC_X`.
    pub(super) env: Vec<(String, String)>,
    /// The Markdown pages, by route pattern, with their front matter: what
    /// `wisp::pages` lists.
    pub(super) md_pages: Vec<(String, Vec<(String, String)>)>,
    /// `src/locales`, and per key whether a template's `t("key")` uses it.
    pub(super) i18n: Option<i18n::Locales>,
    pub(super) t_used: Vec<bool>,
    /// The `#[remote]` functions of pages and `src/*.rs`.
    pub(super) remotes: Vec<RemoteFn>,
    /// What each layout sets for the pages below it, by layout.
    pub(super) layout_opts: Vec<LayoutOpts>,
}

/// The page options a `+layout` sets, which its pages inherit unless they
/// set their own.
#[derive(Default, Clone, Copy)]
pub(super) struct LayoutOpts {
    /// `CACHE` (false) or `CACHE_PUBLIC` (true).
    pub(super) cache: Option<bool>,
    pub(super) ssr: Option<bool>,
    pub(super) prerender: Option<bool>,
}

/// The browser's half: the modules of templates (by template), and the
/// other JavaScript files served.
pub(super) struct Web {
    pub(super) clients: Vec<Option<Client>>,
    pub(super) js_files: Vec<JsFile>,
}

impl Web {
    /// The modules under `prefix` (an npm package's, from esm.sh or
    /// `.wisp/npm`) that the app's own modules import, by their path after
    /// it, each once.
    pub(super) fn imports(&self, prefix: &str) -> Vec<String> {
        let mut out = std::collections::BTreeSet::new();
        let sources = self.js_files.iter().map(|f| &f.source);
        for src in sources.chain(self.clients.iter().flatten().map(|c| &c.source)) {
            if !src.contains(prefix) {
                continue;
            }
            let _ = js::specifiers(src, |s| {
                if let Some(rest) = s.strip_prefix(prefix) {
                    out.insert(rest.to_string());
                }
                Ok(None)
            });
        }
        out.into_iter().collect()
    }
}

impl<'a> Project<'a> {
    /// A release build's id, in a `<meta name="wisp-build">` of the shell:
    /// a hash of its templates and Rust. Baked in, so a request does no more
    /// for it; wisp.js reads it from the page it fetches, and loads a page
    /// of another build whole (see `go` in wisp.js) instead of swapping it
    /// into scripts that are not its own.
    pub(super) fn stamp(&mut self) {
        let mut h = fnv1a(crate::runtime_version().as_bytes());
        for t in &self.templates {
            h = h.rotate_left(7) ^ t.t.shape;
            for c in &t.t.chunks {
                h = h.rotate_left(7) ^ fnv1a(c.as_bytes());
            }
        }
        let rust = self.user_mods.iter().chain(&self.mods).chain(&self.hooks);
        for m in rust {
            h = h.rotate_left(7) ^ fnv1a(&fs::read(&m.file).unwrap_or_default());
        }
        let meta = format!("<meta name=\"wisp-build\" content=\"{h:016x}\">\n");
        self.shell[0].push_str(&meta);
    }

    pub(super) fn new(input: &Input<'a>) -> Result<Project<'a>, String> {
        let root = input.root;
        let tree = crate::routes::scan(&root.join("src").join("routes"))?;
        let rules = crate::config::load(root, &tree)?;
        let shell_path = root.join("src").join("app.html");
        let shell_src = match shell_path.exists() {
            true => crate::read_source(&shell_path).map_err(|e| format!("src/app.html: {e}"))?,
            false => shell::DEFAULT.to_string(),
        };
        let mut shell = shell::split(&shell_src).map_err(|e| format!("src/app.html: {e}"))?;
        let i18n = i18n::load(root)?;
        // `<html lang>` says each request's locale: a shell without one
        // gets one to say it in (the first locale, in a baked page).
        if let Some(l) = &i18n {
            shell[0] = i18n::with_lang(&shell[0], &l.names[0]);
        }
        // The links of the shell are under the base path, as a template's are.
        if !crate::protocol::BASE.is_empty() {
            for part in &mut shell {
                if let Some(based) = template::under_base(part, crate::protocol::BASE) {
                    *part = based;
                }
            }
        }
        // The fonts' preload links come first in the head.
        let faces = crate::fonts::load(root)?;
        shell[0].push_str(&wisp_shared::fonts::preloads(&faces, crate::protocol::BASE));
        shell[0].push_str(&crate::loading::tag(&tree, root)?);
        let t_used = vec![false; i18n.as_ref().map_or(0, i18n::Locales::key_count)];
        Ok(Project {
            root,
            release: input.release,
            maps: input.maps,
            prerendered: input.prerendered,
            tree,
            rules,
            shell,
            comps: Vec::new(),
            templates: Vec::new(),
            user_mods: Vec::new(),
            model: Model::default(),
            hooks: None,
            before_waits: false,
            mods: Vec::new(),
            shared: crate::shared_types(root),
            lives: crate::live_tables(root),
            env: crate::public_env(root),
            md_pages: Vec::new(),
            i18n,
            t_used,
            remotes: Vec::new(),
            layout_opts: Vec::new(),
        })
    }

    /// The whole app, read phase by phase, each finding what the next
    /// needs, into its model.
    pub(super) fn load(input: &Input<'a>) -> Result<Project<'a>, String> {
        let mut p = Project::new(input)?;
        p.components()?;
        p.layouts()?;
        p.error_pages()?;
        p.routes()?;
        p.slots()?;
        p.app_files()?;
        p.translate()?;
        Ok(p)
    }

    /// Each template's `t("key", …)` calls, checked and compiled (see
    /// `i18n`).
    pub(super) fn translate(&mut self) -> Result<(), String> {
        let Some(l) = &self.i18n else {
            return Ok(());
        };
        for t in &mut self.templates {
            let rel = &t.rel;
            let mut found = false;
            for_each_code(&mut t.t.nodes, &mut |c: &mut Code| {
                if !c.src.contains('t') {
                    return Ok(());
                }
                let src = l
                    .rust(&c.src, &mut self.t_used)
                    .map_err(|e| format!("{rel}:{}: {e}", c.line))?;
                found |= src != c.src;
                c.src = src;
                Ok(())
            })?;
            t.i18n = found;
        }
        Ok(())
    }

    /// `p` from the project root, `/`-separated: how errors name a file.
    pub(super) fn rel(&self, p: &Path) -> String {
        p.strip_prefix(self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// The templates' accessibility warnings, `file:line: what (a11y-name)`,
    /// in file order.
    pub(super) fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<(&str, u32, String)> = (self.templates.iter())
            .flat_map(|t| {
                t.t.lints
                    .iter()
                    .map(|l| (t.rel.as_str(), l.line, crate::lint_line(l)))
            })
            .collect();
        warnings.extend(self.slash_lints());
        if let Some(l) = &self.i18n {
            warnings.extend(
                l.warnings
                    .iter()
                    .map(|(f, n, w)| (f.as_str(), *n, w.clone())),
            );
        }
        warnings.sort();
        (warnings.into_iter())
            .map(|(rel, line, w)| format!("{rel}:{line}: {w}"))
            .collect()
    }

    /// Literal `href`s to the app's pages in the form that
    /// `wisp::trailing_slash` (in `src/hooks.rs`; `Never` without it)
    /// answers with a 308: `href="/about/"` where pages end without `/`.
    pub(super) fn slash_lints(&self) -> Vec<(&str, u32, String)> {
        let hooks = crate::read_source(&self.root.join("src").join("hooks.rs")).unwrap_or_default();
        let how = (hooks.split("trailing_slash(").nth(1))
            .and_then(|s| s.split(')').next())
            .and_then(|s| s.rsplit("::").next())
            .map(str::trim);
        let always = match how {
            Some("Always") => true,
            Some("Ignore") => return Vec::new(),
            _ => false,
        };
        let page = |path: &str| {
            let routes = self.tree.routes.iter().zip(&self.model.routes);
            (routes.filter(|(_, m)| m.page.is_some()))
                .any(|(r, _)| r.expansions().iter().any(|e| may_match(e, path)))
        };
        let mut out = Vec::new();
        for t in &self.templates {
            let mut src: Option<String> = None;
            for href in t.t.chunks.iter().flat_map(|c| hrefs(c)) {
                let path = href.split(['?', '#']).next().unwrap_or("");
                let bare = path.trim_end_matches('/');
                let wrong = match always {
                    true => {
                        !path.ends_with('/') && !bare.rsplit('/').next().unwrap_or("").contains('.')
                    }
                    false => path.len() > 1 && path.ends_with('/'),
                };
                if !wrong || path.starts_with("//") || path.starts_with("/_") || !page(bare) {
                    continue;
                }
                let src = src.get_or_insert_with(|| {
                    crate::read_source(&self.root.join(&t.rel)).unwrap_or_default()
                });
                let line = (src.find(&format!("href=\"{href}\"")))
                    .map_or(1, |i| src[..i].matches('\n').count() as u32 + 1);
                let (to, how) = match always {
                    true => (format!("{bare}/"), "Always"),
                    false => (bare.to_string(), "Never"),
                };
                out.push((
                    t.rel.as_str(),
                    line,
                    format!("href=\"{href}\" gets a 308 to {to} (wisp::trailing_slash({how})): link there"),
                ));
            }
        }
        out
    }

    /// The scoped `<style>`s of every template, for `/_app/app.css`.
    pub(super) fn styles(&self) -> String {
        crate::join_styles(
            self.root,
            (self.templates.iter())
                .filter_map(|t| Some((t.rel.as_str(), t.t.style.as_deref()?)))
                .collect(),
        )
    }

    pub(super) fn read(&self, p: &Path) -> Result<String, String> {
        crate::read_source(p).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The Rust types template `t` may name: the app's and its own
    /// block's (or `+page.rs`'s, `+layout.rs`'s).
    pub(super) fn types_of(&self, t: &Tpl) -> Vec<rust_scan::TypeItem> {
        let mut types = self.shared.clone();
        let file = crate::read_source(&self.root.join(&t.rel)).unwrap_or_default();
        let items = match crate::split_front(&file).ok().and_then(|(rust, _)| rust) {
            Some(block) => rust_scan::scan(&rust_scan::split_items(&block).0).ok(),
            None => (t.rel.strip_suffix(".wisp"))
                .and_then(|base| crate::read_source(&self.root.join(format!("{base}.rs"))).ok())
                .and_then(|rs| rust_scan::scan(&rs).ok()),
        };
        types.extend(items.into_iter().flat_map(|i| i.types));
        types
    }

    /// A layout or error page: with the Rust of its `---` block, if it has
    /// one.
    pub(super) fn parse(&self, p: &Path) -> Result<(Template, Option<String>), String> {
        let (front, markup) =
            crate::split_front(&self.read(p)?).map_err(|e| format!("{}:{e}", self.rel(p)))?;
        self.markup(p, &markup, front, &[], false)
    }

    /// The markup of `p`, its action forms' `fields` given the browser's
    /// checks: everything but a component. `drawn`: a page the browser
    /// draws.
    pub(super) fn markup(
        &self,
        p: &Path,
        markup: &str,
        front: Option<String>,
        fields: &[rules::Field],
        drawn: bool,
    ) -> Result<(Template, Option<String>), String> {
        let at = |e: String| format!("{}:{e}", self.rel(p));
        let markup = image::rewrite(markup, self.root, self.release).map_err(at)?;
        let markup = self.listen_live(p, front.as_deref(), markup);
        let (t, rust) =
            crate::parse_markup(&markup, front, fields, &self.rel(p), drawn).map_err(at)?;
        if let Some((_, line)) = t.props {
            return Err(at(format!(
                "{line}: only components, in src/components, take props"
            )));
        }
        Ok((t, rust))
    }

    /// The markup of a page or layout, with a `<script>` that listens to each
    /// `.live()` table its Rust names (`POSTS`), and refreshes the page when
    /// one changes: the page's own `---` block, or the `+page.rs` beside it.
    pub(super) fn listen_live<'x>(
        &self,
        p: &Path,
        front: Option<&str>,
        mut markup: std::borrow::Cow<'x, str>,
    ) -> std::borrow::Cow<'x, str> {
        let rs = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("+page") && n.ends_with(".wisp") => "+page.rs",
            Some("+layout.wisp") => "+layout.rs",
            _ => return markup,
        };
        if self.lives.is_empty() {
            return markup;
        }
        let mut code = front.unwrap_or_default().to_string();
        code.push_str(&crate::read_source(&p.with_file_name(rs)).unwrap_or_default());
        for (ident, table) in &self.lives {
            let url = format!("/_wisp/live/{table}");
            if !names(&code, ident) || markup.contains(&url) {
                continue;
            }
            let call = format!("listen('{url}', invalidate);");
            // A file has one client script: into it, at its end and on its
            // last line, so no line of the file moves.
            let end = (markup.find("<script>"))
                .and_then(|open| Some(open + markup[open..].find("</script>")?));
            match end {
                Some(at) => markup.to_mut().insert_str(at, &format!(";{call}")),
                None => (markup.to_mut()).push_str(&format!("\n<script>{call}</script>\n")),
            }
        }
        markup
    }

    /// A `+layout.rs`, `+page.rs` or `+server.rs`.
    pub(super) fn scan(&self, p: &Path) -> Result<rust_scan::Items, String> {
        let at = |e: String| format!("{}:{e}", self.rel(p));
        let items = rust_scan::scan(&self.read(p)?).map_err(at)?;
        items.check().map_err(at)?;
        Ok(items)
    }

    /// A page's or layout's Rust: its `+page.rs` or `+layout.rs` (`rs`), or
    /// the `---` block of its `.wisp`, split into the items its module holds
    /// and the statements that run before it renders.
    pub(super) fn logic(
        &self,
        rs: Option<PathBuf>,
        wisp: &Path,
        front: Option<String>,
        markup: &str,
    ) -> Result<Logic, String> {
        let Some(code) = front else {
            let items = match &rs {
                Some(f) => self.scan(f)?,
                None => rust_scan::Items::default(),
            };
            return Ok(Logic {
                items,
                file: rs,
                inline: None,
                stmts: None,
            });
        };
        if let Some(f) = rs {
            return Err(format!(
                "{}: this file starts with a `---` block of Rust, and {} is beside it; keep the Rust in one of them",
                self.rel(wisp),
                f.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ));
        }
        let code = rust_scan::mark_actions(&code, markup).unwrap_or(code);
        let (mut items_src, stmts) = rust_scan::split_items(&code);
        let at = |e: String| format!("{}:{e}", self.rel(wisp));
        // Its `mod server` is the route's endpoints, which `server` reads.
        if let Some((rest, _, line)) = rust_scan::split_server(&items_src) {
            let page = (wisp.file_name()).is_some_and(|n| n.to_string_lossy().starts_with("+page"));
            if !page {
                return Err(at(format!(
                    "{line}: `mod server` holds a route's endpoints, and only a +page.wisp is a route; move it into the page's block or a +server.rs"
                )));
            }
            if wisp.with_file_name("+server.rs").is_file() {
                return Err(at(format!(
                    "{line}: this block's `mod server` holds the route's endpoints, and +server.rs is beside it; \
                     keep them in one: move the handlers of +server.rs into `mod server` and delete it, or move `mod server`'s into +server.rs"
                )));
            }
            items_src = rest;
        }
        let items = rust_scan::scan(&items_src).map_err(at)?;
        items.check().map_err(at)?;
        let stmts = (!stmts.trim().is_empty()).then_some(stmts);
        if let (Some(s), Some(load)) = (&stmts, items.function("load")) {
            let first = s.lines().position(|l| !l.trim().is_empty()).unwrap_or(0) + 1;
            return Err(at(format!(
                "{first}: the statements of a `---` block are the page's load, and this block also has `fn load` (line {}); keep one",
                load.line
            )));
        }
        Ok(Logic {
            items,
            file: Some(wisp.to_path_buf()),
            inline: Some(items_src),
            stmts,
        })
    }

    /// Adds the template `t` of `file` (its module `module`), which is for
    /// `kind`; the rest of what it has is the caller's to fill in.
    pub(super) fn add_tpl(
        &mut self,
        module: String,
        file: &Path,
        kind: Kind,
        t: Template,
    ) -> &mut Tpl {
        let rel = self.rel(file);
        self.templates.push(Tpl {
            id: self.templates.len() + 1,
            module,
            rel,
            kind,
            user: None,
            data: Vec::new(),
            load_js: None,
            stmts: None,
            t,
            i18n: false,
            slots: Vec::new(),
        });
        self.templates.last_mut().expect("just pushed")
    }

    /// `src/components/*.wisp`: templates `1..=N`, one per component.
    pub(super) fn components(&mut self) -> Result<(), String> {
        let comp_dir = self.root.join("src").join("components");
        if !comp_dir.is_dir() {
            return Ok(());
        }
        let mut files = Vec::new();
        list_files(&comp_dir, &mut files)?;
        files.sort();
        // Per component, those its markup renders with nothing to stop them.
        let mut always: Vec<Vec<String>> = Vec::new();
        for file in files {
            let file_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if crate::routes::editor_temp(&file_name)
                || file.extension().is_none_or(|e| e != "wisp")
            {
                continue;
            }
            let rel = self.rel(&file);
            let name = file_name.trim_end_matches(".wisp").to_string();
            if !template::is_component_name(&name) {
                // `card.wisp` → `Card.wisp`, `UI.wisp` → `Ui.wisp`.
                let clean: String = name
                    .chars()
                    .filter(|c| c.is_ascii() && ty::is_word(*c as u8))
                    .collect();
                let lower = clean.bytes().any(|b| b.is_ascii_lowercase());
                let suggest: String = clean
                    .chars()
                    .enumerate()
                    .map(|(i, c)| match i {
                        0 => c.to_ascii_uppercase(),
                        _ if lower => c,
                        _ => c.to_ascii_lowercase(),
                    })
                    .collect();
                return Err(format!(
                    "{rel}: a component's file name is its tag: a capital letter first, lowercase letters too (a tag in all capitals is HTML), \
                     and only letters, digits and _, such as {suggest}.wisp"
                ));
            }
            if let Some(other) = self.comps.iter().find(|c| c.name == name) {
                return Err(format!(
                    "{rel}: there is already a component `{name}` ({})",
                    other.module
                ));
            }
            let at = |e: String| format!("{rel}:{e}");
            let (rust, markup) = crate::split_front(&self.read(&file)?).map_err(at)?;
            let markup = image::rewrite(&markup, self.root, self.release).map_err(at)?;
            let (mut t, rust) = crate::parse_markup(&markup, rust, &[], &rel, false).map_err(at)?;
            if rust.is_some() {
                return Err(format!(
                    "{rel}: a component takes what it shows as {{@props …}}; a `---` block of Rust is for pages and layouts"
                ));
            }
            if let Some(line) = first_await(&t.nodes) {
                return Err(format!(
                    "{rel}:{line}: a component renders without waiting, so its markup cannot `.await`; await in the page and pass the value as a prop"
                ));
            }
            // Both read the request's `cx`, which a component has not.
            if t.flash {
                return Err(format!(
                    "{rel}: a component has no request, so no {{@flash}}; put {{@flash}} in the page or a layout"
                ));
            }
            if crate::template::active_links(&markup).is_some() {
                return Err(format!(
                    "{rel}: a component has no request, so a link cannot say `active`; put the nav in a layout, or pass aria-current as a prop"
                ));
            }
            let rune = match &t.script {
                Some(s) => js::props_rune(&s.src).map_err(|(off, msg)| {
                    let (line, col) = script_pos(s, off);
                    format!("{rel}:{line}:{col}: {msg}")
                })?,
                None => None,
            };
            // Without {@props}, `$props()` says what the component takes.
            if t.props.is_none()
                && let Some(r) = &rune
            {
                t.props = Some((inferred_props(r), 0));
            }
            let props = t.props.as_ref().map(|(p, _)| p.clone()).unwrap_or_default();
            let module = format!("tpl_component_{}", self.comps.len());
            let bindable = rune.as_ref().map(|p| {
                p.props
                    .iter()
                    .filter(|x| x.bindable)
                    .map(|x| x.name.clone())
                    .collect()
            });
            always.push(
                (t.nodes.iter())
                    .filter_map(|n| match n {
                        Node::Component { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect(),
            );
            self.comps.push(Comp {
                rel,
                name,
                module: module.clone(),
                rest: props.iter().any(|d| d.name == REST),
                props,
                children: t.uses_children,
                bindable,
                live: t.is_live(),
            });
            self.add_tpl(module, &file, Kind::Component, t);
        }
        for k in 0..always.len() {
            let (mut seen, mut path) = (vec![k], vec![k]);
            // Follow the first of each that is still to see: a loop back to
            // `k` is a component that renders itself, however far round.
            let mut stack = vec![always[k].clone()];
            while let Some(top) = stack.last_mut() {
                let Some(name) = top.pop() else {
                    stack.pop();
                    path.pop();
                    continue;
                };
                let Some(j) = self.comps.iter().position(|c| c.name == name) else {
                    continue;
                };
                if j == k {
                    let way: Vec<String> = path
                        .iter()
                        .map(|&p| format!("<{}>", self.comps[p].name))
                        .collect();
                    return Err(format!(
                        "{}: {} renders itself ({}) with nothing to stop it; put it in a {{:#if}} or {{:#each}} that ends",
                        self.comps[k].rel,
                        way[0],
                        way.join(" -> ") + " -> " + &way[0]
                    ));
                }
                if !seen.contains(&j) {
                    seen.push(j);
                    path.push(j);
                    stack.push(always[j].clone());
                }
            }
        }
        Ok(())
    }

    pub(super) fn layouts(&mut self) -> Result<(), String> {
        for i in 0..self.tree.layouts.len() {
            let dir = self.tree.layouts[i].dir.clone();
            let file = dir.join("+layout.wisp");
            let (t, front) = self.parse(&file)?;
            if let Some(line) = first_await(&t.nodes) {
                return Err(format!(
                    "{}:{line}: a layout renders without waiting, so its markup cannot `.await`; await in the page's markup or `---` block",
                    self.rel(&file)
                ));
            }
            if !t.uses_children {
                return Err(format!(
                    "{}: a layout must contain {{@render children()}}",
                    self.rel(&file)
                ));
            }
            let rs = self.tree.layouts[i].has_rs.then(|| dir.join("+layout.rs"));
            let lg = self.logic(rs, &file, front, "")?;
            let where_ = lg.file.as_deref().map(|f| self.rel(f)).unwrap_or_default();
            if let Some(a) = lg.items.fns.iter().find(|f| f.action) {
                return Err(format!(
                    "{where_}:{}: layouts cannot have actions (`{}`); put it in the page",
                    a.line, a.name
                ));
            }
            if let Some(r) = lg.items.fns.iter().find(|f| f.remote.is_some()) {
                return Err(format!(
                    "{where_}:{}: a layout cannot have #[remote] functions (`{}`); put it in a page or src/remote.rs",
                    r.line, r.name
                ));
            }
            if let Some(c) = lg.items.constant("BODY_LIMIT") {
                return Err(format!(
                    "{where_}:{}: a layout's `BODY_LIMIT` does nothing; set it in the page or +server.rs whose requests it limits",
                    c.line
                ));
            }
            if let Some(c) = lg.items.constant("RUNTIME") {
                return Err(format!(
                    "{where_}:{}: a layout's `RUNTIME` does nothing; set it in the page or +server.rs that runs there",
                    c.line
                ));
            }
            if let Some(c) = ["RATE_LIMIT", "CORS", "TIMEOUT"]
                .iter()
                .find_map(|n| lg.items.constant(n))
            {
                return Err(format!(
                    "{where_}:{}: a layout's `{}` does nothing; set it in the page or +server.rs whose responses it keeps",
                    c.line, c.name
                ));
            }
            let mut guarded = Vec::new();
            let at = lg.file.clone().unwrap_or_else(|| dir.join("+layout.rs"));
            // `CACHE`, `SSR` and `PRERENDER` are the pages' below it, unless
            // a page sets its own.
            let mut opts = LayoutOpts {
                ssr: self.flag(&lg.items, "SSR", &at, &mut guarded)?,
                prerender: self.flag(&lg.items, "PRERENDER", &at, &mut guarded)?,
                ..LayoutOpts::default()
            };
            match (
                lg.items.constant("CACHE"),
                lg.items.constant("CACHE_PUBLIC"),
            ) {
                (None, None) => {}
                (Some(c), None) | (None, Some(c)) => {
                    if c.ty != "u32" || c.is_static {
                        return Err(format!(
                            "{where_}:{}: `{}` is a `{}`; make it a `const` `u32`, the seconds a response is kept, such as `const {0}: u32 = 60;`",
                            c.line, c.name, c.ty
                        ));
                    }
                    opts.cache = Some(c.name == "CACHE_PUBLIC");
                    guarded.push(cache_shim(&lg.items, &c.name));
                }
                (Some(_), Some(c)) => {
                    return Err(format!(
                        "{where_}:{}: `CACHE_PUBLIC` is `CACHE` shared with signed-in visitors too; set one of them",
                        c.line
                    ));
                }
            }
            self.layout_opts.push(opts);
            let mut code = match self.flag(&lg.items, "SIGNED_IN", &at, &mut guarded)? {
                Some(true) => "cx.signed_in()?; ".to_string(),
                _ => String::new(),
            };
            let rel = self.rel(&at);
            code.push_str(&middleware(Some(self.root), &lg.items, &rel)?);
            let guard = !code.is_empty();
            if guard {
                guarded.push(format!(
                    "pub fn __guard(cx: &mut ::wisp::Cx) -> ::wisp::Result<()> {{ {code}Ok(()) }}"
                ));
            }
            let load = lg.items.function("load");
            // The template reads `data` from a load, or names from statements.
            let reads = load.is_some() || lg.stmts.is_some();
            let user = lg.file.is_some().then(|| (format!("layout_{i}"), reads));
            // Any `async fn`, shim or helper, may be waited on: a mistake
            // errs towards waiting (see `now`).
            let waits = lg.file.is_some() && lg.items.fns.iter().any(|f| f.is_async);
            if let Some(src) = lg.file {
                let shims = load
                    .map(|f| shim(f, Shim::Load))
                    .transpose()
                    .map_err(|e| format!("{where_}:{e}"))?;
                let name = format!("layout_{i}");
                let shims = shims.into_iter().chain(guarded).collect();
                self.user_mods
                    .push(UserMod::new(name, src, lg.inline, shims, &lg.items));
            }
            let load = load.is_some();
            let data = lg.items.data_fields();
            let slots = self.slot_names(i, &t, &file)?;
            let tpl = self.add_tpl(format!("tpl_layout_{i}"), &file, Kind::Layout, t);
            tpl.user = user;
            tpl.data = data;
            tpl.slots = slots;
            tpl.stmts = lg.stmts.map(|s| (s, Vec::new()));
            let tpl = self.templates.len() - 1;
            self.model.layouts.push(model::Layout {
                tpl,
                load,
                waits,
                guard,
            });
        }
        Ok(())
    }

    /// The slots' pages are drawn inside a layout, so they render without
    /// waiting: no statements (their data is a `+page.rs`'s `load`), and
    /// they stay out of the sitemap.
    pub(super) fn slots(&mut self) -> Result<(), String> {
        for s in &self.tree.slots {
            let page = self.model.routes[s.route].page.as_ref();
            let tpl = &self.templates[page.expect("a slot is a page").tpl];
            if tpl.stmts.is_some() || tpl.load_js.is_some() {
                return Err(format!(
                    "{}: a slot page is drawn inside its layout and cannot wait: no statements in its `---` block (or `+page.js`); load its data with `fn load(cx: &mut Cx)` in a `+page.rs`",
                    tpl.rel
                ));
            }
            self.model.routes[s.route].indexed = false;
        }
        for i in &self.tree.intercepts {
            self.model.routes[i.route].indexed = false;
        }
        Ok(())
    }

    /// The slots of layout `i`, which its markup must draw, and draw only.
    pub(super) fn slot_names(
        &self,
        i: usize,
        t: &Template,
        file: &Path,
    ) -> Result<Vec<String>, String> {
        fn drawn<'a>(nodes: &'a [Node], out: &mut Vec<&'a str>) {
            for n in nodes {
                if let Node::RenderSnippet {
                    name, local: false, ..
                } = n
                {
                    out.push(name);
                }
                inside(n).into_iter().for_each(|l| drawn(l, out));
            }
        }
        let (mut calls, mut names) = (Vec::new(), Vec::new());
        drawn(&t.nodes, &mut calls);
        let mine = self.tree.slots.iter().filter(|s| s.layout == i);
        for s in mine {
            if !calls.contains(&s.name.as_str()) {
                return Err(format!(
                    "{}: the folder @{} is a slot of this layout: draw it with {{@render {}()}}",
                    self.rel(file),
                    s.name,
                    s.name
                ));
            }
            names.push(s.name.clone());
        }
        // A layout's own snippets are in its file; the rest is a slot, or nothing.
        if let Some(c) = calls.iter().find(|c| !names.iter().any(|n| n == *c)) {
            return Err(format!(
                "{}: {{@render {c}()}} draws a slot, and there is no folder @{c} in this layout's folder",
                self.rel(file)
            ));
        }
        Ok(names)
    }

    pub(super) fn error_pages(&mut self) -> Result<(), String> {
        let routes_dir = self.root.join("src").join("routes");
        for i in 0..self.tree.errors.len() {
            let dir = &self.tree.errors[i].dir;
            if *dir == routes_dir {
                self.model.root_error = Some(i);
            }
            let file = dir.join("+error.wisp");
            let (t, front) = self.parse(&file)?;
            check_no_children(&t, &self.rel(&file))?;
            if front.is_some() {
                return Err(format!(
                    "{}: an error page shows `status` and `message` (and can read `cx`); a `---` block of Rust is for pages and layouts",
                    self.rel(&file)
                ));
            }
            self.add_tpl(format!("tpl_error_{i}"), &file, Kind::Error, t);
            self.model.errors.push(model::ErrorPage {
                tpl: self.templates.len() - 1,
                layouts: self.tree.errors[i].layouts.clone(),
            });
        }
        Ok(())
    }

    /// A route's `BODY_LIMIT`, checked: a `usize`, set once.
    pub(super) fn body_limit(
        &self,
        route: &mut model::Route,
        items: &rust_scan::Items,
        file: &Path,
        module: String,
        page: &str,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        let Some(c) = items.constant("BODY_LIMIT") else {
            return Ok(());
        };
        let at = |msg: &str| format!("{}:{}: {msg}", self.rel(file), c.line);
        if c.ty != "usize" {
            return Err(at(&format!(
                "`BODY_LIMIT` is a `{}`; make it a `usize`, such as `20 * wisp::MB`",
                c.ty
            )));
        }
        set_once(&mut route.body_limit, module, "BODY_LIMIT", page).map_err(|e| at(&e))?;
        shims.push("pub const BODY_LIMIT: usize = super::BODY_LIMIT;".into());
        Ok(())
    }

    /// A page's `const NAME: bool = true;` (or `false`), checked: a `bool`
    /// literal, which the build reads. The shim keeps it used.
    pub(super) fn flag(
        &self,
        items: &rust_scan::Items,
        name: &str,
        file: &Path,
        shims: &mut Vec<String>,
    ) -> Result<Option<bool>, String> {
        let Some(c) = items.constant(name) else {
            return Ok(None);
        };
        let value = match c.value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        let (Some(value), "bool", false) = (value, c.ty.as_str(), c.is_static) else {
            return Err(format!(
                "{}:{}: the build reads `{name}`: write `const {name}: bool = true;` or `false`, as a literal",
                self.rel(file),
                c.line
            ));
        };
        shims.push(format!("const _: bool = super::{name};"));
        Ok(Some(value))
    }

    /// A route's `RUNTIME`, checked; the shim keeps it used. `wisp build`
    /// reads it (see `routes::edge`).
    pub(super) fn runtime(
        &self,
        items: &rust_scan::Items,
        file: &Path,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        if let Err((line, msg)) = crate::routes::edge(items) {
            return Err(format!("{}:{line}: {msg}", self.rel(file)));
        }
        if items.constant("RUNTIME").is_some() {
            shims.push("const _: ::wisp::Runtime = super::RUNTIME;".into());
        }
        Ok(())
    }

    /// A route's `CACHE` (or `CACHE_PUBLIC`), checked: a `u32`, one of the
    /// two, set once.
    pub(super) fn cache(
        &self,
        route: &mut model::Route,
        items: &rust_scan::Items,
        file: &Path,
        module: String,
        page: &str,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        let (c, public) = match (items.constant("CACHE"), items.constant("CACHE_PUBLIC")) {
            (None, None) => return Ok(()),
            (Some(c), None) => (c, false),
            (None, Some(c)) => (c, true),
            (Some(_), Some(c)) => {
                return Err(format!(
                    "{}:{}: `CACHE_PUBLIC` is `CACHE` shared with signed-in visitors too; set one of them",
                    self.rel(file),
                    c.line
                ));
            }
        };
        let at = |msg: &str| format!("{}:{}: {msg}", self.rel(file), c.line);
        if c.ty != "u32" || c.is_static {
            return Err(at(&format!(
                "`{}` is a `{}`; make it a `const` `u32`, the seconds a response is kept, such as `const {0}: u32 = 60;`",
                c.name, c.ty
            )));
        }
        let cache = model::Cache { module, public };
        set_once(&mut route.cache, cache, &c.name, page).map_err(|e| at(&e))?;
        shims.push(cache_shim(items, &c.name));
        Ok(())
    }

    /// Pages and endpoints.
    pub(super) fn routes(&mut self) -> Result<(), String> {
        // Each `+server.rs` once, though it may serve two routes.
        let mut servers: Vec<ServerFile> = Vec::new();
        for i in 0..self.tree.routes.len() {
            let r = &self.tree.routes[i];
            let error = r.error;
            let mut route = model::Route {
                pattern: r.pattern(),
                page: None,
                server: None,
                layouts: r.layouts.clone(),
                error,
                body_limit: None,
                uploads: None,
                guard: None,
                timeout: None,
                cache: None,
                indexed: r.page
                    && !(r.dir.strip_prefix(self.root).unwrap_or(&r.dir).components())
                        .any(|c| c.as_os_str() == "(private)"),
            };
            if r.page {
                self.page(i, &mut route)?;
            }
            if self.tree.routes[i].server {
                self.server(i, &mut route, &mut servers)?;
            }
            // A page's `CACHE` from a layout above it, when nothing sets one
            // (a streamed page is not kept whole).
            if route.cache.is_none()
                && route.page.as_ref().is_some_and(|p| !p.streams)
                && let Some((l, public)) = (self.tree.routes[i].layouts.iter().rev())
                    .find_map(|l| self.layout_opts[*l].cache.map(|p| (*l, p)))
            {
                route.cache = Some(model::Cache {
                    module: format!("layout_{l}"),
                    public,
                });
            }
            self.model.routes.push(route);
        }
        Ok(())
    }

    /// The `+page.wisp` of route `i`, and its Rust.
    pub(super) fn page(&mut self, i: usize, route: &mut model::Route) -> Result<(), String> {
        let r = &self.tree.routes[i];
        let (dir, page_rs, page_js, md) = (r.dir.clone(), r.page_rs, r.page_js, r.md.clone());
        let name = r.page_file.clone();
        let file = md.clone().unwrap_or_else(|| dir.join(&name));
        // Its Rust first: its actions' fields get the browser's checks.
        let src = match md {
            Some(_) => {
                // A layout's `<title>` stands for its `title` (read through
                // `wisp::pages`, which it can wrap).
                let titled = (r.layouts.iter())
                    .any(|&l| self.templates[self.model.layouts[l].tpl].t.has_title());
                let m = crate::markdown::page(&self.read(&file)?, &self.comps, titled)
                    .map_err(|e| format!("{}:{e}", self.rel(&file)))?;
                self.md_pages.push((route.pattern.clone(), m.fields));
                m.wisp
            }
            None => self.read(&file)?,
        };
        route.indexed &= !noindex(&src);
        let (front, markup) =
            crate::split_front(&src).map_err(|e| format!("{}:{e}", self.rel(&file)))?;
        let mut lg = self.logic(
            page_rs.then(|| dir.join("+page.rs")),
            &file,
            front.clone(),
            &markup,
        )?;
        // A form that posts to `?/name` needs the page's action of that name.
        for name in rust_scan::posted_to(&markup) {
            if !lg.items.fns.iter().any(|f| f.action && f.name == name) {
                let line = src
                    .find(&format!("?/{name}"))
                    .map_or(1, |at| src[..at].matches('\n').count() + 1);
                return Err(format!(
                    "{}:{line}: a form posts to `?/{name}`, and this page has no action `{name}`: write `#[action] fn {name}(…)` in it",
                    self.rel(&file)
                ));
            }
        }
        let fields = rules::fields(&lg.items, &self.tree.routes[i].params(), &self.shared);
        let rs = lg.file.clone().unwrap_or_else(|| dir.join("+page.rs"));
        let mut shims = Vec::new();
        // What the layouts above set stands where the page sets nothing.
        let up = |f: fn(&LayoutOpts) -> Option<bool>| {
            (self.tree.routes[i].layouts.iter().rev()).find_map(|l| f(&self.layout_opts[*l]))
        };
        let own_cache = lg
            .items
            .constant("CACHE")
            .or(lg.items.constant("CACHE_PUBLIC"))
            .is_some();
        let ssr = self
            .flag(&lg.items, "SSR", &rs, &mut shims)?
            .or(up(|o| o.ssr));
        let drawn = ssr == Some(false);
        let own = self.flag(&lg.items, "PRERENDER", &rs, &mut shims)?;
        let prerender = own.or(up(|o| o.prerender).filter(|_| !own_cache)) == Some(true);
        let line = lg.items.constant("PRERENDER").map_or(1, |c| c.line);
        let (mut t, _) = self.markup(&file, &markup, front, &fields, drawn)?;
        check_no_children(&t, &self.rel(&file))?;
        // `.await` in the markup: statements, after the block's own.
        let mut lets = Vec::new();
        hoist_awaits(&mut t.nodes, &mut lets).map_err(|e| format!("{}:{e}", self.rel(&file)))?;
        if let (Some((_, line)), Some(load)) = (lets.first(), lg.items.function("load")) {
            return Err(format!(
                "{}:{line}: `.await` in markup runs with the page's statements, and this page has `fn load` (line {}); await in `load`",
                self.rel(&file),
                load.line
            ));
        }
        lg.stmts = with_lets(lg.stmts, &lets);
        let streams = awaits_in(&t.nodes);
        if streams {
            self.streamed(&file, &t, &lg.items)?;
            t.hashes.push(crate::csp::hash(AWAIT_JS));
        }
        // The page comes first: nothing set these before it.
        self.runtime(&lg.items, &rs, &mut shims)?;
        self.body_limit(route, &lg.items, &rs, format!("page_{i}"), "", &mut shims)?;
        self.cache(route, &lg.items, &rs, format!("page_{i}"), "", &mut shims)?;
        let (rel, module) = (self.rel(&rs), format!("page_{i}"));
        let guard = guards(Some(self.root), &lg.items, &rel, &mut shims)?;
        if !guard.is_empty() {
            shims.push(format!(
                "pub fn __guard(cx: &mut ::wisp::Cx) -> ::wisp::Result<()> {{ {guard}Ok(()) }}"
            ));
            route.guard = Some(module.clone());
        }
        if timeout(&lg.items, &rel, &mut shims)? {
            route.timeout = Some(module);
        }
        if let Some(sum) =
            upload_sizes(&lg.items.fns).map_err(|e| format!("{}:{e}", self.rel(&rs)))?
        {
            route.uploads = Some(format!("page_{i}"));
            shims.push(format!("pub const UPLOADS: usize = {sum};"));
        }
        let data = lg.items.data_fields();
        let tables = lg.items.tables();
        let types = lg.items.types;
        let fns = lg.items.fns;
        let has_load = fns.iter().any(|f| f.name == "load");
        if lg.stmts.is_some() && page_js.is_some() {
            return Err(format!(
                "{}: +page.js gets the page's `data`, which comes from a `load`; with a `---` block of statements there is none. Move them into `fn load`.",
                self.rel(&file)
            ));
        }
        // A `let` for each route parameter the statements or markup name.
        let binds: Vec<String> = self.tree.routes[i]
            .segs
            .iter()
            .filter_map(|s| {
                let (n, how) = match s {
                    Seg::Static(_) => return None,
                    Seg::Param(n, m) if m.as_deref() == Some("int") => {
                        (n, ": u64 = cx.param(@).parse().unwrap_or_default()")
                    }
                    Seg::Param(n, _) | Seg::Rest(n) => (n, " = cx.param(@).to_string()"),
                    Seg::Optional(n, m) if m.as_deref() == Some("int") => {
                        (n, ": Option<u64> = cx.param(@).parse().ok()")
                    }
                    Seg::Optional(n, _) => (
                        n,
                        ": Option<String> = Some(cx.param(@)).filter(|s| !s.is_empty()).map(str::to_string)",
                    ),
                };
                let raw = if ty::is_keyword(n) { "r#" } else { "" };
                names_word(&src, n).then(|| format!("let {raw}{n}{};", how.replace('@', &lit(n))))
            })
            .collect();
        let at = |f: &FnItem, msg: String| format!("{}:{}: {msg}", self.rel(&rs), f.line);
        if let Some(f) = fns.iter().find(|f| f.action && f.name == "load") {
            return Err(at(
                f,
                format!("`{}` cannot be both load and an action", f.name),
            ));
        }
        if let Some(f) = fns.iter().find(|f| f.name == "entries") {
            if !f.params.is_empty() || f.is_async || f.action {
                return Err(at(
                    f,
                    "`entries` is `fn entries() -> Vec<...>`: no parameters, not async, not an action".into(),
                ));
            }
            shims.push("pub fn entries() -> Vec<Vec<String>> { super::entries().into_iter().map(::wisp::Entry::params).collect() }".into());
        }
        if let Some(f) = fns
            .iter()
            .find(|f| f.action && f.returns_kind() == Returns::Other)
        {
            return Err(at(
                f,
                format!(
                    "action `{}` returns `{}`. An action returns nothing (or `Result<()>`, so it can use `?`), \
                     or a `Response` to send instead of the page (or `Option<Response>`, to send one only sometimes).",
                    f.name, f.returns
                ),
            ));
        }
        let mut remotes = Vec::new();
        for f in &fns {
            if f.remote.is_some() {
                if f.action || f.name == "load" {
                    return Err(at(
                        f,
                        format!(
                            "`{}` cannot be both #[remote] and {}",
                            f.name,
                            if f.action { "an action" } else { "load" }
                        ),
                    ));
                }
                shims.push(remote_shim(f).map_err(|e| format!("{}:{e}", self.rel(&rs)))?);
                let mut types = types.clone();
                types.extend(self.shared.iter().cloned());
                remotes.push(RemoteFn {
                    f: f.clone(),
                    module: format!("page_{i}"),
                    rel: self.rel(&rs),
                    types,
                });
                continue;
            }
            let kind = match f.name.as_str() {
                _ if f.action => Shim::Answer,
                "load" => Shim::Load,
                _ => continue,
            };
            shims.push(shim(f, kind).map_err(|e| format!("{}:{e}", self.rel(&rs)))?);
        }
        self.remotes.extend(remotes);
        if prerender {
            let stmts = lg.stmts.as_deref();
            self.prerender(i, route, line, stmts, &markup, &fns, &rs, &mut shims)?;
        }
        let reads = has_load || lg.stmts.is_some();
        let user = lg.file.is_some().then(|| (format!("page_{i}"), reads));
        if lg.file.is_some() {
            self.user_mods.push(UserMod {
                name: format!("page_{i}"),
                file: rs,
                inline: lg.inline,
                shims,
                tables,
                configs: Vec::new(),
            });
        }
        let tpl = self.add_tpl(format!("tpl_page_{i}"), &file, Kind::Page, t);
        tpl.user = user;
        tpl.data = data;
        tpl.load_js = page_js.map(|f| dir.join(f));
        let waits = streams
            || fns.iter().any(|f| f.is_async)
            || lg.stmts.as_deref().is_some_and(rust_scan::may_wait);
        // A page with no Rust still reads its route parameters.
        tpl.stmts = match lg.stmts {
            Some(s) => Some((s, binds)),
            None if !has_load && !binds.is_empty() => Some((String::new(), binds)),
            None => None,
        };
        route.page = Some(model::Page {
            module: format!("page_{i}"),
            tpl: self.templates.len() - 1,
            fns,
            waits,
            drawn,
            prerender,
            streams,
        });
        Ok(())
    }

    /// A page with `{#await}` (`t`, of `file`), checked: not kept whole by
    /// `CACHE`, no browser code in its branches.
    pub(super) fn streamed(
        &self,
        file: &Path,
        t: &Template,
        items: &rust_scan::Items,
    ) -> Result<(), String> {
        let rel = self.rel(file);
        if items.constant("CACHE").is_some() {
            return Err(format!(
                "{rel}: a page with `{{#await}}` is streamed as its answers come, and `CACHE` keeps a whole answer: drop one"
            ));
        }
        match bad_await(&t.nodes) {
            Some((line, msg)) => Err(format!("{rel}:{line}: {msg}")),
            None => Ok(()),
        }
    }

    /// A page with `const PRERENDER: bool = true;`, checked: it reads
    /// nothing of the request (no `cx` in its statements, markup or
    /// `load`), and a route with parameters has `entries`. Until `wisp
    /// build` renders it, each worker keeps its first render for good, as
    /// `CACHE_PUBLIC` would.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prerender(
        &self,
        i: usize,
        route: &mut model::Route,
        line: usize,
        stmts: Option<&str>,
        markup: &str,
        fns: &[FnItem],
        rs: &Path,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        let at = |msg: &str| format!("{}:{line}: {msg}", self.rel(rs));
        let load_cx = (fns.iter().find(|f| f.name == "load"))
            .is_some_and(|f| f.implicit_cx || f.params.iter().any(|(_, ty)| ty.contains("Cx")));
        if load_cx || names_word(markup, "cx") || stmts.is_some_and(|s| names_word(s, "cx")) {
            return Err(at(
                "this page is prerendered (`const PRERENDER: bool = true;`): rendered once for every request, \
                 it cannot read the request. Drop `cx` from its statements, markup and `load`, or drop `PRERENDER`",
            ));
        }
        let r = &self.tree.routes[i];
        let required = (r.segs.iter()).any(|s| matches!(s, Seg::Param(..) | Seg::Rest(_)));
        if required && !fns.iter().any(|f| f.name == "entries") {
            return Err(at(
                "this prerendered page has parameters: say which pages to render with \
                 `fn entries() -> Vec<&'static str> { vec![\"a\", \"b\"] }`",
            ));
        }
        if route.cache.is_some() {
            return Err(at(
                "a prerendered page is kept for good: `PRERENDER` and `CACHE` do not go together",
            ));
        }
        route.cache = Some(model::Cache {
            module: format!("page_{i}"),
            public: true,
        });
        shims.push(
            "pub const CACHE: u32 = u32::MAX;
pub const MORE: ::wisp::rt::CacheMore = ::wisp::rt::CacheMore::NONE;"
                .into(),
        );
        Ok(())
    }

    /// The `+server.rs` of route `i`, read once for the two routes it may
    /// serve (`servers` has those read so far).
    pub(super) fn server(
        &mut self,
        i: usize,
        route: &mut model::Route,
        servers: &mut Vec<ServerFile>,
    ) -> Result<(), String> {
        let r = &self.tree.routes[i];
        let (page, member) = (r.page, r.member);
        let page_file = if r.page_rs { "+page.rs" } else { "+page.wisp" };
        // A `+server.rs`, or else the `mod server` of the page's block.
        let rs = r.dir.join("+server.rs");
        let block = match rs.is_file() {
            true => None,
            false => crate::block_server(&r.dir.join(&r.page_file)).map(|s| s.0),
        };
        let file = match block {
            Some(_) => r.dir.join(&r.page_file),
            None => rs,
        };
        let k = match servers.iter().position(|s| s.file == file) {
            Some(k) => {
                let (sf, at) = (&servers[k], |e: String| format!("{}: {e}", self.rel(&file)));
                if sf.limit {
                    set_once(
                        &mut route.body_limit,
                        sf.server.module.clone(),
                        "BODY_LIMIT",
                        page_file,
                    )
                    .map_err(at)?;
                }
                if sf.timeout {
                    let m = sf.server.module.clone();
                    set_once(&mut route.timeout, m, "TIMEOUT", page_file).map_err(at)?;
                }
                if let Some(public) = sf.cache {
                    let cache = model::Cache {
                        module: sf.server.module.clone(),
                        public,
                    };
                    set_once(&mut route.cache, cache, "CACHE", page_file).map_err(at)?;
                }
                k
            }
            None => {
                let items = match &block {
                    Some(src) => {
                        let at = |e: String| format!("{}:{e}", self.rel(&file));
                        let items = rust_scan::scan(src).map_err(at)?;
                        items.check().map_err(at)?;
                        items
                    }
                    None => self.scan(&file)?,
                };
                if let Some(r) = items.fns.iter().find(|f| f.remote.is_some()) {
                    return Err(format!(
                        "{}:{}: `{}` is #[remote], which browser code calls; +server.rs has endpoints. Put it in a page or src/remote.rs",
                        self.rel(&file),
                        r.line,
                        r.name
                    ));
                }
                let module = format!("server_{i}");
                let mut shims = Vec::new();
                self.runtime(&items, &file, &mut shims)?;
                self.body_limit(route, &items, &file, module.clone(), page_file, &mut shims)?;
                self.cache(route, &items, &file, module.clone(), page_file, &mut shims)?;
                let cache = (route.cache.as_ref())
                    .filter(|c| c.module == module)
                    .map(|c| c.public);
                let segs = &self.tree.routes[i].segs;
                let segs = &segs[..segs.len() - usize::from(member)];
                let rel = self.rel(&file);
                let has_timeout = timeout(&items, &rel, &mut shims)?;
                if has_timeout {
                    let m = module.clone();
                    set_once(&mut route.timeout, m, "TIMEOUT", page_file)
                        .map_err(|e| format!("{rel}: {e}"))?;
                }
                let guard = guards(Some(self.root), &items, &rel, &mut shims)?;
                let (handlers, mut before) =
                    server_handlers(&items, segs, &mut shims).map_err(|e| format!("{rel}:{e}"))?;
                before |= add_guard(&mut shims, &guard, before);
                let waits = items.fns.iter().any(|f| f.is_async);
                // Unsafe methods refuse another site's `Origin`, unless the
                // file takes other sites by `CORS`, or says `CSRF = false`.
                let csrf = self.flag(&items, "CSRF", &file, &mut shims)? != Some(false)
                    && items.constant("CORS").is_none();
                (self.user_mods).push(UserMod::new(
                    module.clone(),
                    file.clone(),
                    block.clone(),
                    shims,
                    &items,
                ));
                servers.push(ServerFile {
                    file: file.clone(),
                    limit: route.body_limit.as_ref() == Some(&module),
                    timeout: has_timeout,
                    cache,
                    server: model::Server {
                        module,
                        handlers,
                        before,
                        types: items.types.into(),
                        waits,
                        csrf,
                    },
                });
                servers.len() - 1
            }
        };
        let sf = &servers[k].server;
        let handlers: Vec<model::Handler> = (sf.handlers.iter())
            .filter(|h| h.member == member)
            .cloned()
            .collect();
        let has_actions = route
            .page
            .as_ref()
            .is_some_and(|p| p.actions().next().is_some());
        for h in &handlers {
            if page && (h.op.method == "get" || (h.op.method == "post" && has_actions)) {
                return Err(format!(
                    "{}: `{}` conflicts with the page in the same directory",
                    self.rel(&file),
                    h.shim
                ));
            }
        }
        route.server = Some(model::Server {
            module: sf.module.clone(),
            handlers,
            before: sf.before,
            types: sf.types.clone(),
            waits: sf.waits,
            csrf: sf.csrf,
        });
        Ok(())
    }

    /// `src/hooks.rs`, the param matchers, the app's own modules; then,
    /// with every template read, that the components they use exist.
    pub(super) fn app_files(&mut self) -> Result<(), String> {
        (self.hooks, self.before_waits) = hooks(self.root)?;
        for (m, file) in &self.tree.matchers {
            if m == "locale" && file.is_none() && self.i18n.is_none() {
                return Err(
                    "src/routes: `[[lang=locale]]` matches the app's locales, and it has none: add src/locales/en.json"
                        .into(),
                );
            }
            let Some(file) = file else { continue };
            let at = |e: String| format!("{}:{e}", self.rel(file));
            let items = rust_scan::scan(&self.read(file)?).map_err(at)?;
            items.check_inner().map_err(at)?;
            if items.function("matches").is_none() {
                return Err(format!(
                    "{}: a param matcher is `fn matches(s: &str) -> bool`, which this file does not have",
                    self.rel(file)
                ));
            }
            let shims = vec!["pub fn matches(s: &str) -> bool { super::matches(s) }".into()];
            let name = format!("param_{m}");
            self.user_mods
                .push(UserMod::new(name, file.clone(), None, shims, &items));
        }
        let (mods, remotes) = app_mods(self.root)?;
        self.mods = mods;
        for mut r in remotes {
            r.types.clone_from(&self.shared);
            self.remotes.push(r);
        }
        self.check_remotes()?;
        for t in &self.templates {
            check_components(&t.t.nodes, &t.t, &self.comps, &t.rel, false)?;
        }
        Ok(())
    }

    /// Browser code calls a `#[remote]` function by its name alone, so each
    /// name is the app's once, and one no script's own names hide.
    pub(super) fn check_remotes(&self) -> Result<(), String> {
        for (k, r) in self.remotes.iter().enumerate() {
            let name = r.f.name.as_str();
            let at = format!("{}:{}", r.rel, r.f.line);
            if let Some(o) = self.remotes[..k].iter().find(|o| o.f.name == name) {
                return Err(format!(
                    "{at}: there is already a #[remote] fn `{name}` ({}:{}); browser code calls them by name, so rename one",
                    o.rel, o.f.line
                ));
            }
            let helper = HELPERS.split(',').any(|h| h.trim() == name);
            if helper
                || js::is_reserved(name)
                || js::is_global(name)
                || matches!(name, "define" | "env" | "data")
                || name.starts_with("__")
            {
                return Err(format!(
                    "{at}: browser code calls #[remote] fn `{name}` by its name, which JavaScript or Wisp has already; rename it"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn has_hook(&self, name: &str) -> bool {
        self.hooks.as_ref().is_some_and(|h| h.has(name))
    }

    /// Browser JavaScript: `src/lib/**/*.js`, imported as `$lib/…` and
    /// served under one hash (so every importer names a file by the same
    /// URL), each page's `+page.js`, and the modules of templates.
    pub(super) fn browser(&self) -> Result<Web, String> {
        let mut lib = Vec::new();
        let lib_dir = self.root.join("src").join("lib");
        if lib_dir.is_dir() {
            list_files(&lib_dir, &mut lib)?;
        }
        lib.retain(|f| f.extension().is_some_and(|e| e == "js" || e == "ts"));
        lib.sort();
        let mut lib_src = Vec::new();
        for f in &lib {
            let path = f
                .strip_prefix(&lib_dir)
                .unwrap_or(f)
                .to_string_lossy()
                .replace('\\', "/");
            lib_src.push((path, self.read(f)?));
        }
        // The `#[remote]` functions' module, which scripts import from.
        let remote = (!self.remotes.is_empty()).then(|| {
            let src = remote_js(&self.remotes);
            let source = if self.release { js::runtime(&src) } else { src };
            JsFile {
                path: REMOTE_JS_PATH.into(),
                hash: image::hash(source.as_bytes()),
                source,
                file: None,
            }
        });
        let lib_hash = {
            let mut h = Vec::new();
            // The URL of `wisp:remote`, which they may import.
            if let Some(r) = &remote {
                h.extend_from_slice(r.hash.as_bytes());
            }
            for (p, src) in &lib_src {
                h.extend_from_slice(p.as_bytes());
                h.push(0);
                h.extend_from_slice(src.as_bytes());
                h.push(0);
            }
            // The `env.PUBLIC_X` they may read, filled in.
            for (k, v) in &self.env {
                h.extend_from_slice(format!("{k}={v}\0").as_bytes());
            }
            image::hash(&h)
        };
        let specs = Specs {
            remote: remote.as_ref().map(|f| format!("{}?v={}", f.path, f.hash)),
            lib: lib_src.iter().map(|(p, _)| p.clone()).collect(),
            lib_hash,
            npm: Npm::new(
                npm::deps(self.root)?,
                self.release.then(|| self.root.join(".wisp").join("npm")),
            ),
        };
        // The runtime's less used half, which modules that use it import.
        let extra = {
            let src = rewrite_specifiers(EXTRA_JS, &specs, None)?;
            let source = if self.release { js::runtime(&src) } else { src };
            let hash = image::hash(source.as_bytes());
            JsFile {
                path: EXTRA_JS_PATH.into(),
                hash,
                source,
                file: None,
            }
        };
        let extra_url = format!("{}?v={}", extra.path, extra.hash);
        // A lib file that makes a `persisted` store imports it too (last, so
        // its lines stay). With maps, it ends naming its map, served at
        // `path.map`.
        let maps = self.maps;
        let lib_file = |src: &str,
                        dir: Option<&str>,
                        rel: &str,
                        path: &str,
                        files: &mut Vec<JsFile>| {
            let code = javascript(src, rel)?;
            let code = js::public_env(&code, &|n| var(&self.env, n))
                .map_err(|(off, msg)| format!("{rel}:{}: {msg}", place(src, off)))?;
            // A page sends the messages its own scripts show.
            if self.i18n.is_some() {
                let no = |_: &str| {
                    Err("t('…') shows a message in a .wisp file's script or markup; pass the text to this file from there".to_string())
                };
                js::translate(&code, &no)
                    .map_err(|(off, msg)| format!("{rel}:{}: {msg}", place(src, off)))?;
            }
            let mut s =
                rewrite_specifiers(&code, &specs, dir).map_err(|e| format!("{rel}: {e}"))?;
            let mut added = 0;
            if js::tokens(&code)
                .iter()
                .any(|t| !t.member && t.text(&code) == "persisted")
            {
                s.push_str(&format!("\nimport {};\n", js_str(&extra_url)));
                added = 2;
            }
            if maps {
                let name = path.rsplit('/').next().unwrap_or(path);
                s.push_str(&sourcemap::comment(name));
                files.push(map_file(path, name, rel, src, &sourcemap::same(src, added)));
            }
            Ok::<_, String>(s)
        };
        let mut js_files: Vec<JsFile> = remote.into_iter().collect();
        for (p, src) in &lib_src {
            let dir = format!("lib/{}", p.rfind('/').map_or("", |i| &p[..i]));
            let path = format!("{MODULES}lib/{p}");
            let source = lib_file(
                src,
                Some(&dir),
                &format!("src/lib/{p}"),
                &path,
                &mut js_files,
            )?;
            js_files.push(JsFile {
                path,
                hash: specs.lib_hash.clone(),
                source,
                file: None,
            });
        }

        // The modules of templates.
        let remote_names: Vec<String> = self.remotes.iter().map(|r| r.f.name.clone()).collect();
        let as_client: std::collections::HashSet<String> = self
            .templates
            .iter()
            .flat_map(|t| client_uses(&t.t))
            .collect();
        let mut clients: Vec<Option<Client>> = Vec::with_capacity(self.templates.len());
        for (k, t) in self.templates.iter().enumerate() {
            let load = match &t.load_js {
                Some(f) => {
                    let path = format!("{MODULES}t{}.load.js", t.id);
                    let source = lib_file(
                        &self.read(f)?,
                        Some(&src_dir(&self.rel(f))),
                        &self.rel(f),
                        &path,
                        &mut js_files,
                    )?;
                    let hash = image::hash(source.as_bytes());
                    let url = format!("{path}?v={hash}");
                    js_files.push(JsFile {
                        path,
                        hash,
                        source,
                        file: None,
                    });
                    Some(url)
                }
                None => None,
            };
            // Components are the first templates. One built as a custom
            // element is drawn by the browser.
            if let (Some((_, line)), false) = (&t.t.element, t.kind == Kind::Component) {
                return Err(format!(
                    "{}:{line}: {{@element}} is for components (src/components)",
                    t.rel
                ));
            }
            let is_client = t.kind == Kind::Component
                && (as_client.contains(&self.comps[k].name) || t.t.element.is_some());
            let cx = ClientCx {
                comps: &self.comps,
                templates: &self.templates,
                as_client: is_client,
                specs: &specs,
                load,
                release: self.release,
                maps: self.maps,
                env: &self.env,
                i18n: self.i18n.as_ref(),
                extra: &extra_url,
                remotes: &remote_names,
            };
            let c = client(t, &cx)?;
            if let Some(c) = c.as_ref().filter(|_| self.maps) {
                let file = self.read(&self.root.join(&t.rel))?;
                js_files.push(map_file(
                    &c.path(),
                    &format!("{}.js", c.id),
                    &t.rel,
                    &file,
                    &c.lines,
                ));
            }
            clients.push(c);
        }
        if clients
            .iter()
            .flatten()
            .any(|c| c.source.contains(&extra_url))
        {
            js_files.push(extra);
        }
        // A module imports the modules of the components it renders by URLs
        // whose hash covers every module it can reach, so a change in any of
        // them changes the URL. Components may render each other (or
        // themselves) in a circle: a hash over the set needs no order.
        let finals: Vec<String> = (0..clients.len())
            .map(|k| {
                let mut seen = vec![false; clients.len()];
                seen[k] = true;
                let mut stack: Vec<usize> =
                    clients[k].as_ref().map_or(Vec::new(), |c| c.uses.clone());
                while let Some(ci) = stack.pop() {
                    if !std::mem::replace(&mut seen[ci], true) {
                        stack.extend(clients[ci].iter().flat_map(|c| &c.uses));
                    }
                }
                let all: String = (0..clients.len())
                    .filter(|&j| seen[j])
                    .filter_map(|j| clients[j].as_ref().map(|c| c.hash.as_str()))
                    .collect();
                image::hash(all.as_bytes())
            })
            .collect();
        for (k, c) in clients.iter_mut().enumerate() {
            let Some(c) = c else { continue };
            let url = |ci: usize| {
                c.uses
                    .contains(&ci)
                    .then(|| format!("{MODULES}t{}.js?v={}", self.templates[ci].id, finals[ci]))
            };
            c.source = link_comps(&c.source, url);
            c.hash.clone_from(&finals[k]);
        }
        self.elements(&clients, &specs, &mut js_files)?;
        let mut web = Web { clients, js_files };
        let mut npm_src: Vec<(String, String)> = Vec::new();
        // A release build serves the npm modules imported, and what they
        // import, from .wisp/npm.
        if self.release {
            let dir = self.root.join(".wisp").join("npm");
            let roots: Vec<String> = web
                .imports(NPM_MODULES)
                .into_iter()
                .map(|p| format!("/{p}"))
                .collect();
            let files = npm::walk(&dir, &roots, |p| Err(npm::absent(&p[0])))?;
            for (f, src) in files {
                web.js_files.push(JsFile {
                    path: format!("{NPM_MODULES}{f}"),
                    hash: image::hash(src.as_bytes()),
                    source: String::new(),
                    file: Some(dir.join(&f)),
                });
                npm_src.push((format!("{NPM_MODULES}{f}"), src));
            }
        }
        // Each module's static imports, preloaded with it.
        let mut sources: Vec<(String, &str)> = (web.js_files.iter())
            .filter(|f| f.file.is_none())
            .map(|f| (format!("{}?v={}", f.path, f.hash), f.source.as_str()))
            .collect();
        sources.extend(
            (web.clients.iter().flatten())
                .map(|c| (format!("{}?v={}", c.path(), c.hash), c.source.as_str())),
        );
        sources.extend(npm_src.iter().map(|(u, s)| (u.clone(), s.as_str())));
        let preloads: Vec<Vec<String>> = (web.clients.iter())
            .map(|c| {
                c.as_ref()
                    .map_or(Vec::new(), |c| static_imports(&c.source, &sources))
            })
            .collect();
        for (c, p) in web.clients.iter_mut().zip(preloads) {
            if let Some(c) = c {
                c.preload = p;
            }
        }
        Ok(web)
    }

    /// The modules of the components built as custom elements
    /// (`{@element "x-card"}` → `/_app/c/el/x-card.js`), and the runtime
    /// they share (`/_app/c/el.js`), onto `files`. Each defines its element
    /// from its component's module, its props' kinds and literal defaults,
    /// and the scoped CSS of the components it draws, for its shadow root.
    pub(super) fn elements(
        &self,
        clients: &[Option<Client>],
        specs: &Specs,
        files: &mut Vec<JsFile>,
    ) -> Result<(), String> {
        let mut tags: Vec<&str> = Vec::new();
        let mut runtime: Option<String> = None;
        for (k, t) in self.templates.iter().enumerate() {
            let (Some((tag, line)), Some(c)) = (&t.t.element, &clients[k]) else {
                continue;
            };
            let at = |msg: String| format!("{}:{line}: {msg}", t.rel);
            if tags.contains(&tag.as_str()) {
                return Err(at(format!(
                    "<{tag}> is the element of another component too"
                )));
            }
            tags.push(tag);
            if !template::client_renderable(&t.t.nodes) {
                return Err(at(format!(
                    "<{tag}> is drawn by the browser, but its markup has server code ({{…}} or a {{#…}} block): \
                     show props with {{:prop}} and use {{:#if}} and {{:#each}}"
                )));
            }
            let mut props = Vec::new();
            for d in &self.comps[k].props {
                let default = match d.default.as_deref() {
                    None => "null".to_string(),
                    Some(src) => element_default(src).ok_or_else(|| {
                        at(format!(
                            "<{tag}>'s prop `{}` has a default the browser cannot know (`{src}`): make it a literal",
                            d.name
                        ))
                    })?,
                };
                props.push(format!(
                    "{}: [\"{}\", {default}]",
                    js_str(&d.name),
                    element_kind(&d.ty)
                ));
            }
            // Its CSS, and that of the components it draws, at any depth.
            let mut seen = vec![k];
            let mut i = 0;
            while i < seen.len() {
                for &u in clients[seen[i]].iter().flat_map(|c| &c.uses) {
                    if !seen.contains(&u) {
                        seen.push(u);
                    }
                }
                i += 1;
            }
            seen.sort_unstable();
            let css: Vec<&str> = (seen.iter())
                .filter_map(|&j| self.templates[j].t.style.as_deref())
                .collect();
            let runtime = match &runtime {
                Some(url) => url.clone(),
                None => {
                    let src = rewrite_specifiers(wisp_shared::ELEMENT_JS, specs, None)?;
                    let source = if self.release { js::runtime(&src) } else { src };
                    let hash = image::hash(source.as_bytes());
                    let url = format!("{ELEMENT_JS_PATH}?v={hash}");
                    files.push(JsFile {
                        path: ELEMENT_JS_PATH.into(),
                        hash,
                        source,
                        file: None,
                    });
                    runtime.insert(url).clone()
                }
            };
            let source = format!(
                "import {{ element }} from {};\nimport {};\nelement({}, {}, {{ {} }}, {});\n",
                js_str(&runtime),
                js_str(&format!("{}?v={}", c.path(), c.hash)),
                js_str(tag),
                js_str(&c.id),
                props.join(", "),
                js_str(&css.join("\n"))
            );
            files.push(JsFile {
                path: format!("{ELEMENTS}{tag}.js"),
                hash: image::hash(source.as_bytes()),
                source,
                file: None,
            });
        }
        Ok(())
    }

    /// The service worker and manifest (`pwa`), if the app has either. A
    /// release build's worker lists its browser files (but the custom
    /// elements', for other sites) and `static/`'s, by the URLs pages use;
    /// `css` is `App::CSS`.
    pub(super) fn pwa(
        &self,
        web: &Web,
        assets: &Assets,
        css: Option<&str>,
    ) -> Result<Option<crate::pwa::Pwa>, String> {
        let mut build = Vec::new();
        let mut files = Vec::new();
        if self.release {
            build.extend(css.map(|v| format!("{APP_CSS_PATH}?v={v}")));
            let v = crate::runtime_version();
            build.push(format!("{WISP_JS_PATH}?v={v}"));
            if web.clients.iter().any(Option::is_some) {
                build.push(format!("{LIVE_JS_PATH}?v={v}"));
            }
            build.extend(
                (web.clients.iter().flatten()).map(|c| format!("{}?v={}", c.path(), c.hash)),
            );
            for f in &web.js_files {
                if f.path.ends_with(".map") || f.path.starts_with(ELEMENTS) {
                    continue;
                }
                build.push(match f.file {
                    Some(_) => f.path.clone(),
                    None => format!("{}?v={}", f.path, f.hash),
                });
            }
            // The images' widths are built files too; the rest is `static/`.
            for (url, _, etag) in assets.files.iter().filter(|f| f.0 != APP_CSS_PATH) {
                match url.starts_with(IMAGES) {
                    true => build.push(url.clone()),
                    false => files.push((url.clone(), etag.clone())),
                }
            }
        }
        let hooks = self.root.join("src").join("hooks.rs");
        crate::pwa::build(&crate::pwa::Input {
            root: self.root,
            build,
            files,
            runtime_manifest: crate::read_source(&hooks).is_ok_and(|s| s.contains("app_manifest(")),
            env: &self.env,
        })
    }

    /// Per route, its page when it is the same for every request, whole, as
    /// it is sent: the shell with the head tags `setup` in wisp's http.rs
    /// makes out of dev mode (`css` is `App::CSS`), the page's head and its
    /// body. That is a page and layouts with no Rust that reads anything (a
    /// load, statements, a `+page.js`) and markup the build can write out
    /// (see `fold`). `pwa`: the service worker's and manifest's tags.
    pub(super) fn baked(&self, css: Option<&str>, pwa: &str) -> Vec<Option<String>> {
        let comp = |name: &str| {
            let k = self.comps.iter().position(|c| c.name == name)?;
            let c = &self.comps[k];
            (!c.live).then(|| (&self.templates[k].t, &c.props[..]))
        };
        let fold = fold::Fold {
            comp: &comp,
            depth: Default::default(),
        };
        let reads = |t: &Tpl| {
            t.stmts.is_some() || t.load_js.is_some() || t.user.as_ref().is_some_and(|(_, r)| *r)
        };
        let mut tags = String::new();
        if let Some(v) = css {
            let _ = write!(
                tags,
                "<link rel=\"stylesheet\" href=\"{APP_CSS_PATH}?v={v}\">"
            );
        }
        let _ = write!(
            tags,
            "<script defer src=\"{WISP_JS_PATH}?v={}\"></script>{pwa}",
            crate::runtime_version()
        );
        let [s0, s1, s2] = &self.shell;
        let m = &self.model;
        (m.routes.iter().zip(&self.tree.routes))
            .map(|(r, tr)| {
                // A page in a locale says it (`<html lang>`, the prefix).
                if self.i18n.is_some() && tr.has_locale() {
                    return None;
                }
                let tpl = |&l: &usize| &self.templates[m.layouts[l].tpl];
                let mut layers: Vec<&Tpl> = r.layouts.iter().map(tpl).collect();
                layers.push(&self.templates[r.page.as_ref()?.tpl]);
                if layers.iter().any(|t| reads(t)) {
                    return None;
                }
                let ts: Vec<&Template> = layers.iter().map(|t| &t.t).collect();
                let doc = fold.page(&ts)?;
                Some(
                    [s0, &tags, &doc.head, s1, &doc.body, s2]
                        .map(String::as_str)
                        .concat(),
                )
            })
            .collect()
    }

    /// `layout_0::tpl_layout_0::render(__o, &d0, &|__o| tpl_layout_3::render(__o, &|__o| inner))`:
    /// `inner` inside `layouts`. A layout's slots are drawn from their pages
    /// (`s{route}` is a loaded slot's data), or left empty (`drawn` false:
    /// an error page). `titled`: `inner` writes a `<title>`, so no layout
    /// around it does (`render::<false>`); else the innermost layout with one.
    pub(super) fn wrap_layouts(
        &self,
        layouts: &[usize],
        inner: String,
        titled: bool,
        drawn: bool,
    ) -> String {
        let mut titled = titled;
        layouts.iter().rev().fold(inner, |acc, &l| {
            let layout = &self.model.layouts[l];
            let gate = match self.templates[layout.tpl].t.has_title() {
                true => format!("::<{}>", !std::mem::replace(&mut titled, true)),
                false => String::new(),
            };
            let data = if layout.load {
                format!(", &d{l}")
            } else {
                String::new()
            };
            let slots: String = (self.tree.slots.iter().filter(|s| s.layout == l))
                .map(|s| match drawn {
                    true => {
                        let page = self.model.routes[s.route].page.as_ref();
                        let page = page.expect("a slot is a page");
                        let data = if page.load() {
                            format!(", &s{}", s.route)
                        } else {
                            String::new()
                        };
                        let path = self.templates[page.tpl].path();
                        // A slot something intercepts into is a place wisp.js
                        // finds, which names what goes in it: `data-wisp-cut`
                        // holds `[[target, own URL]]`.
                        let cuts: Vec<String> = (self.tree.intercepts.iter())
                            .filter(|i| {
                                i.slot == s.name && self.tree.routes[i.route].dir.starts_with(&s.dir)
                            })
                            .map(|i| format!("[{},{}]", crate::json_str(&i.target), crate::json_str(&i.inner)))
                            .collect();
                        let open = (!cuts.is_empty()).then(|| {
                            let list = format!("[{}]", cuts.join(","));
                            format!("<div data-wisp-cut=\"{}\">", list.replace('&', "&amp;").replace('"', "&quot;"))
                        });
                        match open {
                            Some(o) => format!(
                                ", &|__o: &mut ::wisp::Out| {{ __o.body.push_str({}); {path}::render(__o, cx{data}); __o.body.push_str(\"</div>\"); }}",
                                lit(&o)
                            ),
                            None => format!(", &|__o: &mut ::wisp::Out| {path}::render(__o, cx{data})"),
                        }
                    }
                    false => ", &|_: &mut ::wisp::Out| {}".to_string(),
                })
                .collect();
            format!(
                "{}::render{gate}(__o, cx{data}, &|__o: &mut ::wisp::Out| {acc}{slots})",
                self.templates[layout.tpl].path()
            )
        })
    }
}
