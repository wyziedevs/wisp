//! `#[action]` marks a `+page.rs` function as a form action,
//! `#[derive(Cookie)]` lets a type be kept in a cookie, `#[derive(Json)]`
//! lets browser code and API clients read it, `#[derive(FromJson)]` reads
//! it from a JSON request body, and `#[derive(Rest)]` does both and keeps
//! it in a table a `+server.rs` serves.
//!
//! No `syn`, no `quote`: this crate compiles instantly. `#[action]` returns
//! its input as it is (`wisp-build` finds it in the source and generates
//! the route), but for adding `cx` to one that uses it without taking it,
//! `-> Result` to one without a return type and `async` to one that
//! `.await`s; the derives read just enough of a type to know its name and
//! its fields' names, and `#[validate]`'s rules as the build reads them
//! (`wisp_shared::rules`).

use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn action(attr: TokenStream, item: TokenStream) -> TokenStream {
    if let Some(first) = attr.into_iter().next() {
        // The function stays, so the only error is this one.
        let mut out = error("#[action] takes no arguments", first.span());
        out.extend(item);
        return out;
    }
    implicit_cx(item)
}

/// `#[remote]` (or `#[remote(get)]`) marks a function browser code calls as
/// `await name(args)`: `wisp-build` finds it and serves it at
/// `/_app/r/<hash>`. The function is made as an action's is.
#[proc_macro_attribute]
pub fn remote(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args: Vec<TokenTree> = attr.into_iter().collect();
    match args.as_slice() {
        [] => {}
        [TokenTree::Ident(i)] if i.to_string() == "get" => {}
        [first, ..] => {
            let mut out = error(
                "#[remote] takes nothing, or `get`: #[remote(get)]",
                first.span(),
            );
            out.extend(item);
            return out;
        }
    }
    implicit_cx(item)
}

/// A parameter list without its `#[validate(...)]` attributes, which
/// `wisp-build` reads to check the input before the call.
fn without_rules(params: TokenStream) -> Vec<TokenTree> {
    let tokens: Vec<TokenTree> = params.into_iter().collect();
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if let (TokenTree::Punct(p), Some(TokenTree::Group(g))) = (&tokens[i], tokens.get(i + 1))
            && p.as_char() == '#'
            && g.delimiter() == Delimiter::Bracket
            && matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(id)) if id.to_string() == "validate")
        {
            i += 2;
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out
}

/// An action whose body uses `cx` but does not take it gets it: `cx: &mut
/// Cx` as its first parameter (`wisp-build` sees the same and passes it).
/// One without `->` returns `Result`, so its body may end in `redirect("/")`
/// or use `?`. One whose body has `.await` is `async` (`wisp-build` awaits
/// it). Its parameters' `#[validate(...)]` rules leave.
fn implicit_cx(item: TokenStream) -> TokenStream {
    let mut tokens: Vec<TokenTree> = item.into_iter().collect();
    let Some(f) = tokens
        .iter()
        .position(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "fn"))
    else {
        return tokens.into_iter().collect();
    };
    // The first `(…)` after the name and its generics: `<F: Fn(u8) -> u8>` has
    // groups of its own.
    let params = angles(&tokens[f..], false)
        .find(|&(i, d)| d <= 0 && matches!(&tokens[f + i], TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis))
        .map(|(i, _)| i);
    let body = tokens
        .iter()
        .rposition(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace));
    let (Some(p), Some(b)) = (params.map(|p| p + f), body) else {
        return tokens.into_iter().collect();
    };
    let (TokenTree::Group(pg), TokenTree::Group(bg)) = (&tokens[p], &tokens[b]) else {
        return tokens.into_iter().collect();
    };
    let span = pg.span();
    let (body_stream, body_span) = (bg.stream(), bg.span());
    // As the build reads the same body (`wisp_shared::rust::awaits`).
    let make_async = wisp_shared::rust::awaits(&body_stream.to_string())
        && !tokens[..f]
            .iter()
            .any(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "async"));
    let fn_span = tokens[f].span();
    let rest = without_rules(pg.stream());
    let mut stream: Vec<TokenTree> = Vec::new();
    if !names(&pg.stream(), &["cx", "Cx"]) && names(&bg.stream(), &["cx"]) {
        stream.extend(
            "#[allow(unused_variables)] cx: &mut ::wisp::Cx"
                .parse::<TokenStream>()
                .expect("valid tokens"),
        );
        if !rest.is_empty() {
            stream.push(Punct::new(',', Spacing::Alone).into());
        }
    }
    stream.extend(rest);
    let mut group = Group::new(Delimiter::Parenthesis, stream.into_iter().collect());
    group.set_span(span);
    tokens[p] = group.into();
    // No `->`: it returns `Result`, whatever its body ends in (`wisp::rt_traits::Done`).
    // Right after the parameters: an arrow in a `where` clause's `Fn(u8) -> u8` is not one.
    let arrow = matches!(
        (tokens.get(p + 1), tokens.get(p + 2)),
        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(c))) if a.as_char() == '-' && c.as_char() == '>'
    );
    if !arrow {
        let mut inner = Group::new(Delimiter::Brace, returns_done(body_stream));
        inner.set_span(body_span);
        let mut body = Group::new(Delimiter::Brace, done(TokenTree::from(inner).into()));
        body.set_span(body_span);
        tokens[b] = body.into();
        let ret = parse("-> ::wisp::Result<()>");
        tokens.splice(p + 1..p + 1, ret);
    }
    if make_async {
        tokens.insert(f, Ident::new("async", fn_span).into());
    }
    tokens.into_iter().collect()
}

/// `::wisp::rt_traits::done(value)`.
fn done(value: TokenStream) -> TokenStream {
    let mut call = parse("::wisp::rt_traits::done");
    call.extend([TokenTree::from(Group::new(Delimiter::Parenthesis, value))]);
    call
}

/// The body of an action without `->`, each `return x` in it made
/// `return done(x)` (a bare `return` is `done(())`), so it returns nothing or
/// a `Result` alike. Closures, `async` blocks and inner functions keep their
/// own `return`s.
fn returns_done(body: TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = body.into_iter().collect();
    let mut out: Vec<TokenTree> = Vec::with_capacity(tokens.len());
    // The next `{…}` is a function's or a closure's of its own (after `fn` or `->`).
    let mut inner_fn = false;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Ident(id) if id.to_string() == "fn" => inner_fn = true,
            // `|x| -> u8 { … }`: the block after a return type is a closure's own.
            TokenTree::Punct(p)
                if p.as_char() == '>'
                    && matches!(out.last(), Some(TokenTree::Punct(d)) if d.as_char() == '-' && d.spacing() == Spacing::Joint) =>
            {
                inner_fn = true
            }
            // `let f: fn(u8) -> u8 = g;` was a type, not a function.
            TokenTree::Punct(p) if p.as_char() == ';' => inner_fn = false,
            TokenTree::Ident(id) if id.to_string() == "return" => {
                // Up to the statement's `;`, the arm's `,` or the block's end.
                // A `,` inside a turbofish (`f::<A, B>()`) does not end it.
                let end = angles(&tokens[i + 1..], true)
                    .find(|&(j, d)| match &tokens[i + 1 + j] {
                        TokenTree::Punct(p) => p.as_char() == ';' || (p.as_char() == ',' && d <= 0),
                        _ => false,
                    })
                    .map_or(tokens.len(), |(j, _)| i + 1 + j);
                let value: TokenStream = tokens[i + 1..end].iter().cloned().collect();
                let value = if value.is_empty() {
                    parse("()")
                } else {
                    returns_done(value)
                };
                out.push(tokens[i].clone());
                out.extend(done(value));
                i = end;
                continue;
            }
            TokenTree::Group(g) => {
                let own = match out.last() {
                    Some(TokenTree::Punct(p)) => p.as_char() == '|',
                    Some(TokenTree::Ident(id)) => {
                        matches!(id.to_string().as_str(), "async" | "move")
                    }
                    _ => false,
                } || (inner_fn && g.delimiter() == Delimiter::Brace);
                if g.delimiter() == Delimiter::Brace {
                    inner_fn = false;
                }
                if !own {
                    let mut n = Group::new(g.delimiter(), returns_done(g.stream()));
                    n.set_span(g.span());
                    out.push(n.into());
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out.into_iter().collect()
}

/// Whether one of `idents` is in `s` as a name of its own, not a field or
/// a path's later segment (`a.cx`, `x::cx`).
fn names(s: &TokenStream, idents: &[&str]) -> bool {
    // The last two punctuation characters just before, if any.
    let mut before = [' ', ' '];
    for t in s.clone() {
        let own = before[1] != '.' && before != [':', ':'];
        match &t {
            TokenTree::Ident(i) if own && idents.contains(&i.to_string().as_str()) => {
                return true;
            }
            TokenTree::Group(g) if names(&g.stream(), idents) => return true,
            _ => {}
        }
        let c = match &t {
            TokenTree::Punct(p) => p.as_char(),
            _ => ' ',
        };
        before = [before[1], c];
    }
    false
}

/// `#[model]` on a struct: `Json`, `FromJson` and `Clone` derived, and the
/// struct and each field `pub`, so it is a table's row type, an action's
/// input and a template's value at once. A struct with a borrowed field
/// (`name: &'static str`, static data) gets no `FromJson`. Not with its own
/// `derive(Clone)`.
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut out = match attr.into_iter().next() {
        Some(first) => error("#[model] takes no arguments", first.span()),
        None => TokenStream::new(),
    };
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let is = |t: &TokenTree, word: &str| matches!(t, TokenTree::Ident(i) if i.to_string() == word);
    let at = tokens.iter().position(|t| is(t, "struct"));
    let named = at.is_some()
        && matches!(tokens.last(), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace);
    // A borrowed field (`&'static str` of static data) cannot be read from
    // a body, so such a model is `Json` and `Clone` only.
    let fields = named.then(|| named_fields(&tokens.iter().cloned().collect()).ok());
    let borrowed = (fields.flatten().into_iter().flatten()).any(|f| f.ty.starts_with('&'));
    out.extend(parse(if borrowed {
        "#[derive(::wisp::Json, ::std::clone::Clone)]"
    } else {
        "#[derive(::wisp::Json, ::wisp::FromJson, ::std::clone::Clone)]"
    }));
    // A model is shared by routes, so it is `pub` without saying so.
    let private = at.is_some_and(|at| !tokens[..at].iter().any(|t| is(t, "pub")));
    let last = tokens.len().wrapping_sub(1);
    let account = named.then(|| account(&tokens)).flatten();
    for (k, t) in tokens.into_iter().enumerate() {
        match t {
            TokenTree::Group(g) if named && k == last => {
                let mut group = Group::new(Delimiter::Brace, public(g.stream()));
                group.set_span(g.span());
                out.extend([TokenTree::from(group)]);
            }
            t if private && Some(k) == at => {
                out.extend(parse("pub"));
                out.extend([t]);
            }
            t => out.extend([t]),
        }
    }
    out.extend(account);
    out
}

/// `impl wisp::Account` for a struct with a `hash` field and an `email` or
/// a `name`.
fn account(tokens: &[TokenTree]) -> Option<TokenStream> {
    let at = tokens
        .iter()
        .position(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "struct"))?;
    let TokenTree::Ident(name) = tokens.get(at + 1)? else {
        return None;
    };
    let fields = named_fields(&tokens.iter().cloned().collect()).ok()?;
    let has = |f: &str| fields.iter().any(|x| x.name.to_string() == f);
    let who = ["email", "name"].into_iter().find(|f| has(f))?;
    // A `Password` field, or else a `hash: String`.
    let typed = fields
        .iter()
        .find(|f| f.ty.rsplit("::").next().map(str::trim) == Some("Password"));
    let (hash, set, hashed) = match typed {
        Some(f) => {
            let p = &f.name;
            (
                format!("self.{p}.as_str()"),
                format!("self.{p} = ::wisp::Password::from_hash(hash);"),
                format!("fn hashed(&self) -> bool {{ self.{p}.hashed() }}"),
            )
        }
        None if has("hash") => (
            "&self.hash".into(),
            "self.hash = hash;".into(),
            String::new(),
        ),
        None => return None,
    };
    Some(parse(&format!(
        "impl ::wisp::Account for {name} {{ const WHO: &'static str = {who:?}; \
         fn who(&self) -> &str {{ ::std::convert::AsRef::<str>::as_ref(&self.{who}) }} \
         fn hash(&self) -> &str {{ {hash} }} \
         fn set_hash(&mut self, hash: String) {{ {set} }} {hashed} }}"
    )))
}

/// A struct's fields with `pub` before each that has no visibility.
fn public(fields: TokenStream) -> TokenStream {
    let mut out = Vec::new();
    for field in items(fields) {
        let rest = skip_attributes(&field);
        out.extend_from_slice(&field[..field.len() - rest.len()]);
        if !matches!(rest.first(), Some(TokenTree::Ident(i)) if i.to_string() == "pub") {
            out.extend(parse("pub"));
        }
        out.extend_from_slice(rest);
        out.push(Punct::new(',', Spacing::Alone).into());
    }
    out.into_iter().collect()
}

/// Implements `Display` and `FromStr` for a struct whose fields do, or for
/// an enum without fields, in a form a cookie can hold: the fields in order,
/// separated by `|`, each escaped (`42|cranesloth|pi`); a variant's name.
/// Then `cx.set_cookie("game", game)` stores it and `cx.cookie_or("game",
/// Game::new())` reads it back, falling back to the default when the cookie
/// is missing or not one.
#[proc_macro_derive(Cookie)]
pub fn derive_cookie(item: TokenStream) -> TokenStream {
    match cookie(item) {
        Ok(code) => code,
        Err((msg, span)) => error(&msg, span),
    }
}

/// Implements `wisp::Json`, so a template's client script or directives can
/// read the value: a struct is an object of its fields (a tuple struct an
/// array, one with a single field that field's value, a unit struct
/// `null`), and an enum without fields is its variant's name.
#[proc_macro_derive(Json)]
pub fn derive_json(item: TokenStream) -> TokenStream {
    match json(item) {
        Ok(code) => code,
        Err((msg, span)) => error(&msg, span),
    }
}

/// Implements `wisp::FromJson`, so an endpoint can take the type as its JSON
/// body (`fn post(body: Note)`): a struct is read from an object of its
/// fields (an `Option` field may be left out or `null`, a `bool` one is
/// `false` then; other fields are ignored), a tuple struct with one field
/// as that field, and an enum without fields from its variant's name.
///
/// `#[validate(...)]` on a field checks it once read: `min = 0`, `max = 10`
/// (numbers), `len = 1..=200` or `min_len = 1`, `max_len = 200` (characters
/// of a string, items of a list), `email`. Every problem is reported, by field, in one 422.
///
/// A struct with named fields may also be an action's input, `#[action] fn
/// save(post: Post)`: its fields are read from the form by name (a blank
/// one is missing) and checked the same way.
#[proc_macro_derive(FromJson, attributes(validate, json, unique))]
pub fn derive_from_json(item: TokenStream) -> TokenStream {
    match from_json(item) {
        Ok(code) => code,
        Err((msg, span)) => error(&msg, span),
    }
}

/// `compile_error!(msg);`, reported at `span`.
fn error(msg: &str, span: Span) -> TokenStream {
    let mut text = Literal::string(msg);
    text.set_span(span);
    let mut args = Group::new(Delimiter::Parenthesis, TokenTree::from(text).into());
    args.set_span(span);
    let mut bang = Punct::new('!', Spacing::Alone);
    bang.set_span(span);
    let mut semi = Punct::new(';', Spacing::Alone);
    semi.set_span(span);
    TokenStream::from_iter([
        TokenTree::from(Ident::new("compile_error", span)),
        bang.into(),
        args.into(),
        semi.into(),
    ])
}

enum Shape {
    /// The fields' names.
    Named(Vec<Ident>),
    /// Where each field is, to report a type that cannot be in a cookie.
    Tuple(Vec<Span>),
    Unit,
    Enum(Vec<String>),
}

type Error = (String, Span);

/// The name and shape of the type that a derive (`what`, such as `Cookie`)
/// is on.
fn read_type(item: TokenStream, what: &str) -> Result<(Ident, Shape), Error> {
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let mut i = 0;
    // Attributes and visibility, up to `struct` or `enum`.
    let kind = loop {
        match tokens.get(i) {
            Some(TokenTree::Ident(id))
                if id.to_string() == "struct" || id.to_string() == "enum" =>
            {
                break id.to_string();
            }
            Some(_) => i += 1,
            None => {
                return Err((
                    format!("#[derive({what})] works on a struct or an enum"),
                    Span::call_site(),
                ));
            }
        }
    };
    let Some(TokenTree::Ident(name)) = tokens.get(i + 1) else {
        return Err(("expected a name".into(), tokens[i].span()));
    };
    if let Some(TokenTree::Punct(p)) = tokens.get(i + 2)
        && p.as_char() == '<'
    {
        return Err((
            format!("#[derive({what})] does not take generic types"),
            p.span(),
        ));
    }
    if let Some(w) = tokens[i + 2..]
        .iter()
        .find(|t| matches!(t, TokenTree::Ident(id) if id.to_string() == "where"))
    {
        return Err((
            format!("#[derive({what})] does not take `where` clauses"),
            w.span(),
        ));
    }
    let shape = match tokens.get(i + 2) {
        Some(TokenTree::Group(g)) if kind == "enum" && g.delimiter() == Delimiter::Brace => {
            let mut names = Vec::new();
            for v in items(g.stream()) {
                match skip_attributes(&v) {
                    [TokenTree::Ident(id)] => names.push(id.to_string()),
                    [TokenTree::Ident(id), TokenTree::Punct(eq), ..] if eq.as_char() == '=' => {
                        names.push(id.to_string())
                    }
                    rest => {
                        return Err((
                            format!(
                                "#[derive({what})] works on enums whose variants have no fields"
                            ),
                            rest.first().unwrap_or(&v[0]).span(),
                        ));
                    }
                }
            }
            if names.is_empty() {
                return Err((
                    format!("#[derive({what})] needs an enum with at least one variant"),
                    name.span(),
                ));
            }
            Shape::Enum(names)
        }
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
            let mut names = Vec::new();
            for field in items(g.stream()) {
                // `#[attr] pub(crate) name: Type`: the name is the last ident before the first `:`.
                let colon = field
                    .iter()
                    .position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ':'));
                match colon.and_then(|c| field[..c].last()) {
                    Some(TokenTree::Ident(id)) => names.push(id.clone()),
                    _ => return Err(("expected `name: Type`".into(), field[0].span())),
                }
            }
            Shape::Named(names)
        }
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => Shape::Tuple(
            items(g.stream())
                .iter()
                .map(|f| {
                    skip_visibility(skip_attributes(f))
                        .first()
                        .unwrap_or(&f[0])
                        .span()
                })
                .collect(),
        ),
        _ => Shape::Unit,
    };
    Ok((name.clone(), shape))
}

fn cookie(item: TokenStream) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item, "Cookie")?;
    let name = &name;

    // A value with no fields is written as nothing, and only nothing reads
    // back as one.
    let (write, read) = match &shape {
        Shape::Named(fields) if !fields.is_empty() => (
            fields
                .iter()
                .map(|f| at(&format!("w.field(&self.{f})?;"), f.span()))
                .collect(),
            TokenStream::from_iter([
                TokenTree::from(name.clone()),
                Group::new(
                    Delimiter::Brace,
                    fields
                        .iter()
                        .map(|f| at(&format!("{f}: r.field()?,"), f.span()))
                        .collect(),
                )
                .into(),
            ]),
        ),
        Shape::Tuple(fields) if !fields.is_empty() => (
            fields
                .iter()
                .enumerate()
                .map(|(i, &span)| at(&format!("w.field(&self.{i})?;"), span))
                .collect(),
            TokenStream::from_iter([
                TokenTree::from(name.clone()),
                Group::new(
                    Delimiter::Parenthesis,
                    fields.iter().map(|&span| at("r.field()?,", span)).collect(),
                )
                .into(),
            ]),
        ),
        Shape::Named(_) => return Ok(empty(name, &format!("{name} {{}}"))),
        Shape::Tuple(_) => return Ok(empty(name, &format!("{name}()"))),
        Shape::Unit => return Ok(empty(name, &name.to_string())),
        Shape::Enum(variants) => {
            let write = format!(
                "w.field(match self {{ {} }})?;",
                variants
                    .iter()
                    .map(|v| format!("{name}::{v} => \"{v}\","))
                    .collect::<String>()
            );
            let read = format!(
                "match &*r.text()? {{ {} _ => return ::std::result::Result::Err(::wisp::rt::BadCookie) }}",
                variants
                    .iter()
                    .map(|v| format!("\"{v}\" => {name}::{v},"))
                    .collect::<String>()
            );
            (parse(&write), parse(&read))
        }
    };
    let template = parse(&format!(
        "impl ::std::fmt::Display for {name} {{
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {{
                let mut w = ::wisp::rt::CookieWriter::new(f);
                __wisp_write
                ::std::result::Result::Ok(())
            }}
        }}
        impl ::std::str::FromStr for {name} {{
            type Err = ::wisp::rt::BadCookie;
            fn from_str(s: &str) -> ::std::result::Result<Self, ::wisp::rt::BadCookie> {{
                let mut r = ::wisp::rt::CookieReader::new(s);
                let value = __wisp_read;
                r.end()?;
                ::std::result::Result::Ok(value)
            }}
        }}"
    ));
    Ok(fill(template, &write, &read))
}

fn json(item: TokenStream) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item, "Json")?;
    // `out.push_str("…");` with JSON text.
    let text = |s: &str| parse(&format!("out.push_str({s:?});"));
    // A field's own JSON, placed at the field so that one whose type is not
    // `Json` is the one the compiler points at.
    let value =
        |field: &str, span: Span| at(&format!("::wisp::Json::json(&self.{field}, out);"), span);
    let bare = |id: &str| id.strip_prefix("r#").unwrap_or(id).to_string();
    let mut body = TokenStream::new();
    match &shape {
        Shape::Named(fields) => {
            body.extend(text("{"));
            for (k, f) in fields.iter().enumerate() {
                let comma = if k > 0 { "," } else { "" };
                body.extend(text(&format!("{comma}\"{}\":", bare(&f.to_string()))));
                body.extend(value(&f.to_string(), f.span()));
            }
            body.extend(text("}"));
        }
        Shape::Tuple(fields) if fields.len() == 1 => body.extend(value("0", fields[0])),
        Shape::Tuple(fields) => {
            body.extend(text("["));
            for (k, &span) in fields.iter().enumerate() {
                if k > 0 {
                    body.extend(text(","));
                }
                body.extend(value(&k.to_string(), span));
            }
            body.extend(text("]"));
        }
        Shape::Unit => body.extend(text("null")),
        Shape::Enum(variants) => {
            let arms: String = variants
                .iter()
                .map(|v| format!("{name}::{v} => {:?},", format!("\"{}\"", bare(v))))
                .collect();
            body.extend(parse(&format!("out.push_str(match self {{ {arms} }});")));
        }
    }
    let template = parse(&format!(
        "impl ::wisp::Json for {name} {{
            fn json(&self, out: &mut ::std::string::String) {{
                __wisp_write
            }}
        }}"
    ));
    let mut out = fill(template, &body, &TokenStream::new());
    out.extend(parse(&ts(&name.to_string(), &shape)));
    Ok(out)
}

/// `wisp::ts::Ts`, which only `wisp check --types` builds have: the type's
/// TypeScript by its name, `interface Note { title: string; }`. Closures
/// read the fields, so the compiler names their types.
fn ts(name: &str, shape: &Shape) -> String {
    let bare = |id: &str| id.strip_prefix("r#").unwrap_or(id).to_string();
    // A field whose type has no TypeScript is `unknown`.
    let field = |f: &str| format!("&(&&::wisp::ts::field(|s: &{name}| &s.{f})).ts(d)");
    // A `s.push_str(…);` for each piece: text, or a field's type.
    let mut body = String::new();
    let mut push = |code: String| body.push_str(&format!("s.push_str({code});"));
    let text = |t: &str| format!("{t:?}");
    match shape {
        Shape::Named(fields) => {
            push(text(&format!("interface {name} {{")));
            for f in fields {
                push(text(&format!(" {}: ", bare(&f.to_string()))));
                push(field(&f.to_string()));
                push(text(";"));
            }
            push(text(" }"));
        }
        Shape::Tuple(fields) => {
            let many = fields.len() > 1;
            push(text(&format!(
                "type {name} = {}",
                if many { "[" } else { "" }
            )));
            for k in 0..fields.len() {
                if k > 0 {
                    push(text(", "));
                }
                push(field(&k.to_string()));
            }
            push(text(if many { "];" } else { ";" }));
        }
        Shape::Unit => push(text(&format!("type {name} = null;"))),
        Shape::Enum(variants) => {
            let names: Vec<String> = variants
                .iter()
                .map(|v| format!("\"{}\"", bare(v)))
                .collect();
            push(text(&format!("type {name} = {};", names.join(" | "))));
        }
    }
    let object = matches!(shape, Shape::Named(_));
    format!(
        "::wisp::__ts! {{
            impl ::wisp::ts::Ts for {name} {{
                type Static = {name};
                const OBJECT: bool = {object};
                fn ts(d: &mut ::wisp::ts::Decls) -> ::std::string::String {{
                    use ::wisp::ts::{{ViaAny as _, ViaTs as _}};
                    d.named({name:?}, |d| {{
                        let mut s = ::std::string::String::new();
                        {body}
                        s
                    }})
                }}
            }}
        }}"
    )
}

fn from_json(item: TokenStream) -> Result<TokenStream, Error> {
    from_json_with(item, &[])
}

/// `FromJson`, with the fields named in `stamped` set to now when they are
/// left out (`#[derive(Rest)]`'s `created_at` and `updated_at`).
fn from_json_with(item: TokenStream, stamped: &[&str]) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item.clone(), "FromJson")?;
    let bare = |id: &str| id.strip_prefix("r#").unwrap_or(id).to_string();
    // Straight from the text, for a struct of named fields (not a stamped
    // one): each key once, in a single pass, then the checks. Anything
    // else leaves it to `from_json` (see `wisp::json::Direct`).
    let direct = match (&shape, stamped.is_empty()) {
        (Shape::Named(_), true) => {
            let fields = named_fields(&item)?;
            let key = |f: &Field| format!("{:?}", bare(&f.name.to_string()));
            let mut slots = String::new();
            let mut arms = String::new();
            let mut take = String::new();
            let mut build = String::new();
            for (k, f) in fields.iter().enumerate() {
                let ty = &f.ty;
                slots.push_str(&format!(
                    "let mut __f{k}: ::std::option::Option<{ty}> = ::std::option::Option::None;"
                ));
                for name in std::iter::once(key(f)).chain(f.was.clone()) {
                    arms.push_str(&format!(
                        "{name} if __f{k}.is_none() => __f{k} = ::std::option::Option::Some(<{ty} as ::wisp::FromJson>::read(__d)?),"
                    ));
                }
                let absent = match &f.default {
                    Some(d) => format!("{{ let __v: {ty} = {d}; __v }}"),
                    None => format!("<{ty} as ::wisp::FromJson>::missing()?"),
                };
                take.push_str(&format!(
                    "let __f{k} = match __f{k} {{ ::std::option::Option::Some(v) => v, ::std::option::Option::None => {absent} }};"
                ));
                for call in &f.checks {
                    take.push_str(&format!(
                        "{{ let __x = &__f{k}; if ({call}).is_some() {{ return ::std::option::Option::None; }} }}"
                    ));
                }
                build.push_str(&format!("{}: __f{k},", f.name));
            }
            // A key twice: the last counts, which `from_json` works out.
            let twice: Vec<String> = fields.iter().map(key).collect();
            if !twice.is_empty() {
                arms.push_str(&format!(
                    "{} => return ::std::option::Option::None,",
                    twice.join(" | ")
                ));
            }
            format!(
                "fn read(__d: &mut ::wisp::json::Direct) -> ::std::option::Option<Self> {{
                    {slots}
                    __d.object()?;
                    let mut __first = true;
                    while let ::std::option::Option::Some(__k) = __d.key(&mut __first)? {{
                        match __k {{ {arms} _ => __d.skip()?, }}
                    }}
                    {take}
                    ::std::option::Option::Some({name} {{ {build} }})
                }}"
            )
        }
        _ => String::new(),
    };
    let body = match &shape {
        Shape::Named(_) => {
            let mut body = parse("let __m = __p.object(__v)?;");
            let mut build = Vec::new();
            for (k, field) in named_fields(&item)?.iter().enumerate() {
                let id = field.name.to_string();
                let key = format!("{:?}", bare(&id));
                let read = match (stamped.contains(&id.as_str()), &field.default, &field.was) {
                    (true, ..) => format!("::wisp::rt::stamped(__p, __m, {key})"),
                    (false, None, None) => format!("__p.field(__m, {key})"),
                    (false, default, was) => {
                        let was = was.as_deref().unwrap_or("\"\"");
                        let default = match default {
                            Some(d) => format!("|| ::std::option::Option::Some({d})"),
                            None => "|| ::std::option::Option::None".into(),
                        };
                        format!("__p.field_or(__m, {key}, {was}, {default})")
                    }
                };
                // Placed at the field, so a type that is not `FromJson` is
                // the one the compiler points at.
                body.extend(at(
                    &format!("let __f{k}: ::std::option::Option<{}> = {read};", field.ty),
                    field.name.span(),
                ));
                let checks: String = (field.checks.iter())
                    .map(|call| format!("__p.check({key}, {call});"))
                    .collect();
                if !checks.is_empty() {
                    body.extend(at(
                        &format!(
                            "if let ::std::option::Option::Some(__x) = &__f{k} {{ {checks} }}"
                        ),
                        field.name.span(),
                    ));
                }
                build.push(format!("{id}: __f{k}?,"));
            }
            body.extend(parse(&format!(
                "::std::option::Option::Some({name} {{ {} }})",
                build.concat()
            )));
            body
        }
        Shape::Tuple(fields) if fields.len() == 1 => parse(&format!(
            "::wisp::FromJson::from_json(__v, __p).map({name})"
        )),
        Shape::Tuple(fields) => {
            let n = fields.len();
            let reads: String = (0..n)
                .map(|i| format!("let __f{i} = __p.read(\"[{i}]\", &__a[{i}]);"))
                .collect();
            let build: String = (0..n).map(|i| format!("__f{i}?,")).collect();
            parse(&format!(
                "let __a = match __v.as_array() {{
                    ::std::option::Option::Some(a) if a.len() == {n} => a,
                    _ => {{ __p.add(\"expected an array of {n} items\"); return ::std::option::Option::None; }}
                }};
                {reads}
                ::std::option::Option::Some({name}({build}))"
            ))
        }
        Shape::Unit => parse(&format!(
            "<() as ::wisp::FromJson>::from_json(__v, __p).map(|()| {name})"
        )),
        Shape::Enum(variants) => {
            let arms: String = variants
                .iter()
                .map(|v| format!("::std::option::Option::Some({:?}) => ::std::option::Option::Some({name}::{v}),", bare(v)))
                .collect();
            let names: Vec<String> = variants.iter().map(|v| bare(v)).collect();
            parse(&format!(
                "match __v.as_str() {{
                    {arms}
                    _ => {{ __p.add({:?}); ::std::option::Option::None }}
                }}",
                format!("must be one of: {}", names.join(", "))
            ))
        }
    };
    // A struct with named fields may be an action's input, read by them.
    let fields = match shape {
        Shape::Named(_) => format!("impl ::wisp::rt_traits::Fields for {name} {{}}"),
        _ => String::new(),
    };
    // `#[unique]`: the saved table of the type refuses a repeat.
    let mut unique = String::new();
    if let Shape::Named(_) = &shape {
        let marked: Vec<Field> = named_fields(&item)?
            .into_iter()
            .filter(|f| f.unique)
            .collect();
        if let Some(second) = marked.get(1) {
            return Err((
                "a type has one `#[unique]` field".into(),
                second.name.span(),
            ));
        }
        if let Some(f) = marked.first() {
            unique = format!(
                "const UNIQUE: ::std::option::Option<(&'static str, fn(&Self) -> &str)> = ::std::option::Option::Some(({:?}, |__v: &Self| ::std::convert::AsRef::<str>::as_ref(&__v.{})));",
                bare(&f.name.to_string()),
                f.name
            );
        }
    }
    // A field that holds a `Password` is sealed as the row enters a table.
    let mut seals = String::new();
    if let Shape::Named(_) = &shape {
        let all = named_fields(&item)?;
        let on = |f: &Field| format!("<{} as ::wisp::FromJson>::SEALS", f.ty);
        let any: String = all.iter().map(|f| format!("|| {}", on(f))).collect();
        let each: String = all
            .iter()
            .map(|f| {
                format!(
                    "if {} {{ ::wisp::FromJson::seal(&mut self.{}, __hash); }}",
                    on(f),
                    f.name
                )
            })
            .collect();
        seals = format!(
            "const SEALS: bool = false {any}; fn seal(&mut self, __hash: bool) {{ {each} }}"
        );
    }
    let template = parse(&format!(
        "impl ::wisp::FromJson for {name} {{
            {seals}
            fn from_json(__v: &::wisp::Value, __p: &mut ::wisp::json::Problems) -> ::std::option::Option<Self> {{
                __wisp_write
            }}
            {direct}
            {unique}
        }}
        {fields}"
    ));
    Ok(fill(template, &body, &TokenStream::new()))
}

/// Makes a struct a JSON resource: `Json`, `FromJson` (with its fields'
/// `#[validate(...)]`), and a saved `wisp::Table` of its own,
/// `Note::table()`, whose rows survive restarts. In a `+server.rs`, Wisp
/// serves it: GET and POST at the route, GET, PUT, PATCH and DELETE at its
/// `/[id]`, for each method the file does not write itself.
///
/// `#[rest(...)]`: `write = "API_KEY"` (writes need `Authorization: Bearer
/// $API_KEY`), `key = "…"` (every request does), `admin = "…"` (DELETE
/// does), `table = "notes"` (its name in the store; the type's, lowercased,
/// by default), `ids = "random"` (ids no one can count through), `memory`
/// (kept in memory only). Fields named `created_at` and `updated_at` are
/// set by Wisp: a number of seconds, or RFC 3339 text in a `String`.
#[proc_macro_derive(Rest, attributes(validate, rest, json, unique))]
pub fn derive_rest(item: TokenStream) -> TokenStream {
    match rest(item) {
        Ok(code) => code,
        Err((msg, span)) => error(&msg, span),
    }
}

/// What the app reads from its environment once, at start: each field of
/// the struct from the variable of its name in capitals (`api_key` from
/// `API_KEY`, or `.env`; `r#type` from `TYPE`), any `FromStr` type, an `Option` one may be unset.
/// `Config::get().api_key` reads it anywhere. A variable missing or not
/// parsing stops the server when it starts (the build calls `Config::load()`
/// before `init`), naming each one, never a request.
#[proc_macro_derive(Config)]
pub fn derive_config(item: TokenStream) -> TokenStream {
    match config(item) {
        Ok(code) => code,
        Err((msg, span)) => error(&msg, span),
    }
}

fn config(item: TokenStream) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item.clone(), "Config")?;
    let fields = named_fields(&item)?;
    if !matches!(shape, Shape::Named(_)) || fields.is_empty() {
        return Err((
            "#[derive(Config)] works on a struct with named fields: one variable each".into(),
            name.span(),
        ));
    }
    let (mut reads, mut some, mut list) = (String::new(), String::new(), String::new());
    for f in &fields {
        // `r#type` is read from `TYPE`.
        let (n, key) = (&f.name, f.name.to_string().replace("r#", "").to_uppercase());
        let ty: String = f.ty.chars().filter(|c| !c.is_whitespace()).collect();
        let ty = ty.strip_prefix("::").unwrap_or(&ty);
        let helper = if ["Option<", "option::Option<", "std::option::Option<"]
            .iter()
            .any(|p| ty.starts_with(p))
        {
            "config_opt"
        } else {
            "config"
        };
        reads.push_str(&format!(
            "let {n} = ::wisp::rt::{helper}({key:?}, &mut __bad); "
        ));
        some.push_str(&format!("Some({n}), "));
        list.push_str(&format!("{n}, "));
    }
    Ok(parse(&format!(
        "static __CONFIG_{name}: ::std::sync::OnceLock<{name}> = ::std::sync::OnceLock::new(); \
         #[allow(dead_code)] impl {name} {{ \
         pub fn load() -> ::wisp::Result<()> {{ \
         if __CONFIG_{name}.get().is_some() {{ return Ok(()); }} \
         let mut __bad = ::std::vec::Vec::<::std::string::String>::new(); \
         {reads}\
         let ({some}) = ({list}) else {{ return Err(::wisp::rt::config_error(&__bad)); }}; \
         let _ = __CONFIG_{name}.set({name} {{ {list} }}); \
         Ok(()) }} \
         pub fn get() -> &'static {name} {{ \
         if let Err(e) = {name}::load() {{ panic!(\"{{}}\", e.message()); }} \
         __CONFIG_{name}.get().expect(\"loaded\") }} }}"
    )))
}

/// What `#[rest(...)]` takes, for its errors.
const REST_TAKES: &str = "#[rest] takes key = \"ENV_VAR\", write = \"ENV_VAR\", admin = \"ENV_VAR\", \
                          table = \"name\", ids = \"random\" and memory";

/// Fields Wisp sets on a `#[derive(Rest)]` row: when it was made, and last
/// changed.
const STAMPED: [&str; 2] = ["created_at", "updated_at"];

fn rest(item: TokenStream) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item.clone(), "Rest")?;
    if !matches!(shape, Shape::Named(_)) {
        return Err((
            "#[derive(Rest)] works on a struct with named fields: a row is a JSON object".into(),
            name.span(),
        ));
    }
    let none = || "None".to_string();
    let (mut key, mut write, mut admin) = (none(), none(), none());
    let mut table = format!("Some({:?})", name.to_string().to_lowercase());
    let mut random = false;
    let tokens: Vec<TokenTree> = item.clone().into_iter().collect();
    for pair in tokens.windows(2) {
        let [TokenTree::Punct(hash), TokenTree::Group(attr)] = pair else {
            continue;
        };
        let inner: Vec<TokenTree> = attr.stream().into_iter().collect();
        let [TokenTree::Ident(id), TokenTree::Group(args)] = inner.as_slice() else {
            continue;
        };
        if hash.as_char() != '#' || id.to_string() != "rest" {
            continue;
        }
        for setting in items(args.stream()) {
            match setting.as_slice() {
                [TokenTree::Ident(k)] if k.to_string() == "memory" => table = none(),
                [
                    TokenTree::Ident(k),
                    TokenTree::Punct(eq),
                    TokenTree::Literal(v),
                ] if eq.as_char() == '=' && v.to_string().starts_with('"') => {
                    let value = format!("Some({v})");
                    match k.to_string().as_str() {
                        "key" => key = value,
                        "write" => write = value,
                        "admin" => admin = value,
                        "table" => table = value,
                        "ids" if v.to_string() == "\"random\"" => random = true,
                        _ => return Err((REST_TAKES.into(), k.span())),
                    }
                }
                other => {
                    return Err((
                        REST_TAKES.into(),
                        other.first().map_or(id.span(), TokenTree::span),
                    ));
                }
            }
        }
    }
    let fields = named_fields(&item)?;
    let bare = |id: &str| id.strip_prefix("r#").unwrap_or(id).to_string();
    let kinds: String = fields
        .iter()
        .map(|f| {
            format!(
                "({:?}, ::wisp::rt::rest::Kind::{}),",
                bare(&f.name.to_string()),
                kind(&f.ty)
            )
        })
        .collect();
    let arms: String = fields
        .iter()
        .map(|f| {
            format!(
                "{:?} => ::wisp::Json::json(&self.{}, out),",
                bare(&f.name.to_string()),
                f.name
            )
        })
        .collect();
    let stamps: String = fields
        .iter()
        .filter(|f| STAMPED.contains(&f.name.to_string().as_str()))
        .map(|f| {
            let (n, ty) = (&f.name, &f.ty);
            let now = format!("<{ty} as ::wisp::rt::Stamp>::now()");
            match n.to_string().as_str() {
                "created_at" => format!(
                    "self.{n} = match old {{ ::std::option::Option::Some(o) => ::std::clone::Clone::clone(&o.{n}), ::std::option::Option::None => {now} }};"
                ),
                _ => format!("self.{n} = {now};"),
            }
        })
        .collect();
    let stamp = match stamps.is_empty() {
        true => String::new(),
        false => format!("fn stamp(&mut self, old: ::std::option::Option<&Self>) {{ {stamps} }}"),
    };
    let mut out = json(item.clone())?;
    out.extend(from_json_with(item, &STAMPED)?);
    out.extend(parse(&format!(
        "impl {name} {{
            /// Its rows, which a `+server.rs` serves, kept in the app's store.
            pub fn table() -> &'static ::wisp::Table<{name}> {{
                static TABLE: ::wisp::Table<{name}> = ::wisp::Table::rest(::std::option::Option::{table}, {random});
                &TABLE
            }}
        }}
        impl ::wisp::Resource for {name} {{
            const KEY: ::std::option::Option<&'static str> = ::std::option::Option::{key};
            const WRITE: ::std::option::Option<&'static str> = ::std::option::Option::{write};
            const ADMIN: ::std::option::Option<&'static str> = ::std::option::Option::{admin};
            const FIELDS: &'static [(&'static str, ::wisp::rt::rest::Kind)] = &[{kinds}];
            fn table() -> &'static ::wisp::Table<{name}> {{
                {name}::table()
            }}
            fn field(&self, name: &str, out: &mut ::std::string::String) -> bool {{
                match name {{
                    {arms}
                    _ => return false,
                }}
                true
            }}
            {stamp}
        }}"
    )));
    Ok(out)
}

/// What JSON a field of type `ty` holds, as a `Kind` variant.
fn kind(ty: &str) -> &'static str {
    let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
    let t = t
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&t);
    let last = t.rsplit("::").next().unwrap_or(t);
    match last {
        "String" | "str" | "&str" | "&'staticstr" | "char" => "Text",
        _ if last.starts_with("Cow<") => "Text",
        "bool" => "Bool",
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" | "f32" | "f64" => "Number",
        _ => "Other",
    }
}

/// A named field, for `FromJson`: its name, its type as text, and the
/// checks of its `#[validate(...)]` rules, as calls on `__x`.
struct Field {
    name: Ident,
    ty: String,
    checks: Vec<String>,
    /// `#[json(default)]`, or `#[json(default = expr)]`: the value of a field
    /// a stored row or a body leaves out.
    default: Option<String>,
    /// `#[json(was = "old")]`: the name it had, which old rows still use.
    was: Option<String>,
    /// `#[unique]`: no two rows of a table have the same value.
    unique: bool,
}

/// The named fields of the struct `item`, with their types and rules.
fn named_fields(item: &TokenStream) -> Result<Vec<Field>, Error> {
    let Some(group) = item.clone().into_iter().find_map(|t| match t {
        TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => Some(g),
        _ => None,
    }) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for field in items(group.stream()) {
        let mut checks = Vec::new();
        let (mut default, mut was, mut unique) = (None, None, false);
        let mut rest = field.as_slice();
        while let [TokenTree::Punct(hash), TokenTree::Group(attr), after @ ..] = rest
            && hash.as_char() == '#'
        {
            let inner: Vec<TokenTree> = attr.stream().into_iter().collect();
            match inner.as_slice() {
                [TokenTree::Ident(id)] if id.to_string() == "unique" => unique = true,
                [TokenTree::Ident(id), TokenTree::Group(args)] if id.to_string() == "json" => {
                    for rule in items(args.stream()) {
                        let value = match &rule[1..] {
                            [TokenTree::Punct(eq), v @ ..]
                                if eq.as_char() == '=' && !v.is_empty() =>
                            {
                                Some(TokenStream::from_iter(v.iter().cloned()).to_string())
                            }
                            _ => None,
                        };
                        match (&rule[0], value) {
                            (TokenTree::Ident(n), v) if n.to_string() == "default" => {
                                default = Some(v.unwrap_or_else(|| {
                                    "::std::default::Default::default()".into()
                                }));
                            }
                            (TokenTree::Ident(n), Some(v))
                                if n.to_string() == "was" && v.starts_with('"') =>
                            {
                                was = Some(v)
                            }
                            (t, _) => {
                                return Err((
                                    "expected `default`, `default = value` or `was = \"old name\"`"
                                        .into(),
                                    t.span(),
                                ));
                            }
                        }
                    }
                }
                _ => {}
            }
            if let [TokenTree::Ident(id), TokenTree::Group(args)] = inner.as_slice()
                && id.to_string() == "validate"
            {
                for rule in items(args.stream()) {
                    let TokenTree::Ident(name) = &rule[0] else {
                        return Err((
                            "expected a rule, such as `min_len = 1`".into(),
                            rule[0].span(),
                        ));
                    };
                    let value = match &rule[1..] {
                        [] => None,
                        [TokenTree::Punct(eq), v @ ..] if eq.as_char() == '=' && !v.is_empty() => {
                            Some(TokenStream::from_iter(v.iter().cloned()).to_string())
                        }
                        other => return Err(("expected `= value`".into(), other[0].span())),
                    };
                    let r = wisp_shared::rules::rule(&name.to_string(), value.as_deref())
                        .map_err(|e| (e, name.span()))?;
                    checks.push(r.check("__x"));
                }
            }
            rest = after;
        }
        let rest = skip_visibility(rest);
        let [TokenTree::Ident(name), TokenTree::Punct(colon), ty @ ..] = rest else {
            return Err(("expected `name: Type`".into(), field[0].span()));
        };
        if colon.as_char() != ':' || ty.is_empty() {
            return Err(("expected `name: Type`".into(), colon.span()));
        }
        out.push(Field {
            name: name.clone(),
            ty: TokenStream::from_iter(ty.iter().cloned()).to_string(),
            checks,
            default,
            was,
            unique,
        });
    }
    Ok(out)
}

/// The impls for a type without fields, whose one value is `value`.
fn empty(name: &Ident, value: &str) -> TokenStream {
    parse(&format!(
        "impl ::std::fmt::Display for {name} {{
            fn fmt(&self, _: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {{
                ::std::result::Result::Ok(())
            }}
        }}
        impl ::std::str::FromStr for {name} {{
            type Err = ::wisp::rt::BadCookie;
            fn from_str(s: &str) -> ::std::result::Result<Self, ::wisp::rt::BadCookie> {{
                if s.is_empty() {{ ::std::result::Result::Ok({value}) }} else {{ ::std::result::Result::Err(::wisp::rt::BadCookie) }}
            }}
        }}"
    ))
}

fn parse(code: &str) -> TokenStream {
    code.parse().expect("generated code parses")
}

/// `code` with every token placed at `field`, the field it reads or writes,
/// so a field whose type is not `Display` or `FromStr` is the one the
/// compiler points at. Names in it still resolve as in the rest of the
/// generated code, where `w` and `r` are.
fn at(code: &str, field: Span) -> TokenStream {
    respan(parse(code), Span::call_site().located_at(field))
}

fn respan(code: TokenStream, span: Span) -> TokenStream {
    code.into_iter()
        .map(|t| match t {
            TokenTree::Group(g) => {
                let mut g = Group::new(g.delimiter(), respan(g.stream(), span));
                g.set_span(span);
                g.into()
            }
            mut t => {
                t.set_span(span);
                t
            }
        })
        .collect()
}

/// `template` with the `__wisp_write` and `__wisp_read` placeholders
/// replaced.
fn fill(template: TokenStream, write: &TokenStream, read: &TokenStream) -> TokenStream {
    let mut out = TokenStream::new();
    for t in template {
        match t {
            TokenTree::Ident(id) if id.to_string() == "__wisp_write" => out.extend(write.clone()),
            TokenTree::Ident(id) if id.to_string() == "__wisp_read" => out.extend(read.clone()),
            TokenTree::Group(g) => {
                let mut filled = Group::new(g.delimiter(), fill(g.stream(), write, read));
                filled.set_span(g.span());
                out.extend([TokenTree::from(filled)]);
            }
            t => out.extend([t]),
        }
    }
    out
}

/// `item` without the `#[...]` attributes in front of it (doc comments are
/// attributes too).
fn skip_attributes(mut item: &[TokenTree]) -> &[TokenTree] {
    while let [TokenTree::Punct(hash), TokenTree::Group(g), rest @ ..] = item
        && hash.as_char() == '#'
        && g.delimiter() == Delimiter::Bracket
    {
        item = rest;
    }
    item
}

/// `item` without `pub` or `pub(...)` in front of it.
fn skip_visibility(item: &[TokenTree]) -> &[TokenTree] {
    match item {
        [TokenTree::Ident(p), TokenTree::Group(g), rest @ ..]
            if p.to_string() == "pub" && g.delimiter() == Delimiter::Parenthesis =>
        {
            rest
        }
        [TokenTree::Ident(p), rest @ ..] if p.to_string() == "pub" => rest,
        _ => item,
    }
}

/// Each token's index with the `<…>` depth before it, for finding what is at
/// the top level of a type or an expression: groups are already one token, but
/// the commas of `Map<K, V>` are not. The `>` of `->` closes nothing. With
/// `expr`, a `<` opens only after `:` (a turbofish, `f::<A, B>()`), else it is
/// less-than, and a stray `>` is greater-than.
fn angles(tokens: &[TokenTree], expr: bool) -> impl Iterator<Item = (usize, i32)> + '_ {
    // The last token was the `-` of `->`, or a `:`.
    let (mut depth, mut dash, mut colon) = (0i32, false, false);
    tokens.iter().enumerate().map(move |(i, t)| {
        let before = depth;
        let (mut next_dash, mut next_colon) = (false, false);
        if let TokenTree::Punct(p) = t {
            match p.as_char() {
                '<' if !expr || colon => depth += 1,
                '>' if !dash && (!expr || depth > 0) => depth -= 1,
                _ => {}
            }
            next_dash = p.as_char() == '-' && p.spacing() == Spacing::Joint;
            next_colon = p.as_char() == ':';
        }
        (dash, colon) = (next_dash, next_colon);
        (i, before)
    })
}

/// Splits a field or variant list at its top-level commas.
fn items(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = vec![Vec::new()];
    for (i, depth) in angles(&tokens, false) {
        match &tokens[i] {
            TokenTree::Punct(p) if p.as_char() == ',' && depth <= 0 => out.push(Vec::new()),
            t => out.last_mut().expect("never empty").push(t.clone()),
        }
    }
    out.retain(|item| !item.is_empty());
    out
}
