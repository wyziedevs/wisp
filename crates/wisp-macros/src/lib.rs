//! `#[action]` marks a `+page.rs` function as a form action,
//! `#[derive(Cookie)]` lets a type be kept in a cookie, `#[derive(Json)]`
//! lets browser code and API clients read it, and `#[derive(FromJson)]` reads
//! it from a JSON request body.
//!
//! No `syn`, no `quote`: this crate compiles instantly. `#[action]` returns
//! its input as it is (`wisp-build` finds it in the source and generates
//! the route), but for adding `cx` to one that uses it without taking it;
//! the derives read just enough of a type to know its name and its fields'
//! names.

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

/// An action whose body uses `cx` but does not take it gets it: `cx: &mut
/// Cx` as its first parameter (`wisp-build` sees the same and passes it).
fn implicit_cx(item: TokenStream) -> TokenStream {
    let mut tokens: Vec<TokenTree> = item.into_iter().collect();
    let Some(f) = tokens
        .iter()
        .position(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "fn"))
    else {
        return tokens.into_iter().collect();
    };
    let params = tokens[f..]
        .iter()
        .position(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis));
    let body = tokens
        .iter()
        .rposition(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace));
    let (Some(p), Some(b)) = (params.map(|p| p + f), body) else {
        return tokens.into_iter().collect();
    };
    let (TokenTree::Group(pg), TokenTree::Group(bg)) = (&tokens[p], &tokens[b]) else {
        return tokens.into_iter().collect();
    };
    if names(&pg.stream(), &["cx", "Cx"]) || !names(&bg.stream(), &["cx"]) {
        return tokens.into_iter().collect();
    }
    let span = pg.span();
    let mut stream: Vec<TokenTree> = "#[allow(unused_variables)] cx: &mut ::wisp::Cx"
        .parse::<TokenStream>()
        .expect("valid tokens")
        .into_iter()
        .collect();
    let rest: Vec<TokenTree> = pg.stream().into_iter().collect();
    if !rest.is_empty() {
        stream.push(Punct::new(',', Spacing::Alone).into());
        stream.extend(rest);
    }
    let mut group = Group::new(Delimiter::Parenthesis, stream.into_iter().collect());
    group.set_span(span);
    tokens[p] = group.into();
    tokens.into_iter().collect()
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
/// fields (an `Option` field may be left out or `null`; other fields are
/// ignored), a tuple struct with one field as that field, and an enum
/// without fields from its variant's name.
///
/// `#[validate(...)]` on a field checks it once read: `min = 0`, `max = 10`
/// (numbers), `min_len = 1`, `max_len = 200` (characters of a string, items
/// of a list), `email`. Every problem is reported, by field, in one 422.
#[proc_macro_derive(FromJson, attributes(validate))]
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
    Ok(fill(template, &body, &TokenStream::new()))
}

fn from_json(item: TokenStream) -> Result<TokenStream, Error> {
    let (name, shape) = read_type(item.clone(), "FromJson")?;
    let bare = |id: &str| id.strip_prefix("r#").unwrap_or(id).to_string();
    let body = match &shape {
        Shape::Named(_) => {
            let mut body = parse("let __m = __p.object(__v)?;");
            let mut build = Vec::new();
            for (k, field) in named_fields(&item)?.iter().enumerate() {
                let id = field.name.to_string();
                let key = format!("{:?}", bare(&id));
                // Placed at the field, so a type that is not `FromJson` is
                // the one the compiler points at.
                body.extend(at(
                    &format!(
                        "let __f{k}: ::std::option::Option<{}> = __p.field(__m, {key});",
                        field.ty
                    ),
                    field.name.span(),
                ));
                let mut checks = String::new();
                for (rule, value, span) in &field.rules {
                    let call = match (rule.as_str(), value) {
                        ("min" | "max", Some(v)) => format!("{rule}(__x, ({v}) as f64)"),
                        ("min_len" | "max_len", Some(v)) => format!("{rule}(__x, {v})"),
                        ("email", None) => "email(__x)".into(),
                        ("email", Some(_)) => return Err(("`email` takes no value".into(), *span)),
                        (_, None) => {
                            return Err((format!("`{rule}` needs a value: `{rule} = 1`"), *span));
                        }
                        _ => unreachable!("rules are checked as they are read"),
                    };
                    checks.push_str(&format!("__p.check({key}, ::wisp::json::check::{call});"));
                }
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
    let template = parse(&format!(
        "impl ::wisp::FromJson for {name} {{
            fn from_json(__v: &::wisp::Value, __p: &mut ::wisp::json::Problems) -> ::std::option::Option<Self> {{
                __wisp_write
            }}
        }}"
    ));
    Ok(fill(template, &body, &TokenStream::new()))
}

/// A named field, for `FromJson`: its name, its type as text, and its
/// `#[validate(...)]` rules (name, value as text, where it is).
struct Field {
    name: Ident,
    ty: String,
    rules: Vec<(String, Option<String>, Span)>,
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
        let mut rules = Vec::new();
        let mut rest = field.as_slice();
        while let [TokenTree::Punct(hash), TokenTree::Group(attr), after @ ..] = rest
            && hash.as_char() == '#'
        {
            let inner: Vec<TokenTree> = attr.stream().into_iter().collect();
            if let [TokenTree::Ident(id), TokenTree::Group(args)] = inner.as_slice()
                && id.to_string() == "validate"
            {
                for rule in items(args.stream()) {
                    let TokenTree::Ident(rule_name) = &rule[0] else {
                        return Err((
                            "expected a rule, such as `min_len = 1`".into(),
                            rule[0].span(),
                        ));
                    };
                    let known = ["min", "max", "min_len", "max_len", "email"];
                    let rule_text = rule_name.to_string();
                    if !known.contains(&rule_text.as_str()) {
                        return Err((
                            format!(
                                "#[validate] has no `{rule_text}`: it takes min, max, min_len, max_len and email"
                            ),
                            rule_name.span(),
                        ));
                    }
                    let value = match &rule[1..] {
                        [] => None,
                        [TokenTree::Punct(eq), v @ ..] if eq.as_char() == '=' && !v.is_empty() => {
                            Some(TokenStream::from_iter(v.iter().cloned()).to_string())
                        }
                        other => return Err(("expected `= value`".into(), other[0].span())),
                    };
                    rules.push((rule_text, value, rule_name.span()));
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
            rules,
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

/// Splits a field or variant list at its top-level commas. Commas inside
/// groups are already hidden; those inside `<...>` of a type are not.
fn items(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0i32;
    let mut arrow = false; // the `-` of `->`, whose `>` is not a closing bracket
    for t in stream {
        if let TokenTree::Punct(p) = &t {
            match p.as_char() {
                '<' => depth += 1,
                '>' if !arrow => depth -= 1,
                ',' if depth == 0 => {
                    out.push(Vec::new());
                    continue;
                }
                _ => {}
            }
            arrow = p.as_char() == '-' && p.spacing() == Spacing::Joint;
        } else {
            arrow = false;
        }
        out.last_mut().expect("never empty").push(t);
    }
    out.retain(|item| !item.is_empty());
    out
}
