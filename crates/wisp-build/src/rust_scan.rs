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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub name: String,
    pub action: bool,
    /// `pub`, in any form.
    pub public: bool,
    /// `async fn`: the call is awaited.
    pub is_async: bool,
    /// Every parameter, as its pattern and its type: `("slug", "String")`.
    pub params: Vec<(String, String)>,
    /// Returns a `Result`, so the call ends in `?`.
    pub fallible: bool,
    /// The return type as written, `""` for none.
    pub returns: String,
    /// The line the name is on, from 1.
    pub line: usize,
}

/// A top-level `struct`, `enum`, `union` or `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeItem {
    pub name: String,
    /// The fields of a struct with named fields: name and type.
    pub fields: Vec<(String, String)>,
}

/// A top-level `const` or `static`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub name: String,
    /// The type as written.
    pub ty: String,
    pub line: usize,
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
    /// `//!`), which a file Wisp includes into a module cannot have.
    pub inner: Option<usize>,
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

    /// Why the file cannot be a route file (or `src/hooks.rs`), if it
    /// cannot: Wisp includes it into a module of its own, with the prelude,
    /// so it cannot start with `//!` docs or `#![…]` attributes, and its
    /// `load` must return the `Data` the template reads.
    pub fn check(&self) -> Result<(), String> {
        self.check_inner()?;
        let Some(load) = self.function("load") else {
            return Ok(());
        };
        if !load
            .returns
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
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

    /// The file has no `//!` docs or `#![…]` attributes, which a file Wisp
    /// includes into a module cannot have.
    pub fn check_inner(&self) -> Result<(), String> {
        match self.inner {
            Some(line) => Err(format!(
                "{line}: Wisp includes this file into a module of its own, so it cannot have `//!` docs or `#![…]` \
                 attributes; write `//` comments, or `///` on an item"
            )),
            None => Ok(()),
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
            let name = pat.strip_prefix("mut ").unwrap_or(pat).trim();
            let plain = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && name != "_";
            if !plain {
                return Err(format!(
                    "{}: `{}` takes `{pat}: {ty}`; each parameter but `cx` is read from the request by its name, so it needs one, like `id: u64`",
                    self.line, self.name
                ));
            }
            out.push((name, ty.as_str()));
        }
        Ok(out)
    }

    /// What it returns, looking through a `Result`.
    pub fn returns_kind(&self) -> Returns {
        let mut t = self.returns.trim();
        if self.fallible {
            t = first_arg(t).unwrap_or("?");
        }
        if t.is_empty() || t == "()" {
            Returns::Nothing
        } else if last_segment(t) == "Response" && !t.contains('<') {
            Returns::Response
        } else if last_segment(t) == "Option"
            && first_arg(t).is_some_and(|a| last_segment(a) == "Response" && !a.contains('<'))
        {
            Returns::MaybeResponse
        } else {
            Returns::Other
        }
    }
}

/// A `Cx` parameter's type: `&mut Cx`, `&Cx`, `&'a mut wisp::Cx`.
pub fn is_cx(ty: &str) -> bool {
    let t = ty.trim().strip_prefix('&').unwrap_or(ty).trim_start();
    let t = match t.strip_prefix('\'') {
        Some(rest) => rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_'),
        None => t,
    };
    let t = t
        .trim_start()
        .strip_prefix("mut ")
        .unwrap_or(t.trim_start());
    last_segment(t) == "Cx" && !t.contains('<')
}

/// `Result<Option<Response>, E>` → `Option<Response>`.
fn first_arg(t: &str) -> Option<&str> {
    let open = t.find('<')?;
    let inner = t[open + 1..].strip_suffix('>')?;
    let mut depth = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => return Some(inner[..i].trim()),
            _ => {}
        }
    }
    Some(inner.trim())
}

/// `wisp::Response` → `Response`, `Option<T>` → `Option`.
pub fn last_segment(t: &str) -> &str {
    t.split('<')
        .next()
        .unwrap_or("")
        .trim()
        .rsplit("::")
        .next()
        .unwrap_or("")
        .trim()
}

/// Top-level `fn` and type items in source order. Nested functions,
/// functions inside `impl`/`mod` blocks, comments and string contents are
/// ignored. Fails, with the line, on an `#[action]` that is not on a
/// top-level function, where it would silently do nothing.
pub fn scan(src: &str) -> Result<Items, String> {
    let b = src.as_bytes();
    let mut items = Items::default();
    let mut depth = 0u32;
    let line = |at: usize| src[..at].matches('\n').count() + 1;
    // Seen since the last item ended.
    let mut action = false;
    let mut is_async = false;
    let mut public = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                if depth == 0 && b.get(i + 2) == Some(&b'!') {
                    items.inner.get_or_insert(line(i));
                }
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                if depth == 0 && b.get(i + 2) == Some(&b'!') {
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
                    if depth == 0 {
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
                    i = end;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    (action, is_async, public) = (false, false, false);
                }
            }
            b';' if depth == 0 => (action, is_async, public) = (false, false, false),
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                match &src[start..i] {
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
                            items.consts.push(ConstItem {
                                name: name.to_string(),
                                ty,
                                line: line(at),
                            });
                        }
                    }
                    word @ ("fn" | "struct" | "enum" | "union" | "type") if depth == 0 => {
                        while i < b.len() && b[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        let name_start = i;
                        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                            i += 1;
                        }
                        let name = src[name_start..i].to_string();
                        if name.is_empty() {
                            // `fn(u8) -> u8` as a type, say.
                        } else if word == "fn" {
                            let (params, fallible, returns) = signature(src, i);
                            let line = line(name_start);
                            items.fns.push(FnItem {
                                name,
                                action,
                                public,
                                is_async,
                                params,
                                fallible,
                                returns,
                                line,
                            });
                            (action, is_async, public) = (false, false, false);
                        } else {
                            let fields = if word == "struct" {
                                fields(src, i)
                            } else {
                                Vec::new()
                            };
                            items.types.push(TypeItem { name, fields });
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

/// The fields of the struct whose name ends at `i`, as (name, type).
/// Tuple structs and generic ones have none.
fn fields(src: &str, i: usize) -> Vec<(String, String)> {
    let b = src.as_bytes();
    let open = skip_space(b, i);
    if b.get(open) != Some(&b'{') {
        return Vec::new();
    }
    let (mut out, mut depth, mut start, mut j) = (Vec::new(), 0i32, open + 1, open + 1);
    while j < b.len() {
        match b[j] {
            b'/' if b.get(j + 1) == Some(&b'/') => j = skip_space(b, j) - 1,
            b'/' if b.get(j + 1) == Some(&b'*') => j = skip_block_comment(b, j),
            b'"' => j = skip_str(b, j),
            b'\'' => j = skip_char(b, j),
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b'>' if b[j - 1] == b'-' => {}
            b')' | b']' | b'>' => depth -= 1,
            b',' | b'}' if depth <= 0 => {
                out.extend(field(&src[start..j]));
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
    out
}

/// `name: Type`, after any attributes, comments and visibility (`pub`,
/// `pub(crate)`): the template is compiled inside the module, so it sees
/// private fields too.
fn field(mut s: &str) -> Option<(String, String)> {
    loop {
        s = s.trim_start();
        if let Some(c) = s.strip_prefix("//") {
            s = c.split_once('\n').map_or("", |x| x.1);
        } else if s.starts_with("#[") {
            s = &s[matching_bracket(s.as_bytes(), 1)? + 1..];
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
    Some((name.trim().to_string(), ty.trim().to_string()))
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

/// Reads a signature from just past the function's name up to its body:
/// its parameters, whether its return type is a `Result` (`Result<T>`,
/// `wisp::Result<T>`, `io::Result<T>`, ...), and that type.
fn signature(src: &str, mut i: usize) -> (Vec<(String, String)>, bool, String) {
    let b = src.as_bytes();
    let mut depth = 0i32; // (), [] and <>
    // Where the parameter list starts, until it has been read.
    let mut open = None;
    let mut read = false;
    let mut params = Vec::new();
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
                    params = split_top(&text).into_iter().map(param).collect();
                }
            }
            b'{' | b';' if depth == 0 => break,
            b'\'' => i = skip_char(b, i),
            _ => {}
        }
        i += 1;
    }
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
    (params, fallible, returns.to_string())
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

/// The index of the first byte at or after `i` that is not whitespace or
/// in a comment.
fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i = skip_block_comment(b, i) + 1;
        } else {
            break;
        }
    }
    i
}

/// The end of the identifier starting at `i` (`i` itself if there is none).
fn ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    i
}

fn skip_block_comment(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if b[i] == b'*' && b[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i - 1;
            }
        } else {
            i += 1;
        }
    }
    b.len()
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
            b'"' => i = skip_str(b, i),
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
 pub(crate) c: u8,
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
    fn inner_attributes_are_refused() {
        let check = |src: &str| scan(src).unwrap().check();
        assert!(
            check("//! Docs.\nfn a() {}")
                .unwrap_err()
                .starts_with("1: Wisp includes this file")
        );
        assert!(
            check("fn a() {}\n#![allow(dead_code)]")
                .unwrap_err()
                .starts_with("2: ")
        );
        assert!(check("/*! Docs. */").is_err());
        assert_eq!(check("/// An item's.\nfn a() { let s = \"//!\"; }"), Ok(()));
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
                "mut n:Option<u32>",
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
}
