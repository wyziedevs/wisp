//! Just enough Rust lexing to list a file's top-level functions and types.
//!
//! The build step needs to know whether `+page.rs` defines `load`, which
//! functions carry `#[action]`, and which HTTP methods `+server.rs` defines.
//! Of a signature it reads only what shapes the call: `pub` or not, `async`
//! or not, its parameters (`cx`, and the inputs read by name), `Result` or a
//! plain value, plus the return type's text. That is enough to catch the
//! usual mistakes here, against the user's file, rather than by rustc in
//! generated code: an action that returns a value, a `load` that returns
//! something the template cannot read. Other types are left to rustc.

use crate::template::{raw_str_start, skip_char, skip_raw_str, skip_str};
pub use crate::ty::last_segment;
use crate::ty::{first_arg, is_ident, is_word, option_inner};
use std::fmt::Write;
pub use wisp_shared::rust::awaits;
use wisp_shared::rust::{skip_block_comment, skip_literal, skip_space};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub name: String,
    pub action: bool,
    /// `#[remote]`: browser code calls it (`#[remote(get)]`: by GET).
    pub remote: Option<Remote>,
    /// `pub`, in any form.
    pub public: bool,
    /// `async fn`, or an action whose body `.await`s (`#[action]` makes it
    /// `async`): the call is awaited.
    pub is_async: bool,
    /// Every parameter, as its pattern and its type: `("slug", "String")`.
    pub params: Vec<(String, String)>,
    /// Returns a `Result`, so the call ends in `?`: so does an action
    /// without `->`, which `#[action]` makes return one.
    pub fallible: bool,
    /// The return type as written, `""` for none.
    pub returns: String,
    /// The line the name is on, from 1.
    pub line: usize,
    /// An action whose body uses `cx` without taking it: `#[action]` adds
    /// `cx: &mut Cx` as its first parameter, and the call passes it.
    pub implicit_cx: bool,
    /// `#[validate(len = 1..=100)] text: String`: each parameter's rules,
    /// as (parameter, what is inside `validate(…)`).
    pub checks: Vec<(String, String)>,
}

/// How browser code calls a `#[remote]` function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// `#[remote]`: a POST with the arguments as JSON.
    Post,
    /// `#[remote(get)]`: a GET with them in the query, which may be cached.
    Get,
}

/// A top-level `struct`, `enum`, `union` or `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeItem {
    pub name: String,
    /// The fields of a struct with named fields: name and type.
    pub fields: Vec<(String, String)>,
    /// What its `#[derive(...)]`s name, by last segment: `Json`, `Rest`.
    pub derives: Vec<String>,
    /// `#[validate(len = 1..=9)]` on its fields, as (field, what is inside
    /// `validate(…)`).
    pub rules: Vec<(String, String)>,
}

impl TypeItem {
    /// Field `name` is one Wisp sets when it is left out: a
    /// `#[derive(Rest)]` type's `created_at` and `updated_at`.
    pub fn set_by_wisp(&self, name: &str) -> bool {
        matches!(name, "created_at" | "updated_at") && self.derives.iter().any(|d| d == "Rest")
    }
}

/// A top-level `const` or `static`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub name: String,
    /// The type as written.
    pub ty: String,
    pub line: usize,
    /// A `static`, not a `const`: one value for the whole program.
    pub is_static: bool,
    /// Its value as written, after the `=`.
    pub value: String,
}

/// What a function hands back, as far as the generated call cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Returns {
    /// Nothing, `()`, or `Result<()>`.
    Nothing,
    /// A `Response`, plain or in a `Result`.
    Response,
    /// An `Option<Response>`, plain or in a `Result`.
    MaybeResponse,
    Other,
}

#[derive(Debug, Default)]
pub struct Items {
    pub fns: Vec<FnItem>,
    pub types: Vec<TypeItem>,
    pub consts: Vec<ConstItem>,
    /// The line of the first inner attribute or doc comment (`#![…]`,
    /// `//!`) after the file's first item: those belong at the top.
    pub inner: Option<usize>,
    /// The first `Err(error(..))` or the like, from before `error()` and
    /// `redirect()` returned the `Result` themselves: line and function.
    old_call: Option<(usize, &'static str)>,
}

impl Items {
    pub fn function(&self, name: &str) -> Option<&FnItem> {
        self.fns.iter().find(|f| f.name == name)
    }

    /// The fields of `Data`, which a template can use by name.
    pub fn data_fields(&self) -> Vec<(String, String)> {
        let data = self.types.iter().find(|t| t.name == "Data");
        data.map(|t| t.fields.clone()).unwrap_or_default()
    }

    pub fn constant(&self, name: &str) -> Option<&ConstItem> {
        self.consts.iter().find(|c| c.name == name)
    }

    /// A page the browser draws: `const SSR: bool = false;`. (The build
    /// checks such a flag is a `bool` literal; see `codegen::flag`.)
    pub fn drawn(&self) -> bool {
        self.constant("SSR").is_some_and(|c| c.value == "false")
    }

    /// The tables the file keeps, which load when the app starts: each
    /// `static` of type `Table<…>` and each `#[derive(Rest)]` type's own.
    /// As paths from a module inside the file's (`super::TODOS`).
    pub fn tables(&self) -> Vec<String> {
        let statics = self
            .consts
            .iter()
            .filter(|c| c.is_static && last_segment(&c.ty) == "Table")
            .map(|c| format!("super::{}", c.name));
        let rest = self
            .types
            .iter()
            .filter(|t| t.derives.iter().any(|d| d == "Rest"))
            .map(|t| format!("super::{}::table()", t.name));
        statics.chain(rest).collect()
    }

    /// The `.live()` tables the file keeps, as (their static's name, their
    /// name in the store, which is their channel): `static POSTS:
    /// Table<Post> = Table::saved("posts").live();`.
    pub fn live_tables(&self) -> Vec<(String, String)> {
        let live = |c: &&ConstItem| c.is_static && last_segment(&c.ty) == "Table";
        (self.consts.iter().filter(live))
            .filter(|c| c.value.contains(".live()"))
            .filter_map(|c| {
                let name = c.value.split("saved(\"").nth(1)?.split('"').next()?;
                Some((c.name.clone(), name.to_string()))
            })
            .collect()
    }

    /// The `#[derive(Config)]` types, which read the environment at start.
    pub fn configs(&self) -> Vec<String> {
        let has = |t: &&TypeItem| t.derives.iter().any(|d| d == "Config");
        (self.types.iter().filter(has))
            .map(|t| format!("super::{}", t.name))
            .collect()
    }

    /// Why the file cannot be a route file (or `src/hooks.rs`), if it
    /// cannot: its `load` must return the `Data` the template reads.
    pub fn check(&self) -> Result<(), String> {
        self.check_inner()?;
        let Some(load) = self.function("load") else {
            return Ok(());
        };
        if !load
            .returns
            .split(|c: char| !(c.is_ascii() && is_word(c as u8)))
            .any(|w| w == "Data")
        {
            let returns = if load.returns.is_empty() {
                "nothing"
            } else {
                &load.returns
            };
            return Err(format!(
                "{}: `load` returns {returns}, but the template reads a `Data`. Return `Data` or `Result<Data>`.",
                load.line
            ));
        }
        Ok(())
    }

    /// The file has `//!` docs and `#![…]` attributes only at its top, and
    /// no `Err(error(..))`, which `error()` returning the `Result` broke.
    pub fn check_inner(&self) -> Result<(), String> {
        if let Some(line) = self.inner {
            return Err(format!(
                "{line}: `//!` docs and `#![…]` attributes go at the top of the file, before its first item"
            ));
        }
        match self.old_call {
            Some((line, name)) if !self.fns.iter().any(|f| f.name == name) => {
                let error = match name {
                    "error" => "Error::new(status, message)",
                    _ => "Error::redirect(status, location)",
                };
                Err(format!(
                    "{line}: `{name}()` returns the `Result` itself, so `Err({name}(..))` is a `Result` inside an `Err`.                      Write `return {name}(..)`, or `{error}` where the `Error` is wanted (`Err(..)`, `ok_or`, `map_err`)."
                ))
            }
            _ => Ok(()),
        }
    }
}

impl FnItem {
    /// The parameters other than `cx`, which are read from the request by
    /// name, as (name, type). A pattern that is not a plain name is an error.
    pub fn inputs(&self) -> Result<Vec<(&str, &str)>, String> {
        let mut out = Vec::new();
        for (pat, ty) in &self.params {
            if is_cx(ty) {
                continue;
            }
            let name = pat.as_str();
            if !is_ident(name) || name == "_" {
                return Err(format!(
                    "{}: `{}` takes `{pat}: {ty}`; each parameter but `cx` is read from the request by its name, so it needs one, like `id: u64`",
                    self.line, self.name
                ));
            }
            out.push((name, ty.as_str()));
        }
        Ok(out)
    }

    /// The type it returns, looking through a `Result`: `""` for nothing.
    pub fn value_type(&self) -> &str {
        let t = self.returns.trim();
        match (self.fallible, t.contains('<')) {
            (false, _) => t,
            // `Result` alone is `Result<()>`.
            (true, true) => first_arg(t).unwrap_or("?"),
            (true, false) => "",
        }
    }

    /// `T` of an `Option<T>` it returns (other than `Option<Response>`):
    /// `None` is a 404, `Some(())` a 204.
    pub fn optional_value(&self) -> Option<&str> {
        match self.returns_kind() {
            Returns::Other => option_inner(self.value_type()),
            _ => None,
        }
    }

    /// What it returns, looking through a `Result`.
    pub fn returns_kind(&self) -> Returns {
        returns_kind(self.value_type())
    }
}

/// What a function whose value (through a `Result`) is of type `t` returns.
pub fn returns_kind(t: &str) -> Returns {
    let response = |t: &str| last_segment(t) == "Response" && !t.contains('<');
    let t = t.trim();
    if t.is_empty() || t == "()" {
        Returns::Nothing
    } else if response(t) {
        Returns::Response
    } else if option_inner(t).is_some_and(response) {
        Returns::MaybeResponse
    } else {
        Returns::Other
    }
}

/// A `Cx` parameter's type: `&mut Cx`, `&Cx`, `&'a mut wisp::Cx`.
pub fn is_cx(ty: &str) -> bool {
    let t = crate::ty::unref(ty);
    last_segment(t) == "Cx" && !t.contains('<')
}

/// Top-level `fn` and type items in source order. Nested functions,
/// functions inside `impl`/`mod` blocks, comments and string contents are
/// ignored. Fails, with the line, on an `#[action]` that is not on a
/// top-level function, where it would silently do nothing.
pub fn scan(src: &str) -> Result<Items, String> {
    let b = src.as_bytes();
    let mut items = Items::default();
    let mut depth = 0u32;
    let lead = inner_end(src);
    let line = |at: usize| src[..at].matches('\n').count() + 1;
    // Seen since the last item ended.
    let mut action = false;
    let mut remote = None;
    let mut is_async = false;
    let mut public = false;
    let mut derives: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                if depth == 0 && i >= lead && b.get(i + 2) == Some(&b'!') {
                    items.inner.get_or_insert(line(i));
                }
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                if depth == 0 && i >= lead && b.get(i + 2) == Some(&b'!') {
                    items.inner.get_or_insert(line(i));
                }
                i = skip_block_comment(b, i);
            }
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            b'#' => {
                // Attribute: #[path] or #![path]. Record the marker, skip the rest.
                let mut j = i + 1;
                if b.get(j) == Some(&b'!') {
                    if depth == 0 && i >= lead {
                        items.inner.get_or_insert(line(i));
                    }
                    j += 1;
                }
                if b.get(j) == Some(&b'[') {
                    // Unclosed (a half-typed save): nothing after it is an
                    // item yet, and rustc will say what is wrong.
                    let Some(end) = matching_bracket(b, j) else {
                        break;
                    };
                    let path: String = src[j + 1..end]
                        .chars()
                        .take_while(|&c| c != '(')
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    // `action`, `wisp::action`, `::wisp::action`...
                    let marks_action = path.rsplit("::").next() == Some("action");
                    if marks_action && depth > 0 {
                        return Err(format!(
                            "{}: #[action] marks a top-level function of +page.rs; this one is inside a block, where it does nothing",
                            line(i)
                        ));
                    }
                    action |= marks_action;
                    if path.rsplit("::").next() == Some("remote") {
                        if depth > 0 {
                            return Err(format!(
                                "{}: #[remote] marks a top-level function; this one is inside a block, where it does nothing",
                                line(i)
                            ));
                        }
                        let args = src[j + 1..end].split_once('(').map_or("", |(_, a)| a);
                        remote = Some(match args.trim_end_matches(')').trim() {
                            "get" => Remote::Get,
                            _ => Remote::Post,
                        });
                    }
                    // `#[model]` derives these.
                    if depth == 0 && path.rsplit("::").next() == Some("model") {
                        derives.extend(["Json", "FromJson", "Clone"].map(String::from));
                    }
                    if depth == 0 && path.rsplit("::").next() == Some("derive") {
                        let args = src[j + 1..end].split_once('(').map_or("", |(_, a)| a);
                        derives.extend(
                            args.trim_end_matches([')', ' ', '\n', '\r', '\t'])
                                .split(',')
                                .map(|d| last_segment(d).to_string())
                                .filter(|d| !d.is_empty()),
                        );
                    }
                    i = end;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    (action, is_async, public, remote) = (false, false, false, None);
                    derives.clear();
                }
            }
            b';' if depth == 0 => {
                (action, is_async, public, remote) = (false, false, false, None);
                derives.clear();
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                i = ident_end(b, i);
                match &src[start..i] {
                    word @ ("error" | "redirect") if old_call(b, start, i) => {
                        let name = if word == "error" { "error" } else { "redirect" };
                        items.old_call.get_or_insert((line(start), name));
                    }
                    "pub" if depth == 0 => public = true,
                    "async" if depth == 0 => is_async = true,
                    word @ ("const" | "static") if depth == 0 => {
                        let mut at = skip_space(b, i);
                        let mut name = &src[at..ident_end(b, at)];
                        if word == "static" && name == "mut" {
                            at = skip_space(b, at + 3);
                            name = &src[at..ident_end(b, at)];
                        }
                        // `const fn` and the like are functions, found at their `fn`.
                        if !matches!(name, "" | "_" | "fn" | "async" | "unsafe" | "extern") {
                            let after = at + name.len();
                            let ty = match src[after..].trim_start().strip_prefix(':') {
                                Some(t) => t[..t.find(['=', ';']).unwrap_or(t.len())]
                                    .trim()
                                    .to_string(),
                                None => String::new(),
                            };
                            let rest = &src[after..];
                            let value = (rest.find(['=', ';']))
                                .filter(|&e| rest.as_bytes()[e] == b'=')
                                .map(|e| &rest[e + 1..])
                                .map_or("", |v| v[..v.find(';').unwrap_or(v.len())].trim());
                            items.consts.push(ConstItem {
                                name: name.to_string(),
                                ty,
                                line: line(at),
                                is_static: word == "static",
                                value: value.to_string(),
                            });
                        }
                    }
                    word @ ("fn" | "struct" | "enum" | "union" | "type") if depth == 0 => {
                        while i < b.len() && b[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        let name_start = i;
                        i = ident_end(b, i);
                        let name = src[name_start..i].to_string();
                        if name.is_empty() {
                            // `fn(u8) -> u8` as a type, say.
                        } else if word == "fn" {
                            let ((params, checks), fallible, returns, body) = signature(src, i);
                            let line = line(name_start);
                            let takes_cx = params.iter().any(|(p, t)| p == "cx" || is_cx(t));
                            // `#[remote]` makes its function as `#[action]` does.
                            let marked = action || remote.is_some();
                            let implicit_cx = marked
                                && !takes_cx
                                && b.get(body) == Some(&b'{')
                                && uses_ident(&b[body..block_end(b, body)], b"cx");
                            // `#[action]` makes one without `->` return `Result`,
                            // and one that awaits `async`.
                            let fallible = fallible || (marked && returns.is_empty());
                            is_async = is_async
                                || (marked
                                    && b.get(body) == Some(&b'{')
                                    && awaits(&src[body..block_end(b, body)]));
                            items.fns.push(FnItem {
                                name,
                                action,
                                remote,
                                public,
                                is_async,
                                params,
                                fallible,
                                returns,
                                line,
                                implicit_cx,
                                checks,
                            });
                            (action, is_async, public, remote) = (false, false, false, None);
                            derives.clear();
                        } else {
                            let (fields, rules) = if word == "struct" {
                                fields(src, i)
                            } else {
                                (Vec::new(), Vec::new())
                            };
                            items.types.push(TypeItem {
                                name,
                                fields,
                                derives: std::mem::take(&mut derives),
                                rules,
                            });
                        }
                    }
                    _ => {}
                }
                continue; // `i` already points past the identifier.
            }
            _ => {}
        }
        i += 1;
    }
    Ok(items)
}

/// The fields of the struct whose name ends at `i`, as (name, type), and
/// the `#[validate(…)]` rules on them. Tuple structs and generic ones have
/// none.
fn fields(src: &str, i: usize) -> Params {
    let b = src.as_bytes();
    let open = skip_space(b, i);
    if b.get(open) != Some(&b'{') {
        return (Vec::new(), Vec::new());
    }
    let (mut out, mut rules) = (Vec::new(), Vec::new());
    let (mut depth, mut start, mut j) = (0i32, open + 1, open + 1);
    while j < b.len() {
        match b[j] {
            // Strings (raw ones too), chars and comments.
            b'/' | b'"' | b'\'' | b'r' if skip_literal(b, j) != j => j = skip_literal(b, j),
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b'>' if b[j - 1] == b'-' => {}
            b')' | b']' | b'>' => depth -= 1,
            b',' | b'}' if depth <= 0 => {
                if let Some((name, ty, rule)) = field(&src[start..j]) {
                    rules.extend(rule.map(|r| (name.clone(), r)));
                    out.push((name, ty));
                }
                if b[j] == b'}' {
                    break;
                }
                start = j + 1;
            }
            b'}' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    (out, rules)
}

/// `name: Type`, after any attributes, comments and visibility (`pub`,
/// `pub(crate)`): the template is compiled inside the module, so it sees
/// private fields too. And what is inside its `#[validate(…)]`, if it has one.
fn field(mut s: &str) -> Option<(String, String, Option<String>)> {
    let mut rules = None;
    loop {
        s = s.trim_start();
        if let Some(c) = s.strip_prefix("//") {
            s = c.split_once('\n').map_or("", |x| x.1);
        } else if s.starts_with("#[") {
            let end = matching_bracket(s.as_bytes(), 1)?;
            rules = validate_args(&s[2..end]).or(rules);
            s = &s[end + 1..];
        } else {
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("pub") {
        let rest = rest.trim_start();
        s = match rest.strip_prefix('(') {
            Some(r) => &r[r.find(')')? + 1..],
            None => rest,
        };
    }
    let (name, ty) = s.split_once(':')?;
    Some((name.trim().to_string(), ty.trim().to_string(), rules))
}

/// What is inside `validate(…)`, for the inside of an attribute's brackets.
fn validate_args(inner: &str) -> Option<String> {
    let args = inner
        .trim()
        .strip_prefix("validate")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    Some(args.trim().to_string())
}

/// `a: u8, b: impl Fn(u8) -> u8` split at its top-level commas, blank
/// pieces (after a trailing comma) left out.
fn split_top(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' | b'[' | b'<' | b'{' => depth += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b')' | b']' | b'>' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out.retain(|p| !p.trim().is_empty());
    out
}

/// `slug: String` → `("slug", "String")`: split at the first `:` that is
/// not part of a `::`.
fn param(p: &str) -> (String, String) {
    let b = p.as_bytes();
    let colon = (0..b.len())
        .find(|&i| b[i] == b':' && b.get(i + 1) != Some(&b':') && (i == 0 || b[i - 1] != b':'));
    match colon {
        Some(i) => (p[..i].trim().to_string(), p[i + 1..].trim().to_string()),
        None => (p.trim().to_string(), String::new()),
    }
}

/// A parameter's `#[validate(…)]` rules (what is inside the parentheses)
/// and the parameter without its attributes.
fn param_attrs(mut p: &str) -> (Option<String>, &str) {
    let mut rules = None;
    loop {
        p = p.trim_start();
        let Some(end) = p
            .strip_prefix('#')
            .and_then(|r| r.trim_start().starts_with('[').then_some(()))
            .and_then(|()| matching_bracket(p.as_bytes(), p.find('[')?))
        else {
            return (rules, p);
        };
        rules = validate_args(&p[p.find('[').unwrap_or(0) + 1..end]).or(rules);
        p = &p[end + 1..];
    }
}

/// Names and types, and the `#[validate(…)]` rules on them: a signature's
/// parameters, a struct's fields.
type Params = (Vec<(String, String)>, Vec<(String, String)>);

/// Reads a signature from just past the function's name up to its body:
/// its parameters (and their rules), whether its return type is a `Result`
/// (`Result<T>`, `wisp::Result<T>`, `io::Result<T>`, ...), that type, and
/// where the body's `{` (or the `;` of a function without one) is.
fn signature(src: &str, mut i: usize) -> (Params, bool, String, usize) {
    let b = src.as_bytes();
    let mut depth = 0i32; // (), [] and <>
    // Where the parameter list starts, until it has been read.
    let mut open = None;
    let mut read = false;
    let mut params: Params = (Vec::new(), Vec::new());
    let mut ret = None;
    while i < b.len() {
        match b[i] {
            b'-' if b.get(i + 1) == Some(&b'>') => {
                if depth == 0 && ret.is_none() {
                    ret = Some(i + 2);
                }
                i += 1;
            }
            b'(' => {
                if depth == 0 && !read {
                    open = Some(i + 1);
                }
                depth += 1;
            }
            b'[' | b'<' => depth += 1,
            b')' | b']' | b'>' => {
                depth -= 1;
                if depth == 0
                    && b[i] == b')'
                    && let Some(start) = open.take()
                {
                    read = true;
                    let text = strip_comments(&src[start..i]);
                    for piece in split_top(&text) {
                        let (rules, rest) = param_attrs(piece);
                        let (name, ty) = param(rest);
                        // `mut n` reads input `n`: the name alone, once.
                        let name = match name.strip_prefix("mut ") {
                            Some(n) => n.trim().to_string(),
                            None => name,
                        };
                        if let Some(r) = rules {
                            params.1.push((name.clone(), r));
                        }
                        params.0.push((name, ty));
                    }
                }
            }
            b'{' | b';' if depth == 0 => break,
            b'\'' => i = skip_char(b, i),
            _ => {}
        }
        i += 1;
    }
    let i = i.min(b.len());
    let returns = ret.map_or("", |r| {
        let t = &src[r..i];
        // Up to a `where` clause.
        let clause = t.match_indices("where").find(|&(w, _)| {
            let before = t[..w].chars().next_back().is_none_or(char::is_whitespace);
            let after = t[w + 5..].chars().next().is_none_or(char::is_whitespace);
            before && after
        });
        clause.map_or(t, |(w, _)| &t[..w]).trim()
    });
    let path = returns
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
        .next();
    let fallible = path.and_then(|p| p.rsplit("::").next()) == Some("Result");
    (params, fallible, returns.to_string(), i)
}

/// The index just past the `}` closing the `{` at `open` (the end of `b` if
/// it is not closed), skipping literals and comments.
fn block_end(b: &[u8], open: usize) -> usize {
    let mut depth = 0u32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => i = skip_literal(b, i),
        }
        i += 1;
    }
    b.len()
}

/// Whether the identifier `name` appears in `b` as code (not in a literal,
/// a comment, or as part of a longer name or a path after `.`/`::`).
fn uses_ident(b: &[u8], name: &[u8]) -> bool {
    let mut i = 0;
    while i < b.len() {
        let j = skip_literal(b, i);
        if j != i {
            i = j + 1;
            continue;
        }
        if is_word(b[i]) {
            let start = i;
            i = ident_end(b, i);
            let after_dot = start > 0 && b[start - 1] == b'.';
            if &b[start..i] == name && !after_dot && !b[..start].ends_with(b"::") {
                return true;
            }
            continue;
        }
        i += 1;
    }
    false
}

/// A page's `---` block split in two: its items (`fn`, `struct`, `use`,
/// `static`, `impl`...), which go in the page's module, and its statements,
/// which run for each request before the template renders and whose names
/// it reads. Each comes back as long as `code`, the other part blanked but
/// its newlines kept, so every line stays on its line in the file. An item
/// takes the docs, comments and attributes just before it.
pub fn split_items(code: &str) -> (String, String) {
    let b = code.as_bytes();
    let mut items: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut depth, mut boundary) = (0, 0i32, true);
    while i < b.len() {
        if boundary && depth == 0 {
            boundary = false;
            // Where the next piece starts, comments included.
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if let Some(end) = item_at(b, i) {
                items.push((i, end));
                i = end;
                boundary = true;
                continue;
            }
        }
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                boundary = depth == 0;
            }
            b';' if depth == 0 => boundary = true,
            _ => i = skip_literal(b, i),
        }
        i += 1;
    }
    let keep = |inside: bool| -> String {
        code.char_indices()
            .map(|(k, c)| {
                let in_item = items.iter().any(|&(s, e)| k >= s && k < e);
                if c == '\n' || in_item == inside {
                    c
                } else {
                    ' '
                }
            })
            .collect()
    };
    (keep(true), keep(false))
}

/// A `---` block's `fn default`, when it is the only one and the block has
/// no `#[action]`, is the page's default action: the block with `#[action]`
/// put before it, on its line, or `None` when that is not so. One that
/// returns data is a function of the page's own, not an action.
pub fn mark_default(code: &str) -> Option<String> {
    let items = scan(&split_items(code).0).ok()?;
    if items.fns.iter().any(|f| f.action) {
        return None;
    }
    let mut lone = items.fns.iter().filter(|f| f.name == "default");
    let f = lone.next().filter(|_| lone.next().is_none())?;
    if !(f.returns.is_empty() || f.fallible || f.returns_kind() != Returns::Other) {
        return None;
    }
    let line: usize = code
        .split_inclusive('\n')
        .take(f.line - 1)
        .map(str::len)
        .sum();
    let at = code.len() - code[line..].trim_start_matches([' ', '\t']).len();
    Some(format!("{}#[action] {}", &code[..at], &code[at..]))
}

/// The names the top-level `let`s of `stmts` bind: `let (a, mut b) = …`
/// binds `a` and `b`.
pub fn let_names(stmts: &str) -> Vec<String> {
    let b = stmts.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let (mut i, mut depth) = (0, 0i32);
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            c if depth == 0 && (c.is_ascii_alphabetic() || c == b'_') => {
                let end = ident_end(b, i);
                let before = i == 0 || !is_word(b[i - 1]);
                if before && &b[i..end] == b"let" {
                    // The pattern: up to its type, its `=` or its `;`.
                    let mut j = end;
                    let mut d = 0i32;
                    while j < b.len() {
                        match b[j] {
                            b'(' | b'[' | b'{' => d += 1,
                            b')' | b']' | b'}' => d -= 1,
                            b':' if d == 0 && b.get(j + 1) != Some(&b':') && b[j - 1] != b':' => {
                                break;
                            }
                            b'=' | b';' if d == 0 => break,
                            _ => {}
                        }
                        j += 1;
                    }
                    let pat = &stmts[end..j];
                    let pb = pat.as_bytes();
                    let mut k = 0;
                    while k < pb.len() {
                        if pb[k].is_ascii_lowercase() || pb[k] == b'_' {
                            let e = ident_end(pb, k);
                            let word = &pat[k..e];
                            let next = pat[e..].trim_start();
                            let path = next.starts_with("::") || pat[..k].ends_with("::");
                            let call = next.starts_with('(') || next.starts_with('{');
                            if !matches!(word, "mut" | "ref" | "_")
                                && !path
                                && !call
                                && !out.iter().any(|n| n == word)
                            {
                                out.push(word.to_string());
                            }
                            k = e;
                        } else if pb[k].is_ascii_alphanumeric() {
                            k = ident_end(pb, k);
                        } else {
                            k += 1;
                        }
                    }
                    i = j;
                    continue;
                }
                i = end;
                continue;
            }
            _ => i = skip_literal(b, i),
        }
        i += 1;
    }
    out
}

/// Whether statements `code` may wait: they `.await`, or call a macro,
/// which may expand to an await (`join!`, `select!`, one of the app's),
/// but for std's that cannot. A doubt is a yes: see `codegen`'s `now`.
pub fn may_wait(code: &str) -> bool {
    const PLAIN: [&[u8]; 32] = [
        b"assert",
        b"assert_eq",
        b"assert_ne",
        b"cfg",
        b"column",
        b"concat",
        b"dbg",
        b"debug_assert",
        b"debug_assert_eq",
        b"debug_assert_ne",
        b"env",
        b"eprint",
        b"eprintln",
        b"file",
        b"format",
        b"format_args",
        b"include_bytes",
        b"include_str",
        b"line",
        b"matches",
        b"module_path",
        b"option_env",
        b"panic",
        b"print",
        b"println",
        b"stringify",
        b"todo",
        b"unimplemented",
        b"unreachable",
        b"vec",
        b"write",
        b"writeln",
    ];
    if awaits(code) {
        return true;
    }
    let b = code.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let end = ident_end(b, i);
            let bang = b.get(end) == Some(&b'!') && b.get(end + 1) != Some(&b'=');
            // Any path is a doubt (`tokio::join!`), even `std::vec!`.
            let path = b[..i].ends_with(b"::");
            if bang && (path || !PLAIN.contains(&&b[i..end])) {
                return true;
            }
            i = end;
            continue;
        }
        i = skip_literal(b, i) + 1;
    }
    false
}

/// If an item starts at `i` (after any comments, attributes and `pub`),
/// the index just past its end.
fn item_at(b: &[u8], mut i: usize) -> Option<usize> {
    let word = |at: usize| {
        let at = skip_space(b, at);
        (at, &b[at..ident_end(b, at)])
    };
    loop {
        i = skip_space(b, i);
        if b[i..].starts_with(b"#[") {
            i = matching_bracket(b, i + 1)? + 1;
        } else {
            break;
        }
    }
    let (at, mut w) = word(i);
    let mut next = at + w.len();
    if w == b"pub" {
        next = skip_space(b, next);
        if b.get(next) == Some(&b'(') {
            next = b[next..].iter().position(|&c| c == b')')? + next + 1;
        }
        (next, w) = word(next);
        next += w.len();
    }
    let semi = match w {
        b"const" if b.get(skip_space(b, next)) == Some(&b'{') => return None,
        b"use" | b"static" | b"type" | b"const" => true,
        b"fn" | b"struct" | b"enum" | b"union" | b"trait" | b"impl" | b"mod" => false,
        b"async" | b"unsafe" | b"extern" => {
            let (_, w2) = word(next);
            if !matches!(w2, b"fn" | b"impl" | b"trait" | b"crate" | b"unsafe") {
                return None;
            }
            w2 == b"crate"
        }
        b"macro_rules" if b.get(next) == Some(&b'!') => false,
        _ => return None,
    };
    // Up to its `;`, or (for one with a body) the `}` that closes it.
    let mut depth = 0i32;
    let mut j = next;
    while j < b.len() {
        match b[j] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0 && !semi {
                    return Some(j + 1);
                }
            }
            b';' if depth == 0 => return Some(j + 1),
            _ => j = skip_literal(b, j),
        }
        j += 1;
    }
    Some(b.len())
}

/// Per line of `code`: whether it ends in code, where a `// …` comment can
/// be added, rather than inside a string or a block comment.
pub fn line_ends_in_code(code: &str) -> Vec<bool> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            out.push(true);
            i += 1;
            continue;
        }
        let j = skip_literal(b, i);
        if j != i && !b[i..].starts_with(b"//") {
            // Lines the literal or comment runs past end inside it.
            let open = b[i..j.min(b.len())].iter().filter(|&&c| c == b'\n').count();
            out.extend(std::iter::repeat_n(false, open));
        }
        i = j + 1;
    }
    out.push(true);
    out
}

/// `src` with each `static NAME: … = Table::saved();` named for its static,
/// `Table::saved("name")` (`USERS` is "users"), or `None` if there is none.
/// The text keeps its lines.
pub fn name_saved(src: &str) -> Option<String> {
    let b = src.as_bytes();
    let (mut out, mut from, mut name, mut i) = (String::new(), 0, String::new(), 0);
    while i < b.len() {
        if b[i] == b';' {
            name.clear();
        }
        if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
            i = skip_literal(b, i) + 1;
            continue;
        }
        let end = ident_end(b, i);
        match &src[i..end] {
            "static" => {
                let at = skip_space(b, end);
                name = src[at..ident_end(b, at)].to_ascii_lowercase();
            }
            "Table" if !name.is_empty() && src[end..].starts_with("::saved()") => {
                out.push_str(&src[from..end + 7]);
                let _ = write!(out, "({name:?})");
                from = end + 9;
            }
            _ => {}
        }
        i = end;
    }
    out.push_str(&src[from..]);
    (from > 0).then_some(out)
}

/// The table `wisp::users(&db::USERS)` names, as written, in `src` (the
/// hooks file), if it does.
pub fn users_table(src: &str) -> Result<Option<String>, String> {
    let src = strip_comments(src);
    let Some(at) = src.find("wisp::users(") else {
        return Ok(None);
    };
    let arg = &src[at + 12..];
    let arg = arg[..arg.find(')').unwrap_or(arg.len())]
        .trim()
        .trim_start_matches('&');
    if !arg.contains("::") {
        return Err(format!(
            "`wisp::users` takes the table by its module path: `wisp::users(&db::{arg})`"
        ));
    }
    Ok(Some(arg.trim().to_string()))
}

/// `src` with each `cx.user()` made `cx.user(&TABLE)` (the table
/// `wisp::users` names in `init`), or `None` if there is none. An error
/// when there is one but no table.
pub fn bind_user(src: &str, table: Option<&str>) -> Result<Option<String>, String> {
    let b = src.as_bytes();
    let (mut out, mut from, mut i) = (String::new(), 0, 0);
    while i < b.len() {
        if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
            i = skip_literal(b, i) + 1;
            continue;
        }
        let end = ident_end(b, i);
        if &src[i..end] == "cx" && src[end..].starts_with(".user()") && (i == 0 || b[i - 1] != b'.')
        {
            let Some(table) = table else {
                return Err("`cx.user()` needs `wisp::users(&db::USERS)` in `init` (src/hooks.rs), naming the users table".into());
            };
            out.push_str(&src[from..end + 6]);
            let _ = write!(out, "&{table}");
            from = end + 6;
        }
        i = end;
    }
    out.push_str(&src[from..]);
    Ok((from > 0).then_some(out))
}

/// `s` with its comments blanked out.
fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"//") || b[i..].starts_with(b"/*") {
            let end = skip_space(b, i);
            out.push(' ');
            i = end;
        } else {
            let n = s[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&s[i..i + n]);
            i += n;
        }
    }
    out
}

/// The end of the identifier starting at `i` (`i` itself if there is none).
pub(crate) fn ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_word(b[i]) {
        i += 1;
    }
    i
}

/// Where the file's leading `//!` docs and `#![…]` attributes end (0 if it
/// has none). A module can hold them only at its top, so the build splices
/// what Wisp adds after them.
pub fn inner_end(src: &str) -> usize {
    let b = src.as_bytes();
    let (mut i, mut end) = (0, 0);
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let rest = &src[i..];
        if rest.starts_with("//!") || (rest.starts_with("//") && !rest.starts_with("///")) {
            let doc = rest.starts_with("//!");
            i += rest.find('\n').unwrap_or(rest.len());
            if doc {
                end = i;
            }
        } else if rest.starts_with("/*") && !rest.starts_with("/**") {
            let doc = rest.starts_with("/*!");
            i = (skip_block_comment(b, i) + 1).min(b.len());
            if doc {
                end = i;
            }
        } else if rest.starts_with("#![") {
            match matching_bracket(b, i + 2) {
                Some(close) => (i, end) = (close + 1, close + 1),
                None => return end,
            }
        } else {
            return end;
        }
    }
}

/// Whether `error` or `redirect` at `start..end` is a call written the old
/// way: inside `Err(`, `ok_or(` or a closure, which wants an `Error`.
fn old_call(b: &[u8], start: usize, end: usize) -> bool {
    if b.get(skip_space(b, end)) != Some(&b'(') {
        return false;
    }
    let mut at = start;
    let back = |mut at: usize| {
        while at > 0 && b[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        at
    };
    at = back(at);
    if b[..at].ends_with(b"wisp::") {
        at = back(at - 6);
    }
    let before = &b[..at];
    before.ends_with(b"Err(") || before.ends_with(b"ok_or(") || before.ends_with(b"|")
}

/// The `]` that closes the `[` at `open`, if there is one.
fn matching_bracket(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' | b'r' => i = skip_literal(b, i),
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top_level_fns(src: &str) -> Vec<FnItem> {
        scan(src).unwrap().fns
    }

    fn takes_cx(f: &FnItem) -> bool {
        f.params.iter().any(|(_, ty)| is_cx(ty))
    }

    fn names(src: &str) -> Vec<(String, bool)> {
        top_level_fns(src)
            .into_iter()
            .map(|f| (f.name, f.action))
            .collect()
    }

    #[test]
    fn finds_load_and_actions() {
        let src = r#"
            use wisp::prelude::*;
            // fn commented_out() {}
            /* fn also_not() { /* nested */ } */
            pub struct Data { pub n: i32 }

            /// Loads things.
            pub async fn load(cx: &mut Cx) -> Result<Data> {
                fn inner() {}
                let s = "fn fake() {";
                let c = '{';
                Ok(Data { n: 1 })
            }

            #[action]
            pub async fn like(cx: &mut Cx) -> Result<()> { Ok(()) }

            #[wisp::action]
            #[allow(unused)]
            pub(crate) async fn delete_all<'a>(cx: &'a mut Cx) -> Result<()> { Ok(()) }

            pub async fn helper() {}
            impl Data { pub fn method(&self) {} }
        "#;
        assert_eq!(
            names(src),
            [
                ("load".into(), false),
                ("like".into(), true),
                ("delete_all".into(), true),
                ("helper".into(), false)
            ]
        );
    }

    #[test]
    fn signatures() {
        let src = r#"
            pub async fn load(cx: &mut Cx) -> Result<Data> { Ok(Data) }
            pub fn plain() -> Data { Data }
            pub fn nothing(cx: &Cx) {}
            pub async fn generic<'a, F: Fn(u8) -> Result<()>>(cx: &'a mut Cx, f: F) -> wisp::Result<()> {}
            pub fn io( ) -> std::io::Result<Data> where Data: Sized {}
            fn later() -> Data;
            const X: fn() -> Result<()> = f;
            async fn after_const() {}
        "#;
        let sigs: Vec<_> = top_level_fns(src)
            .into_iter()
            .map(|f| (f.name.clone(), f.is_async, takes_cx(&f), f.fallible))
            .collect();
        assert_eq!(
            sigs,
            [
                ("load".into(), true, true, true),
                ("plain".into(), false, false, false),
                ("nothing".into(), false, true, false),
                ("generic".into(), true, true, true),
                ("io".into(), false, false, true),
                ("later".into(), false, false, false),
                ("after_const".into(), true, false, false),
            ]
        );
    }

    #[test]
    fn actions_that_await_are_async() {
        let src = "#[action] fn a() { let h = hash(&p).await; }
                   #[action] fn b() { let s = \"x.await\"; }
                   fn c() { f().await }
                   #[action] async fn d() {}";
        let fs = top_level_fns(src);
        let got: Vec<bool> = fs.iter().map(|f| f.is_async).collect();
        assert_eq!(got, [true, false, false, true]);
    }

    #[test]
    fn return_types() {
        let src = "#[action]\npub fn a() {}\n#[action] pub fn b(cx: &mut Cx) -> Result<()> { Ok(()) }\n\
                   #[action] async fn c() -> wisp::Result<(), MyError> {}\nfn d() -> Result<u8> {}\n\
                   fn e() -> std::io::Result<Data> where Data: Sized {}\nfn f() -> () {}\nfn g() -> Resultant<()> {}";
        let fns = top_level_fns(src);
        let seen: Vec<_> = fns
            .iter()
            .map(|f| {
                (
                    f.name.as_str(),
                    f.returns.as_str(),
                    f.returns_kind() == Returns::Nothing,
                    f.line,
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("a", "", true, 2),
                ("b", "Result<()>", true, 3),
                ("c", "wisp::Result<(), MyError>", true, 4),
                ("d", "Result<u8>", false, 5),
                ("e", "std::io::Result<Data>", false, 6),
                ("f", "()", true, 7),
                ("g", "Resultant<()>", false, 8),
            ]
        );
    }

    #[test]
    fn responses() {
        let src = "fn a() -> Response {}\nfn b() -> Result<wisp::Response> {}\nfn c() -> Option<Response> {}\n\
                   async fn d(cx: &mut Cx) -> Result<Option<Response>, Error> {}\nfn e() -> Vec<Response> {}\n\
                   fn f() -> Option<Response<u8>> {}\nfn g() -> MyResponse {}";
        let kinds: Vec<_> = top_level_fns(src)
            .iter()
            .map(|f| f.returns_kind())
            .collect();
        use Returns::*;
        assert_eq!(
            kinds,
            [
                Response,
                Response,
                MaybeResponse,
                MaybeResponse,
                Other,
                Other,
                Other
            ]
        );
    }

    #[test]
    fn constants() {
        let src = "pub const BODY_LIMIT: usize = 20 * wisp::MB;\nconst fn f() {}\npub const fn g() {}\nstatic N: AtomicU8 = x;\n\
                   const _: () = ();\nfn h() { const INNER: u8 = 1; }";
        let items = scan(src).unwrap();
        let consts: Vec<_> = items
            .consts
            .iter()
            .map(|c| (c.name.as_str(), c.ty.as_str(), c.line))
            .collect();
        assert_eq!(consts, [("BODY_LIMIT", "usize", 1), ("N", "AtomicU8", 4)]);
        let fns: Vec<_> = items
            .fns
            .iter()
            .map(|f| (f.name.as_str(), f.public))
            .collect();
        assert_eq!(fns, [("f", false), ("g", true), ("h", false)]);
    }

    #[test]
    fn action_marker_does_not_leak_past_items() {
        let src = "#[action] const X: i32 = 1;\npub async fn load() {}";
        assert_eq!(names(src), [("load".into(), false)]);
    }

    #[test]
    fn half_typed_files_do_not_panic() {
        for src in [
            "#[",
            "pub fn a() {}\n#[action\n// café",
            "#![",
            "fn",
            "pub fn load(",
            "#[action] pub fn",
        ] {
            let _ = scan(src);
        }
        assert_eq!(
            names("pub fn a() {}\n#[action\npub fn b() {}"),
            [("a".into(), false)]
        );
    }

    #[test]
    fn any_path_to_action_marks_one() {
        let src = "#[::wisp::action] pub fn a() {}\n#[wisp :: action] pub fn b() {}\n#[my_action] pub fn c() {}";
        assert_eq!(
            names(src),
            [("a".into(), true), ("b".into(), true), ("c".into(), false)]
        );
        let err = scan("mod m {\n    #[action]\n    pub fn a() {}\n}").unwrap_err();
        assert!(
            err.starts_with("2: #[action] marks a top-level function"),
            "{err}"
        );
    }

    #[test]
    fn comments_are_not_parameters() {
        let fns = top_level_fns(
            "pub fn load(/* none */) -> Data {}\npub fn b( // why\n) {}\npub fn c(/* x */ cx: &Cx) {}",
        );
        let takes: Vec<_> = fns.iter().map(takes_cx).collect();
        assert_eq!(takes, [false, false, true]);
    }

    #[test]
    fn data_fields() {
        let src = "pub struct Data {
 /// one, two
 pub a: Vec<(u8, u8)>,
 #[x(a, b)] pub b: String,
 #[doc = r#\"a\", }\"#] pub(crate) c: u8,
 d: u8,
 pub e: fn(u8) -> u8,
 // pub f: u8,
}";
        let got = scan(src).unwrap().data_fields();
        let names: Vec<_> = got.iter().map(|(n, t)| format!("{n}:{t}")).collect();
        assert_eq!(
            names,
            [
                "a:Vec<(u8, u8)>",
                "b:String",
                "c:u8",
                "d:u8",
                "e:fn(u8) -> u8"
            ]
        );
        assert!(
            scan("pub struct Data(pub u8);")
                .unwrap()
                .data_fields()
                .is_empty()
        );
        assert!(scan("pub struct Data;").unwrap().data_fields().is_empty());
    }

    #[test]
    fn load_returns_data() {
        let check = |src: &str| scan(src).unwrap().check();
        assert_eq!(check("struct Data;\nfn load() -> Data { Data }"), Ok(()));
        assert_eq!(
            check("pub struct Data;\npub async fn load(cx: &mut Cx) -> Result<Data> { todo!() }"),
            Ok(())
        );
        assert_eq!(check("fn helper() {}"), Ok(()));
        assert!(
            check("struct Page;\nfn load() -> Page { Page }")
                .unwrap_err()
                .contains("returns Page")
        );
        assert!(
            check("fn load() {}")
                .unwrap_err()
                .contains("returns nothing")
        );
    }

    #[test]
    fn inner_attributes_belong_at_the_top() {
        let check = |src: &str| scan(src).unwrap().check();
        assert_eq!(
            check(
                "//! Docs.
#![allow(dead_code)]
fn a() {}"
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "// Hi
/*! Docs. */
#![allow(x)]
fn a() {}"
            ),
            Ok(())
        );
        assert!(
            check(
                "fn a() {}
#![allow(dead_code)]"
            )
            .unwrap_err()
            .starts_with("2: ")
        );
        assert!(
            check(
                "fn a() {}
//! late"
            )
            .is_err()
        );
        assert_eq!(
            check(
                "/// An item's.
fn a() { let s = \"//!\"; }"
            ),
            Ok(())
        );
        let src = "//! A.
#![allow(x)]

use a;";
        assert_eq!(
            &src[..inner_end(src)],
            "//! A.
#![allow(x)]"
        );
        assert_eq!(
            inner_end(
                "use a;
//! b"
            ),
            0
        );
        assert_eq!(
            inner_end(
                "// only
fn a() {}"
            ),
            0
        );
    }

    #[test]
    fn the_old_error_calls_are_found() {
        let check = |src: &str| scan(src).unwrap().check();
        for src in [
            "fn a() { return Err(error(404, \"x\")); }",
            "fn a() { Err(wisp::redirect(\"/x\")) }",
            "fn a() { x.ok_or(error(404, \"x\"))? }",
            "fn a() { x.ok_or_else(|| error(404, \"x\"))? }",
            "fn a() { x.map_err(|_| error(500, \"x\"))? }",
        ] {
            let e = check(src).unwrap_err();
            assert!(e.contains("Result` inside an `Err`"), "{src}: {e}");
        }
        assert_eq!(check("fn a() { return error(404, \"x\"); }"), Ok(()));
        assert_eq!(check("fn a() { Err(Error::new(404, \"x\")) }"), Ok(()));
        assert_eq!(check("fn a() { Err(redirect_to(1)) }"), Ok(()));
        assert_eq!(check("fn error(s: u8) {} fn a() { Err(error(4)) }"), Ok(()));
    }

    #[test]
    fn blocks_split_into_items_and_statements() {
        let code = "\n/// Kept in memory.\nstatic N: Shared<u8> = Shared::new(0);\n\
                    let n = *N.lock();\n#[action]\nfn add(by: u8) {\n    *N.lock() += by;\n}\n\
                    if n > 3 { return redirect(\"/\"); } else { let _ = 1; }\nuse std::fmt;\n\
                    let s = S { a: 1 };\npub(crate) async fn f() -> u8 { 1 }\nconst C: [u8; 2] = [1, 2];\n\
                    const { () };\nimpl A { fn g() {} }\nlet t = \"fn x() {}\";\n";
        let (items, stmts) = split_items(code);
        assert_eq!(items.len(), code.len());
        assert_eq!(items.lines().count(), code.lines().count());
        let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            words(&items),
            "/// Kept in memory. static N: Shared<u8> = Shared::new(0); #[action] fn add(by: u8) { *N.lock() += by; } \
             use std::fmt; pub(crate) async fn f() -> u8 { 1 } const C: [u8; 2] = [1, 2]; impl A { fn g() {} }"
        );
        assert_eq!(
            words(&stmts),
            "let n = *N.lock(); if n > 3 { return redirect(\"/\"); } else { let _ = 1; } let s = S { a: 1 }; \
             const { () }; let t = \"fn x() {}\";"
        );
        let found = scan(&items).unwrap();
        let add = found.function("add").unwrap();
        assert!(add.action && add.line == 6, "{add:?}");
    }

    #[test]
    fn actions_may_use_cx_without_taking_it() {
        let fns = top_level_fns(
            "#[action] fn a() { cx.flash(\"x\"); }\n#[action] fn b(c: &mut Cx) { cx.x(); }\n\
             #[action] fn c() { let s = \"cx\"; x.cx; wisp::cx(); }\nfn d() { cx.x(); }\n#[action] fn e() { f(S { a: cx }) }",
        );
        let implicit: Vec<bool> = fns.iter().map(|f| f.implicit_cx).collect();
        assert_eq!(implicit, [true, false, false, false, true]);
    }

    #[test]
    fn remote_functions_are_made_as_actions() {
        let fns = top_level_fns(
            "#[remote] fn a(id: u64) -> u64 { id }\n#[wisp::remote(get)] fn b() { cx.x(); }\n\
             #[remote( get )] fn c() { x().await; }\nfn d() {}",
        );
        let kinds: Vec<Option<Remote>> = fns.iter().map(|f| f.remote).collect();
        assert_eq!(
            kinds,
            [
                Some(Remote::Post),
                Some(Remote::Get),
                Some(Remote::Get),
                None
            ]
        );
        assert!(!fns[0].fallible && fns[1].fallible && fns[1].implicit_cx);
        assert!(fns[2].is_async && !fns[2].action);
        let err = scan("mod m {\n    #[remote]\n    fn a() {}\n}").unwrap_err();
        assert!(
            err.starts_with("2: #[remote] marks a top-level function"),
            "{err}"
        );
    }

    #[test]
    fn line_ends() {
        let code = "let a = \"one\ntwo\";\nlet b = 1; // c\n/* x\ny */ let r = r#\"\n\"#;";
        assert_eq!(
            line_ends_in_code(code),
            [false, true, true, false, false, true]
        );
    }

    #[test]
    fn parameter_rules() {
        let f = &top_level_fns(
            "fn a(#[validate(len = 1..=9, email)] mut t: String, #[allow(x)] n: u8) {}",
        )[0];
        assert_eq!(
            f.params,
            [
                ("t".to_string(), "String".to_string()),
                ("n".into(), "u8".into())
            ]
        );
        assert_eq!(
            f.checks,
            [("t".to_string(), "len = 1..=9, email".to_string())]
        );
        let g = &top_level_fns("fn g(id: u64) -> Result<Option<Note>> { todo!() }")[0];
        assert_eq!(g.optional_value(), Some("Note"));
    }

    #[test]
    fn plain_result_is_nothing() {
        let f = &top_level_fns("fn a() -> Result { Ok(()) }")[0];
        assert!(f.fallible && f.returns_kind() == Returns::Nothing);
        // `#[action]` makes one without `->` return `Result`; a plain fn stays.
        let fs = top_level_fns("#[action]\nfn a() { redirect(\"/\") }\nfn b() {}");
        assert!(fs[0].fallible && fs[0].returns_kind() == Returns::Nothing);
        assert!(!fs[1].fallible);
    }

    #[test]
    fn awaits() {
        assert!(super::awaits("db::items().await"));
        assert!(super::awaits("f(x). await ?"));
        assert!(!super::awaits("\"a.await\""));
        assert!(!super::awaits("x.awaited"));
        assert!(super::awaits(
            "f(x)./* c */
 await"
        ));
    }

    #[test]
    fn statements_that_may_wait() {
        let wait = super::may_wait;
        assert!(wait("let a = db::a().await;"));
        assert!(wait(
            "let a = f()
    .await?;"
        ));
        assert!(wait("let (a, b) = tokio::join!(f(), g());"));
        assert!(wait("let a = join!(f(), g());"));
        assert!(wait("let a = get!(f());"));
        assert!(wait("let v = std::vec![1];"));
        assert!(!wait(
            "let a = format!(\"{}\", 1); let v = vec![1]; assert!(a != \"\");"
        ));
        assert!(!wait("let a = x!=y; let s = \"join!(a)\"; // get!(x)"));
        assert!(!wait("let a = f(); /* x.await */"));
    }

    #[test]
    fn unclosed_literals_and_comments_end_the_text() {
        // Found by the fuzz tests: each panicked past the end.
        assert_eq!(super::inner_end("/*! a"), 5);
        assert_eq!(super::inner_end("/*"), 0);
        assert!(!super::awaits("x./* a"));
        assert!(super::scan("fn t()->'\\").is_ok());
        assert!(super::scan("fn r->'\\").is_ok());
    }

    #[test]
    fn parameters() {
        let f = &top_level_fns(
            "fn a(cx: &mut Cx, slug: String, mut n: Option<u32>, f: impl Fn(u8, u8) -> u8,) {}\n\
             fn b<'a>(cx: &'a wisp::Cx, /* c */ tags: Vec<String>) {}\nfn c((a, b): (u8, u8)) {}\nfn d(x: &mut Cxx) {}",
        );
        let params: Vec<_> = f[0]
            .params
            .iter()
            .map(|(p, t)| format!("{p}:{t}"))
            .collect();
        assert_eq!(
            params,
            [
                "cx:&mut Cx",
                "slug:String",
                "n:Option<u32>",
                "f:impl Fn(u8, u8) -> u8"
            ]
        );
        assert_eq!(
            f[0].inputs().unwrap(),
            [
                ("slug", "String"),
                ("n", "Option<u32>"),
                ("f", "impl Fn(u8, u8) -> u8")
            ]
        );
        assert!(takes_cx(&f[1]) && f[1].inputs().unwrap() == [("tags", "Vec<String>")]);
        assert!(
            f[2].inputs()
                .unwrap_err()
                .contains("needs one, like `id: u64`")
        );
        assert!(!takes_cx(&f[3]));
    }

    #[test]
    fn saved_tables_are_named_for_their_statics() {
        let src = "static USERS: Table<U> = Table::saved();\n// Table::saved()\nlet s = \"Table::saved()\";\npub static Post_Items: wisp::Table<P> = Table::saved();\nstatic OLD: Table<U> = Table::saved(\"old\");";
        let want = "static USERS: Table<U> = Table::saved(\"users\");\n// Table::saved()\nlet s = \"Table::saved()\";\npub static Post_Items: wisp::Table<P> = Table::saved(\"post_items\");\nstatic OLD: Table<U> = Table::saved(\"old\");";
        assert_eq!(name_saved(src).as_deref(), Some(want));
        assert_eq!(name_saved("static A: Table<U> = Table::new();"), None);
        assert_eq!(name_saved("let t = Table::saved();"), None);
    }

    #[test]
    fn cx_user_takes_the_table_init_names() {
        let init = "// wisp::users(&other::X)\nfn init() { wisp::users(&db::USERS); }";
        assert_eq!(users_table(init), Ok(Some("db::USERS".into())));
        assert_eq!(users_table("fn init() {}"), Ok(None));
        assert!(users_table("wisp::users(&USERS)").is_err());
        let src = "let a = cx.user()?; let b = \"cx.user()\"; let c = x.cx.user(&T);";
        let want = "let a = cx.user(&db::USERS)?; let b = \"cx.user()\"; let c = x.cx.user(&T);";
        assert_eq!(bind_user(src, Some("db::USERS")), Ok(Some(want.into())));
        assert_eq!(bind_user("cx.user(&T)", None), Ok(None));
        assert!(bind_user(src, None).unwrap_err().contains("wisp::users"));
    }

    #[test]
    fn a_lone_default_is_an_action() {
        let marked = |c| mark_default(c);
        assert_eq!(
            marked("let a = 1;\n    pub async fn default(x: u8) {}\n").as_deref(),
            Some("let a = 1;\n    #[action] pub async fn default(x: u8) {}\n")
        );
        assert!(marked("fn default() -> Result {}").is_some());
        // Not when another says which are the actions, there are two, or it gives data.
        assert_eq!(marked("#[action] fn a() {}\nfn default() {}"), None);
        assert_eq!(marked("fn default() {}\nasync fn default() {}"), None);
        assert_eq!(marked("fn default() -> u32 { 1 }"), None);
        assert_eq!(marked("fn other() {}"), None);
        assert_eq!(marked("#[action]\nfn default() {}"), None);
    }

    #[test]
    fn model_derives_what_actions_read() {
        let items = scan("#[model]\nstruct Post { title: String }").unwrap();
        assert_eq!(items.types[0].derives, ["Json", "FromJson", "Clone"]);
    }
}
