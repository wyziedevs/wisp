//! Typed routes and the Rust places and scopes of browser values.

use super::*;

/// The part of a chain read in the browser that the server sends: its Rust
/// field names, without a last `length` (a list or a string arrives whole,
/// and JavaScript knows its length).
pub(super) fn server_path(path: &[String]) -> Vec<String> {
    let mut p: Vec<String> = path
        .iter()
        .take_while(|s| ty::is_ident(s))
        .cloned()
        .collect();
    if p.len() > 1 && p.last().is_some_and(|l| l == "length") {
        p.pop();
    }
    p
}

/// `pub mod routes`: a function per route, so `routes::blog_post(slug)` is
/// the path `/blog/<slug>` and a link to a route that is gone, or without a
/// parameter it needs, does not compile. Named by the pattern: `/` is
/// `home`, `/blog/[slug]` is `blog_slug`; a name taken gets `_2`, `_3`.
pub(super) fn typed_routes<'a>(
    routes: impl IntoIterator<Item = &'a crate::routes::Route>,
) -> String {
    let ident = |s: &str| {
        let s: String = s
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        match s.starts_with(|c: char| c.is_ascii_digit()) {
            true => format!("r_{s}"),
            false => rust_place(&[s]),
        }
    };
    let mut out = String::from(
        "/// A function per route: `routes::blog_slug(slug)` is `/blog/<slug>`.\n\
         #[allow(dead_code, unused_mut, clippy::all)]\npub mod routes {\n",
    );
    let mut taken: Vec<String> = Vec::new();
    for r in routes {
        let mut name = match r.segs.is_empty() {
            true => "home".to_string(),
            false => (r.segs.iter())
                .map(|s| match s {
                    Seg::Static(n) | Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => {
                        n.replace(['-', '.'], "_")
                    }
                })
                .collect::<Vec<_>>()
                .join("_"),
        };
        let base = name.clone();
        for k in 2.. {
            if !taken.contains(&name) {
                break;
            }
            name = format!("{base}_{k}");
        }
        taken.push(name.clone());
        let mut args = Vec::new();
        let mut body = format!("let mut s = String::from({});", lit(crate::protocol::BASE));
        for seg in &r.segs {
            match seg {
                Seg::Static(n) => {
                    body.push_str(&format!(" s.push_str({});", lit(&format!("/{n}"))))
                }
                Seg::Param(n, _) | Seg::Rest(n) | Seg::Optional(n, _) => {
                    let (n, rest) = (ident(n), matches!(seg, Seg::Rest(_)));
                    match seg {
                        Seg::Optional(..) => {
                            args.push(format!("{n}: Option<impl ::core::fmt::Display>"));
                            body.push_str(&format!(
                                " if let Some({n}) = {n} {{ s.push('/'); ::wisp::rt::path_param(&mut s, &{n}, false); }}"
                            ));
                        }
                        _ => {
                            args.push(format!("{n}: impl ::core::fmt::Display"));
                            body.push_str(&format!(
                                " s.push('/'); ::wisp::rt::path_param(&mut s, &{n}, {rest});"
                            ));
                        }
                    }
                }
            }
        }
        body.push_str(" if s.is_empty() { s.push('/'); } s");
        out.push_str(&format!(
            "    /// `{}`\n    pub fn {}({}) -> String {{ {body} }}\n",
            r.pattern(),
            ident(&name),
            args.join(", ")
        ));
    }
    out.push_str("}\n\n");
    out
}

/// `data.type` is `data.r#type` in Rust.
pub(super) fn rust_place(path: &[String]) -> String {
    const KEYWORDS: [&str; 38] = [
        "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "static", "struct", "trait", "true", "try", "type",
        "unsafe", "use", "where", "while", "yield", "box",
    ];
    let segs: Vec<String> = path
        .iter()
        .map(|s| {
            if KEYWORDS.contains(&s.as_str()) {
                format!("r#{s}")
            } else {
                s.clone()
            }
        })
        .collect();
    segs.join(".")
}

/// A JSON object of the values at `paths` (each with where it is read),
/// nested as they are: `data.a.b` and `data.c` are `{"data":{"a":{"b":…},
/// "c":…}}`. A path under another is covered by it.
pub(super) fn json_tree(paths: &[(Vec<String>, u32)]) -> Vec<Piece> {
    let mut keep: Vec<&(Vec<String>, u32)> = Vec::new();
    for p in paths {
        let covered = paths
            .iter()
            .any(|q| q.0.len() < p.0.len() && p.0.starts_with(&q.0))
            || keep.iter().any(|q| q.0 == p.0);
        if !covered {
            keep.push(p);
        }
    }
    let mut out = Vec::new();
    json_object(&keep, 0, &mut out);
    // Adjacent text as one piece.
    let mut merged: Vec<Piece> = Vec::new();
    for p in out {
        match (merged.last_mut(), p) {
            (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(&b),
            (_, p) => merged.push(p),
        }
    }
    merged
}

pub(super) fn json_object(paths: &[&(Vec<String>, u32)], depth: usize, out: &mut Vec<Piece>) {
    out.push(Piece::Text("{".into()));
    let mut keys: Vec<&str> = Vec::new();
    for p in paths {
        let key = p.0[depth].as_str();
        if keys.contains(&key) {
            continue;
        }
        if !keys.is_empty() {
            out.push(Piece::Text(",".into()));
        }
        keys.push(key);
        out.push(Piece::Text(format!("{}:", js_str(key))));
        let under: Vec<&(Vec<String>, u32)> = paths
            .iter()
            .filter(|q| q.0[depth] == key)
            .copied()
            .collect();
        match under.as_slice() {
            [one] if one.0.len() == depth + 1 => out.push(Piece::Value {
                expr: rust_place(&one.0),
                line: one.1,
            }),
            _ => json_object(&under, depth + 1, out),
        }
    }
    out.push(Piece::Text("}".into()));
}

/// The Rust names bound around each `Live` node: by `{#each}`, `if let`,
/// `{:case}` and `{@const}`.
pub(super) fn rust_scopes(nodes: &[Node], scope: &mut Vec<String>, out: &mut [Vec<String>]) {
    let outer = scope.len();
    for n in nodes {
        match n {
            Node::Const(c) => {
                // `{@const x: T = e}`: the pattern is before `=`, and before a type.
                scope.extend(pattern_names(template::untyped(let_pattern(&c.src))));
            }
            Node::Snippet { params, body, .. } => {
                let k = scope.len();
                for p in params {
                    scope.extend(pattern_names(template::untyped(p)));
                }
                rust_scopes(body, scope, out);
                scope.truncate(k);
            }
            Node::Live { group } => out[*group] = scope.clone(),
            Node::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    let k = scope.len();
                    if let Some(rest) = cond
                        .src
                        .strip_prefix("let")
                        .filter(|r| r.starts_with(char::is_whitespace))
                    {
                        scope.extend(pattern_names(let_pattern(rest)));
                    }
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
                if let Some(o) = otherwise {
                    rust_scopes(o, scope, out);
                }
            }
            Node::Each {
                pat,
                index,
                body,
                otherwise,
                ..
            } => {
                let k = scope.len();
                scope.extend(index.iter().cloned());
                scope.extend(pattern_names(pat));
                rust_scopes(body, scope, out);
                scope.truncate(k);
                if let Some(o) = otherwise {
                    rust_scopes(o, scope, out);
                }
            }
            Node::Match { arms, .. } => {
                for (pat, body) in arms {
                    let k = scope.len();
                    scope.extend(pattern_names(guardless(&pat.src)));
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
            }
            Node::Await {
                pending,
                then,
                catch,
                ..
            } => {
                rust_scopes(pending, scope, out);
                for (pat, body) in then.iter().chain(catch) {
                    let k = scope.len();
                    scope.extend(pattern_names(pat));
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
            }
            Node::Head(body)
            | Node::Component {
                children: Some(body),
                ..
            } => rust_scopes(body, scope, out),
            // Its names end with it, as a block's do.
            Node::Client(branches) => {
                for (group, body) in branches {
                    out[*group] = scope.clone();
                    let k = scope.len();
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
            }
            _ => {}
        }
    }
    scope.truncate(outer);
}

/// The macro a `{#snippet}` of this name compiles to.
pub(super) fn snippet_macro(name: &str) -> String {
    format!("__wisp_snippet_{name}")
}

/// `PAT = EXPR` → `PAT`.
pub(super) fn let_pattern(s: &str) -> &str {
    let b = s.as_bytes();
    let mut eq = None;
    template::for_each_top(s, |i| {
        let plain = b[i] == b'='
            && b.get(i + 1).is_none_or(|&c| c != b'=' && c != b'>')
            && (i == 0 || !matches!(b[i - 1], b'=' | b'!' | b'<' | b'>' | b'.'));
        if plain && eq.is_none() {
            eq = Some(i);
        }
    });
    s[..eq.unwrap_or(s.len())].trim()
}

/// A `{:case}` pattern without its `if` guard.
pub(super) fn guardless(pat: &str) -> &str {
    let b = pat.as_bytes();
    let mut at = None;
    template::for_each_top(pat, |i| {
        let word = b[i..].starts_with(b"if")
            && i > 0
            && b[i - 1].is_ascii_whitespace()
            && b.get(i + 2).is_some_and(|c| c.is_ascii_whitespace());
        if word && at.is_none() {
            at = Some(i);
        }
    });
    pat[..at.unwrap_or(pat.len())].trim()
}

/// The names a Rust pattern binds: `(k, v)`, `Some(x)`, `Point { x, y: py }`,
/// `n @ 1..=5`. Paths, constructors, field names and literals are not names.
pub(super) fn pattern_names(pat: &str) -> Vec<String> {
    let b = pat.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i = template::skip_str(b, i) + 1;
            continue;
        }
        if c == b'\'' {
            i = template::skip_char(b, i) + 1;
            continue;
        }
        if !ty::is_word(c) {
            i += 1;
            continue;
        }
        let start = i;
        i = rust_scan::ident_end(b, i);
        let word = &pat[start..i];
        let (before, after) = (pat[..start].trim_end(), pat[i..].trim_start());
        let lower =
            word.as_bytes()[0].is_ascii_lowercase() || (word.starts_with('_') && word.len() > 1);
        let binds = lower
            && !matches!(word, "ref" | "mut" | "box" | "true" | "false")
            && !before.ends_with("::")
            && !after.starts_with("::")
            && !after.starts_with(['(', '{', '!'])
            && !(after.starts_with(':') && !after.starts_with("::"));
        if binds && !out.iter().any(|o| o == word) {
            out.push(word.to_string());
        }
    }
    out
}
