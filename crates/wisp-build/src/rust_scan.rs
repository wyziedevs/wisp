//! Just enough Rust lexing to list a file's top-level functions and types.
//!
//! The build step needs to know whether `+page.rs` defines `load`, which
//! functions carry `#[action]`, and which HTTP methods `+server.rs` defines.
//! Of a signature it reads only what shapes the call: `pub` or not, `async`
//! or not, `cx` or no parameters, `Result` or a plain value, plus the return
//! type's text. That is enough to catch the usual mistakes here, against
//! the user's file, rather than by rustc in generated code: a private
//! `load`, an action that returns a value, a `Data` the template cannot see.
//! Other types are left to rustc.

use crate::template::{raw_str_start, skip_char, skip_raw_str, skip_str};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub name: String,
    pub action: bool,
    /// `pub`, in any form: the generated code, outside the module, can call it.
    pub public: bool,
    /// `async fn`: the call is awaited.
    pub is_async: bool,
    /// Has parameters, so it is passed `cx`.
    pub takes_cx: bool,
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
    pub public: bool,
    pub line: usize,
    /// The `pub` fields of a struct with named fields: name and type.
    pub fields: Vec<(String, String)>,
}

/// A top-level `const` or `static`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    pub name: String,
    pub public: bool,
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
}

impl Items {
    pub fn function(&self, name: &str) -> Option<&FnItem> {
        self.fns.iter().find(|f| f.name == name)
    }

    /// The `pub` fields of `Data`, which a template can use by name.
    pub fn data_fields(&self) -> Vec<(String, String)> {
        let data = self.types.iter().find(|t| t.name == "Data");
        data.map(|t| t.fields.clone()).unwrap_or_default()
    }

    pub fn constant(&self, name: &str) -> Option<&ConstItem> {
        self.consts.iter().find(|c| c.name == name)
    }

    /// Why `load` and the `Data` it returns cannot be used by the page's
    /// template, if they cannot.
    pub fn check_load(&self) -> Result<(), String> {
        let Some(load) = self.function("load") else {
            return Ok(());
        };
        load.check_public()?;
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
        if let Some(t) = self.types.iter().find(|t| t.name == "Data" && !t.public) {
            return Err(format!(
                "{}: `Data` must be `pub` (and so must its fields) for the template to read it",
                t.line
            ));
        }
        Ok(())
    }
}

impl FnItem {
    /// Generated code, outside the module, calls it: it must be `pub`.
    pub fn check_public(&self) -> Result<(), String> {
        if self.public {
            Ok(())
        } else {
            Err(format!(
                "{}: `{}` must be `pub` for Wisp to call it: `pub fn {}`",
                self.line, self.name, self.name
            ))
        }
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
fn last_segment(t: &str) -> &str {
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
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block_comment(b, i),
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            b'#' => {
                // Attribute: #[path] or #![path]. Record the marker, skip the rest.
                let mut j = i + 1;
                if b.get(j) == Some(&b'!') {
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
                                public,
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
                            let (takes_cx, fallible, returns) = signature(src, i);
                            let line = line(name_start);
                            items.fns.push(FnItem {
                                name,
                                action,
                                public,
                                is_async,
                                takes_cx,
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
                            items.types.push(TypeItem {
                                name,
                                public,
                                line: line(name_start),
                                fields,
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

/// The `pub` fields of the struct whose name ends at `i`, as (name, type).
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

/// `pub name: Type`, after any attributes and comments; `None` if not `pub`.
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
    let rest = s.strip_prefix("pub")?;
    // `pub(crate)` is not visible to the generated code's module.
    let (name, ty) = rest.strip_prefix(char::is_whitespace)?.split_once(':')?;
    Some((name.trim().to_string(), ty.trim().to_string()))
}

/// Reads a signature from just past the function's name up to its body:
/// whether it has parameters, whether its return type is a `Result`
/// (`Result<T>`, `wisp::Result<T>`, `io::Result<T>`, ...), and that type.
fn signature(src: &str, mut i: usize) -> (bool, bool, String) {
    let b = src.as_bytes();
    let mut depth = 0i32; // (), [] and <>
    let mut params = None;
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
                if depth == 0 && params.is_none() {
                    params = Some(b.get(skip_space(b, i + 1)) != Some(&b')'));
                }
                depth += 1;
            }
            b'[' | b'<' => depth += 1,
            b')' | b']' | b'>' => depth -= 1,
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
    (params.unwrap_or(false), fallible, returns.to_string())
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
            .map(|f| (f.name, f.is_async, f.takes_cx, f.fallible))
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
            .map(|c| (c.name.as_str(), c.public, c.ty.as_str(), c.line))
            .collect();
        assert_eq!(
            consts,
            [
                ("BODY_LIMIT", true, "usize", 1),
                ("N", false, "AtomicU8", 4)
            ]
        );
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
        let takes: Vec<_> = fns.iter().map(|f| f.takes_cx).collect();
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
        assert_eq!(names, ["a:Vec<(u8, u8)>", "b:String", "e:fn(u8) -> u8"]);
        assert!(
            scan("pub struct Data(pub u8);")
                .unwrap()
                .data_fields()
                .is_empty()
        );
        assert!(scan("pub struct Data;").unwrap().data_fields().is_empty());
    }

    #[test]
    fn load_and_data_must_be_visible() {
        let check = |src: &str| scan(src).unwrap().check_load();
        assert_eq!(
            check("pub struct Data;\npub fn load() -> Data { Data }"),
            Ok(())
        );
        assert_eq!(
            check(
                "pub struct Data;\npub(crate) async fn load(cx: &mut Cx) -> Result<Data> { todo!() }"
            ),
            Ok(())
        );
        assert_eq!(check("pub fn helper() {}"), Ok(()));
        assert!(
            check("pub struct Data;\nfn load() -> Data { Data }")
                .unwrap_err()
                .starts_with("2: `load` must be `pub`")
        );
        assert!(
            check("struct Data;\npub fn load() -> Data { Data }")
                .unwrap_err()
                .starts_with("1: `Data` must be `pub`")
        );
        assert!(
            check("pub struct Page;\npub fn load() -> Page { Page }")
                .unwrap_err()
                .contains("returns Page")
        );
        assert!(
            check("pub fn load() {}")
                .unwrap_err()
                .contains("returns nothing")
        );
    }
}
