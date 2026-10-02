//! Browser code: the values a page sends to its client scripts, as JSON,
//! and the record of which template instances a response rendered.
//!
//! A template with a client script or directives is compiled into an ES
//! module (`/_app/c/ID.js`). Each time it renders, it is an *instance*: its
//! elements are marked with it and their binding group (`GROUP_ATTR`, and
//! `LOOP_ATTR` in a Rust loop), and the page ends with the list of
//! instances, the server values each one's code reads, and the runtime
//! (`wisp_shared::LIVE_JS`) that starts them. See `page` in `http.rs`, and
//! `protocol.rs` for the format.

use crate::Out;
use crate::protocol::{ISLAND_NONE, LIVE_CLOSE, LIVE_OPEN, LIVE_PARAMS, LIVE_RECORDS, LIVE_ROUTE};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::rc::Rc;
use std::sync::Arc;

/// A value a template's browser code can read: `data.user.name` in a client
/// script or directive sends `data.user.name` to the browser as JSON.
///
/// Implemented for strings, numbers, `bool`, `char`, `Option`, sequences,
/// tuples and maps with string keys. `#[derive(Json)]` implements it for
/// your own structs (an object of their fields) and fieldless enums (the
/// variant's name).
///
/// The output is safe inside `<script type="application/json">` and, once
/// HTML-escaped, inside an attribute: `<`, `>` and `&` in strings are
/// written as `<`, `>` and `&`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be sent to browser code: it does not implement `wisp::Json`",
    label = "read by a client script or directive",
    note = "add `#[derive(Json)]` to the type (it is in `wisp::prelude`), or read a field that has a simpler type"
)]
pub trait Json {
    fn json(&self, out: &mut String);
}

/// A template's browser module, compiled into the binary.
pub struct ClientModule {
    /// Short and stable, such as `t3`.
    pub id: &'static str,
    /// `/_app/c/t3.js`, where it is served.
    pub path: &'static str,
    /// `/_app/c/t3.js?v=HASH`, the address pages use.
    pub url: &'static str,
    pub etag: &'static str,
    pub source: &'static str,
    /// A module it imports that the page preloads with it (the runtime's
    /// less used half, `/_app/c/extra.js?v=…`), or "".
    pub preload: &'static str,
}

/// The instances a response rendered, kept in its `Out`.
#[derive(Default)]
pub(crate) struct Live {
    count: u32,
    /// The modules instances use, each with whether one of them starts
    /// with the page (rather than as an island, later).
    modules: Vec<(&'static ClientModule, bool)>,
    /// `[0,"t3",-1,{…}],[1,"t5",0,{…},"v"]`: id, module, the instance it
    /// renders inside (for context), its server values, and how an island
    /// starts. The last record is left open: its blob is the caller's to
    /// write after `live`, and its `how` comes after the blob, so it is
    /// closed, with `last_how` if any, by the next `live` or by `tail`.
    instances: String,
    last_how: Option<&'static str>,
    /// The instances rendering now, innermost last, and when each starts.
    open: Vec<(u32, Start)>,
    /// `,"r":"/post/[slug]","p":{"slug":"x"}` for a page with a `+page.js`.
    route: String,
    /// The `client:…` of the component about to render (see `live_how`).
    next: Option<&'static str>,
    /// Where a `client:none` instance writes its values: thrown away.
    scratch: String,
}

/// When an instance's browser code starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    /// With the page.
    Now,
    /// When the browser sees fit: it, or an instance it renders inside, is
    /// an island (`client:visible`, `client:idle`, …).
    Later,
    /// Never: `client:none`, or inside one. Its code is not sent.
    Never,
}

impl Live {
    pub(crate) fn clear(&mut self) {
        self.count = 0;
        self.modules.clear();
        self.instances.clear();
        self.last_how = None;
        self.open.clear();
        self.route.clear();
        self.next = None;
    }

    /// What goes at the end of the page's body when it has instances: the
    /// instances and the modules they need, the modules that start with the
    /// page preloaded, and the runtime that starts them, if any does (an
    /// island's wake-up is in wisp.js, which loads the runtime itself).
    /// Written onto `s`; nothing otherwise.
    pub(crate) fn tail(&self, s: &mut String) {
        if self.instances.is_empty() {
            return;
        }
        s.reserve(self.instances.len() + 200 * self.modules.len() + 200);
        s.push_str(LIVE_OPEN);
        for (k, (m, _)) in self.modules.iter().enumerate() {
            let comma = if k > 0 { "," } else { "" };
            let _ = write!(s, "{comma}\"{}\":\"{}\"", m.id, m.url);
        }
        s.push_str(LIVE_RECORDS);
        s.push_str(&self.instances);
        close(s, self.last_how);
        s.push(']');
        s.push_str(&self.route);
        s.push_str(LIVE_CLOSE);
        let mut extra = "";
        for (m, _) in self.modules.iter().filter(|m| m.1) {
            let _ = write!(s, "<link rel=\"modulepreload\" href=\"{}\">", m.url);
            if extra.is_empty() {
                extra = m.preload;
            }
        }
        if !extra.is_empty() {
            let _ = write!(s, "<link rel=\"modulepreload\" href=\"{extra}\">");
        }
        if self.modules.iter().any(|m| m.1) {
            s.push_str(concat!(
                "<script type=\"module\" src=\"/_app/live.js?v=",
                env!("WISP_RUNTIME_V"),
                "\"></script>"
            ));
        }
    }
}

/// Starts an instance of `module`: records it and returns its id, with the
/// buffer its blob (a JSON object of the server values it reads) goes into.
/// The caller writes the blob; the record is closed after it, with its
/// `how`, when the next one starts or the list is written.
pub fn live<'a>(out: &'a mut Out, module: &'static ClientModule) -> (u32, &'a mut String) {
    let l = &mut out.live;
    let how = l.next.take();
    let parent = l.open.last().copied();
    let start = match (how, parent.map(|p| p.1)) {
        (Some(ISLAND_NONE), _) | (_, Some(Start::Never)) => Start::Never,
        (Some(_), _) | (_, Some(Start::Later)) => Start::Later,
        _ => Start::Now,
    };
    let i = l.count;
    l.count += 1;
    l.open.push((i, start));
    if start == Start::Never {
        l.scratch.clear();
        return (i, &mut l.scratch);
    }
    match l.modules.iter_mut().find(|m| m.0.id == module.id) {
        Some(m) => m.1 |= start == Start::Now,
        None => l.modules.push((module, start == Start::Now)),
    }
    if !l.instances.is_empty() {
        close(&mut l.instances, l.last_how);
        l.instances.push(',');
    }
    l.last_how = how;
    let parent = parent.map_or(-1, |p| p.0 as i64);
    let _ = write!(l.instances, "[{i},\"{}\",{parent},", module.id);
    (i, &mut l.instances)
}

/// Closes the open record: its `how`, then `]`.
fn close(s: &mut String, how: Option<&'static str>) {
    if let Some(how) = how {
        s.push(',');
        string(s, how);
    }
    s.push(']');
}

/// The component about to render is an island: its instance starts as
/// `how` says, rather than with the page. `v` when it is seen, `i` when
/// the browser is idle, `x` at the first pointer, focus or key on it,
/// `m(query)` when the media query matches, `n` never (`client:none`: its
/// code and values are not sent). What renders inside it waits with it.
pub fn live_how(out: &mut Out, how: &'static str) {
    out.live.next = Some(how);
}

/// The instance `live` started last has rendered everything inside it.
pub fn live_end(out: &mut Out) {
    out.live.open.pop();
}

/// The route of a page with a `+page.js`, which its `load` gets: the id (its
/// pattern, such as `/post/[slug]`) and the parameters.
pub fn live_route(out: &mut Out, id: &str, params: &[(&str, &str)]) {
    let r = &mut out.live.route;
    r.clear();
    r.push_str(LIVE_ROUTE);
    string(r, id);
    r.push_str(LIVE_PARAMS);
    for (k, (name, value)) in params.iter().enumerate() {
        if k > 0 {
            r.push(',');
        }
        string(r, name);
        r.push(':');
        string(r, value);
    }
    r.push('}');
}

/// `value` as JSON. Generated code calls this, so a server value that is
/// not `Json` is reported at its use in the template.
#[inline]
pub fn json<T: Json + ?Sized>(out: &mut String, value: &T) {
    value.json(out);
}

/// `value` as browser code would show it in `{:value}`, HTML-escaped: a
/// string as itself, `null` as nothing, anything else as its JSON. The
/// server's first paint of a hole whose value it knows.
pub fn js_text<T: Json + ?Sized>(out: &mut String, value: &T) {
    Js(&crate::to_json(value)).text(out);
}

/// `{label}` and `title={label}` in Rust, for a prop only a component's
/// `$props()` names (any browser value): shown as `{:label}` would be.
impl crate::html::Direct for crate::html::Text<'_, dyn Json + '_> {
    fn put(&self, out: &mut String) {
        js_text(out, self.0);
    }
}

/// A JSON value as browser code reads it: the server's first paint of a
/// client block or component whose values it knows walks one of these.
/// It reads what `Json` writes (no whitespace), not JSON in general.
#[derive(Clone, Copy, Debug)]
pub struct Js<'a>(pub &'a str);

impl<'a> Js<'a> {
    /// `data.user`: a member of an object; `null` for anything else, as
    /// `undefined` shows and tests the same.
    pub fn get(self, key: &str) -> Js<'a> {
        self.entries()
            .find(|(k, _)| *k == key)
            .map_or(Js("null"), |(_, v)| v)
    }

    /// The keys and values of an object (nothing for anything else). What
    /// is not JSON (a hand-written `Json` impl's mistake) ends it early,
    /// never in a panic or a loop.
    pub fn entries(self) -> impl Iterator<Item = (Cow<'a, str>, Js<'a>)> {
        let (s, b) = (self.0, self.0.as_bytes());
        let mut i = if b.first() == Some(&b'{') { 1 } else { b.len() };
        std::iter::from_fn(move || {
            if i >= b.len() || b[i] != b'"' {
                return None;
            }
            let k = value_end(b, i); // at the `:`
            if b.get(k) != Some(&b':') {
                i = b.len();
                return None;
            }
            let e = value_end(b, k + 1);
            let item = (unescape(&s[i..k]), Js(&s[k + 1..e]));
            i = e + 1; // after the `,`
            Some(item)
        })
    }

    /// As `String(value)` has it: a string's text, anything else its JSON.
    pub fn raw(self, out: &mut String) {
        match self.0.as_bytes().first() {
            Some(b'"') => out.push_str(&unescape(self.0)),
            _ => out.push_str(self.0),
        }
    }

    /// The items of an array (nothing for anything else).
    pub fn items(self) -> impl Iterator<Item = Js<'a>> {
        let (s, b) = (self.0, self.0.as_bytes());
        let mut i = if b.first() == Some(&b'[') { 1 } else { b.len() };
        std::iter::from_fn(move || {
            if i >= b.len() || b[i] == b']' {
                return None;
            }
            let e = value_end(b, i);
            let item = Js(&s[i..e]);
            // On past a `,`; at a `]`, or anything else, the end.
            i = if b.get(e) == Some(&b',') {
                e + 1
            } else {
                b.len()
            };
            Some(item)
        })
    }

    /// `.length`: of an array, or of a string (in UTF-16 units, as
    /// JavaScript counts); `None` (undefined) otherwise.
    pub fn length(self) -> Option<usize> {
        match self.0.as_bytes().first() {
            Some(b'[') => Some(self.items().count()),
            Some(b'"') => Some(unescape(self.0).encode_utf16().count()),
            _ => None,
        }
    }

    /// As `if (value)` tests it: all but `null`, `false`, `""` and zero.
    pub fn truthy(self) -> bool {
        match self.0.as_bytes().first() {
            None => false,
            Some(b'-' | b'0'..=b'9') => !self.0.parse::<f64>().is_ok_and(|n| n == 0.0),
            _ => !matches!(self.0, "null" | "false" | "\"\""),
        }
    }

    /// As `{:value}` shows it, HTML-escaped: a string as itself, `null` as
    /// nothing, anything else as its JSON.
    pub fn text(self, out: &mut String) {
        match self.0.as_bytes().first() {
            Some(b'"') => crate::contexts::escape(out, &unescape(self.0)),
            _ if self.0 != "null" => crate::contexts::escape(out, self.0),
            _ => {}
        }
    }
}

/// A `name={:value}` attribute's first paint, as the browser sets it:
/// ` name="value"`, ` name` for `true`, nothing for `null` and `false` (but
/// `aria-*` writes them). `class` also takes arrays and objects of names
/// (`[a, { on: b }]`), `style` an object of properties.
pub fn js_attr(out: &mut String, name: &str, v: Js<'_>) {
    let aria = name.starts_with("aria-");
    match v.0 {
        "null" => return,
        "false" | "true" if !aria => {
            if v.0 == "true" {
                out.push(' ');
                out.push_str(name);
            }
            return;
        }
        _ => {}
    }
    let mut s = String::new();
    let open = v.0.as_bytes().first().copied();
    if name == "class" && matches!(open, Some(b'[' | b'{')) {
        class_names(v, &mut s);
    } else if name == "style" && open == Some(b'{') {
        for (k, x) in v.entries() {
            if matches!(x.0, "null" | "false") {
                continue;
            }
            if !s.is_empty() {
                s.push(';');
            }
            css_property(&mut s, &k);
            s.push(':');
            x.raw(&mut s);
        }
    } else {
        v.raw(&mut s);
    }
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    let start = out.len();
    crate::contexts::escape(out, &s);
    crate::contexts::guard_attr(out, name, start);
    out.push('"');
}

/// A `style` object's key as a CSS property: `fontSize` is `font-size`;
/// a custom property (`--x`) stays as it is.
fn css_property(out: &mut String, key: &str) {
    for c in key.chars() {
        if c.is_ascii_uppercase() && !key.starts_with("--") {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
}

/// `{:...attrs}`'s first paint: each key an attribute (see `js_attr`), but
/// the `on…` ones, which are the browser's listeners. Pairs of a name and a
/// value too.
pub fn js_attrs(out: &mut String, v: Js<'_>) {
    // A component's `...rest` comes as `[name, value]` pairs.
    let pairs = v.items().filter_map(|p| {
        let mut i = p.items();
        let (k, x) = (i.next()?, i.next()?);
        let mut name = String::new();
        k.raw(&mut name);
        Some((Cow::Owned(name), x))
    });
    for (k, x) in v.entries().chain(pairs) {
        let ok = !k.starts_with("on")
            && !k.is_empty()
            && k.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'.'));
        if ok {
            js_attr(out, &k, x);
        }
    }
}

/// `class={:[a, { on: b }]}`'s names: strings, and keys whose value holds,
/// at any depth.
fn class_names(v: Js<'_>, out: &mut String) {
    let mut add = |s: &str| {
        if !s.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(s);
        }
    };
    match v.0.as_bytes().first() {
        Some(b'[') => {
            for item in v.items() {
                let mut s = String::new();
                class_names(item, &mut s);
                add(&s);
            }
        }
        Some(b'{') => {
            for (k, x) in v.entries() {
                if x.truthy() {
                    add(&k);
                }
            }
        }
        _ if v.truthy() && v.0 != "true" => {
            let mut s = String::new();
            v.raw(&mut s);
            add(&s);
        }
        _ => {}
    }
}

/// `<wisp:element this="…">`'s name as the server paints it: the value, when
/// it is a string that is a tag's name, else `wisp-element` (the browser
/// sets the tag when it starts).
pub fn tag_name(out: &mut String, v: Js<'_>) {
    let s =
        v.0.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or("");
    let ok = s.bytes().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-');
    out.push_str(if ok { s } else { "wisp-element" });
}

/// Written as the JSON it holds: a component's browser-value prop defaults
/// to one (`$props()` without `{@props}`).
impl Json for Js<'_> {
    fn json(&self, out: &mut String) {
        out.push_str(self.0);
    }
}

/// Where the JSON value starting at `i` ends: at the `,` `:` `]` or `}`
/// after it, or the end.
fn value_end(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0u32;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' if depth == 0 => return i,
            b']' | b'}' => depth -= 1,
            b',' | b':' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// A JSON string's text, its escapes undone: itself when it has none.
fn unescape(json: &str) -> Cow<'_, str> {
    let inner = json
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(json);
    if !inner.contains('\\') {
        return Cow::Borrowed(inner);
    }
    let mut s = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    let mut high = None; // the first half of a surrogate pair
    while let Some(c) = chars.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => s.push('\n'),
            Some('r') => s.push('\r'),
            Some('t') => s.push('\t'),
            Some('b') => s.push('\u{8}'),
            Some('f') => s.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let n = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
                match (high.take(), n) {
                    (None, 0xd800..=0xdbff) => high = Some(n),
                    (Some(h), 0xdc00..=0xdfff) => s.push(
                        char::from_u32(0x10000 + ((h - 0xd800) << 10) + (n - 0xdc00))
                            .unwrap_or('\u{fffd}'),
                    ),
                    (_, n) => s.push(char::from_u32(n).unwrap_or('\u{fffd}')),
                }
            }
            Some(c) => s.push(c),
            None => {}
        }
    }
    Cow::Owned(s)
}

/// The `?v=` of `/_app/wisp.js` and `/_app/live.js`: the version and a hash
/// of both, from `wisp_build::runtime_version`.
pub const RUNTIME_VERSION: &str = env!("WISP_RUNTIME_V");

/// Whether `build`, the runtime version wisp-build generated an app's code
/// for, is this crate's: modules import the runtime by it.
pub const fn same_version(build: &str) -> bool {
    let (a, b) = (build.as_bytes(), RUNTIME_VERSION.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// A JSON string, safe in a `<script>` and (HTML-escaped) in an attribute.
/// As in `contexts::escape`, text with nothing to escape (the usual case) is
/// not looked at a byte at a time: 16 bytes at once in a loop the compiler
/// makes a few vector compares, then 8 in a word (SWAR), then the last few.
/// 2.5x (short) to 7x (long) the speed of a loop over the characters.
fn string(out: &mut String, s: &str) {
    use crate::swar::{below, eq, word};
    out.push('"');
    let b = s.as_bytes();
    let mut done = 0; // `s[..done]` is in `out` already
    let mut i = 0;
    for chunk in b.as_chunks::<16>().0 {
        let mut hit = 0u8;
        for &c in chunk {
            hit |= script_special(c) as u8;
        }
        if hit != 0 {
            script_escape(out, s, &mut done, i, i + 16);
        }
        i += 16;
    }
    if i + 8 <= b.len() {
        let x = word(b, i);
        let hit = below(x, 0x20)
            | eq(x | 0x0404_0404_0404_0404, b'&')
            | eq(x | 0x0202_0202_0202_0202, b'>')
            | eq(x, b'\\')
            | eq(x, 0xe2);
        if hit != 0 {
            script_escape(out, s, &mut done, i, i + 8);
        }
        i += 8;
    }
    script_escape(out, s, &mut done, i, b.len());
    out.push_str(&s[done..]);
    out.push('"');
}

/// A byte that may need escaping in a JSON string in a `<script>`: a
/// control, `"`, `\`, `<`, `>`, `&`, or the first of U+2028 or U+2029
/// (which end a line in older JavaScript). `"` and `&` differ only in bit
/// 2, `<` and `>` only in bit 1. (`string`'s eight-at-a-time test is the
/// same set.)
#[inline(always)]
fn script_special(c: u8) -> bool {
    (c < 0x20) | ((c | 4) == b'&') | ((c | 2) == b'>') | (c == b'\\') | (c == 0xe2)
}

/// Escapes `s[from..to]` for a JSON string in a `<script>` (which, then
/// HTML-escaped, is safe in an attribute too), appending what precedes
/// each escaped byte first. `<`, `>` and `&` are `<`..., so no
/// `</script>` or `<!--` can be in it.
#[inline(always)]
fn script_escape(out: &mut String, s: &str, done: &mut usize, from: usize, to: usize) {
    let b = s.as_bytes();
    for i in from..to {
        let (esc, len) = match b[i] {
            b'"' => ("\\\"", 1),
            b'\\' => ("\\\\", 1),
            b'\n' => ("\\n", 1),
            b'\r' => ("\\r", 1),
            b'\t' => ("\\t", 1),
            b'<' => ("\\u003c", 1),
            b'>' => ("\\u003e", 1),
            b'&' => ("\\u0026", 1),
            0xe2 if b.get(i + 1) == Some(&0x80) => match b.get(i + 2) {
                Some(0xa8) => ("\\u2028", 3),
                Some(0xa9) => ("\\u2029", 3),
                _ => continue,
            },
            c @ 0..0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push_str(&s[*done..i]);
                out.push_str("\\u00");
                out.push(HEX[usize::from(c >> 4)] as char);
                out.push(HEX[usize::from(c & 15)] as char);
                *done = i + 1;
                continue;
            }
            _ => continue,
        };
        // Escaped bytes are ASCII or start a character, so `i` is a char
        // boundary. The bytes of U+2028 after the first are never special.
        out.push_str(&s[*done..i]);
        out.push_str(esc);
        *done = i + len;
    }
}

impl Json for str {
    fn json(&self, out: &mut String) {
        string(out, self);
    }
}

impl Json for String {
    fn json(&self, out: &mut String) {
        string(out, self);
    }
}

impl Json for Cow<'_, str> {
    fn json(&self, out: &mut String) {
        string(out, self);
    }
}

impl Json for char {
    fn json(&self, out: &mut String) {
        string(out, self.encode_utf8(&mut [0; 4]));
    }
}

impl Json for bool {
    fn json(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" });
    }
}

impl Json for () {
    fn json(&self, out: &mut String) {
        out.push_str("null");
    }
}

macro_rules! unsigned {
    ($($t:ty)*) => {$(
        impl Json for $t {
            fn json(&self, out: &mut String) {
                crate::decimal(out, *self as u64);
            }
        }
    )*};
}

macro_rules! signed {
    ($($t:ty)*) => {$(
        impl Json for $t {
            fn json(&self, out: &mut String) {
                if *self < 0 {
                    out.push('-');
                }
                crate::decimal(out, self.unsigned_abs() as u64);
            }
        }
    )*};
}

/// Wider than a `u64`: through `Display`.
macro_rules! wide {
    ($($t:ty)*) => {$(
        impl Json for $t {
            fn json(&self, out: &mut String) {
                let _ = write!(out, "{self}");
            }
        }
    )*};
}

unsigned!(u8 u16 u32 u64 usize);
signed!(i8 i16 i32 i64 isize);
wide!(u128 i128);

macro_rules! floats {
    ($($t:ty)*) => {$(
        /// JSON has no infinities or NaN: they are `null`.
        impl Json for $t {
            fn json(&self, out: &mut String) {
                if self.is_finite() {
                    let _ = write!(out, "{self}");
                } else {
                    out.push_str("null");
                }
            }
        }
    )*};
}

floats!(f32 f64);

impl<T: Json + ?Sized> Json for &T {
    fn json(&self, out: &mut String) {
        (**self).json(out);
    }
}

impl<T: Json + ?Sized> Json for &mut T {
    fn json(&self, out: &mut String) {
        (**self).json(out);
    }
}

impl<T: Json + ?Sized> Json for Box<T> {
    fn json(&self, out: &mut String) {
        (**self).json(out);
    }
}

impl<T: Json + ?Sized> Json for Rc<T> {
    fn json(&self, out: &mut String) {
        (**self).json(out);
    }
}

impl<T: Json + ?Sized> Json for Arc<T> {
    fn json(&self, out: &mut String) {
        (**self).json(out);
    }
}

impl<T: Json> Json for Option<T> {
    fn json(&self, out: &mut String) {
        match self {
            Some(v) => v.json(out),
            None => out.push_str("null"),
        }
    }
}

impl<T: Json> Json for [T] {
    fn json(&self, out: &mut String) {
        out.push('[');
        for (k, v) in self.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            v.json(out);
        }
        out.push(']');
    }
}

impl<T: Json, const N: usize> Json for [T; N] {
    fn json(&self, out: &mut String) {
        self.as_slice().json(out);
    }
}

impl<T: Json> Json for Vec<T> {
    fn json(&self, out: &mut String) {
        self.as_slice().json(out);
    }
}

macro_rules! tuples {
    ($(($($name:ident $n:tt),+))*) => {$(
        /// A tuple is an array.
        impl<$($name: Json),+> Json for ($($name,)+) {
            fn json(&self, out: &mut String) {
                out.push('[');
                $(
                    if $n > 0 {
                        out.push(',');
                    }
                    self.$n.json(out);
                )+
                out.push(']');
            }
        }
    )*};
}

tuples! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
}

/// An object whose keys are `entries`' keys.
fn object<'a, K: AsRef<str> + 'a, V: Json + 'a>(
    out: &mut String,
    entries: impl Iterator<Item = (&'a K, &'a V)>,
) {
    out.push('{');
    for (k, (key, v)) in entries.enumerate() {
        if k > 0 {
            out.push(',');
        }
        string(out, key.as_ref());
        out.push(':');
        v.json(out);
    }
    out.push('}');
}

impl<K: AsRef<str>, V: Json> Json for BTreeMap<K, V> {
    fn json(&self, out: &mut String) {
        object(out, self.iter());
    }
}

impl<K: AsRef<str>, V: Json, S: BuildHasher> Json for HashMap<K, V, S> {
    fn json(&self, out: &mut String) {
        object(out, self.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::to_json;

    fn tail_of(out: &Out) -> String {
        let mut s = String::new();
        out.live.tail(&mut s);
        s
    }

    #[test]
    fn values() {
        assert_eq!(to_json(&(1u8, -2i64, 1.5f64, true)), "[1,-2,1.5,true]");
        assert_eq!(to_json(&[f64::NAN, f64::INFINITY, -0.0]), "[null,null,-0]");
        assert_eq!(to_json(&vec![Some('x'), None]), "[\"x\",null]");
        assert_eq!(to_json(&()), "null");
        assert_eq!(to_json(&(String::from("a"),)), "[\"a\"]");
        let map: BTreeMap<&str, u8> = [("b", 2), ("a", 1)].into();
        assert_eq!(to_json(&map), "{\"a\":1,\"b\":2}");
        assert_eq!(to_json(&Box::new(&&7u32)), "7");
        assert_eq!(
            to_json(&u128::MAX),
            "340282366920938463463374607431768211455"
        );
        assert_eq!(
            to_json(&(i64::MIN, i64::MAX, u64::MAX, (i8::MIN, 0u8, -1i32))),
            "[-9223372036854775808,9223372036854775807,18446744073709551615,[-128,0,-1]]"
        );
        assert_eq!(to_json(&i128::MIN), i128::MIN.to_string());
    }

    /// Every ASCII character and some longer ones (the U+2028 family among
    /// them), at every place of strings 0 to 40 bytes long (16-byte chunks,
    /// the 8-byte word, the tail), alone and with a `"` at the end, against
    /// a character at a time.
    #[test]
    fn strings_escape_as_a_char_loop_would() {
        let slow = |s: &str| -> String {
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                        let _ = write!(out, "\\u{:04x}", c as u32);
                    }
                    c if (c as u32) < 0x20 => {
                        let _ = write!(out, "\\u{:04x}", c as u32);
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        };
        let chars = (0..128u8).map(char::from).chain([
            'é', '€', '😀', '\u{2027}', '\u{2028}', '\u{2029}', '\u{202a}', '\u{e280}',
        ]);
        for c in chars {
            for len in 0..=40 {
                for at in 0..len {
                    let mut s: String = (0..len).map(|k| if k == at { c } else { 'a' }).collect();
                    for _ in 0..2 {
                        assert_eq!(to_json(s.as_str()), slow(&s), "{s:?}");
                        s.push('"');
                    }
                }
            }
        }
    }

    #[test]
    fn json_of_makes_a_response() {
        let r = crate::Response::json_of(&vec![("a", 1)]);
        assert_eq!(r.content_type, "application/json");
        assert_eq!(r.body, b"[[\"a\",1]]");
    }

    #[test]
    fn strings_are_safe_in_html() {
        let s = to_json("a\"b\\c\n\t\u{1}</script><!--&\u{2028}\u{2029}é");
        assert_eq!(
            s,
            "\"a\\\"b\\\\c\\n\\t\\u0001\\u003c/script\\u003e\\u003c!--\\u0026\\u2028\\u2029é\""
        );
        assert!(!s.contains(['<', '>', '&']));
    }

    #[test]
    fn first_paint_reads_json() {
        let j = to_json(&(vec![("a", 1u8)], "x\\\"é<", (true, 0u8, Option::<u8>::None)));
        let v = Js(&j);
        let items: Vec<&str> = v.items().map(|i| i.0).collect();
        assert_eq!(
            items,
            ["[[\"a\",1]]", "\"x\\\\\\\"é\\u003c\"", "[true,0,null]"]
        );
        let mut m = BTreeMap::new();
        m.insert("list", vec![1u8, 2]);
        m.insert("empty", vec![]);
        let o = to_json(&m);
        let o = Js(&o);
        assert_eq!(o.get("list").0, "[1,2]");
        assert_eq!(o.get("list").length(), Some(2));
        assert_eq!(o.get("nope").0, "null");
        assert!(o.get("list").truthy() && o.get("empty").truthy() && !o.get("nope").truthy());
        assert!(!Js("0").truthy() && !Js("\"\"").truthy() && Js("\"0\"").truthy());
        assert_eq!(Js("\"😀\"").length(), Some(2));
        let mut s = String::new();
        v.items().nth(1).unwrap().text(&mut s);
        Js("null").text(&mut s);
        Js("[1,2]").text(&mut s);
        assert_eq!(s, "x\\&quot;é&lt;[1,2]");
        assert_eq!(Js("5").items().count(), 0);
    }

    /// What a hand-written `Json` impl might get wrong: read without a
    /// panic or a loop (`[1:2]` once never ended, `{"a"` sliced past its end).
    #[test]
    fn first_paint_survives_what_is_not_json() {
        use crate::fuzz::{Rng, mutate};
        let walk = |j: &str| {
            let v = Js(j);
            let _ = (
                v.get("a"),
                v.length(),
                v.items().count(),
                v.entries().count(),
            );
            let mut s = String::new();
            for name in ["class", "style", "href", "title"] {
                js_attr(&mut s, name, v);
            }
            js_attrs(&mut s, v);
            v.text(&mut s);
            v.raw(&mut s);
            tag_name(&mut s, v);
        };
        for bad in [
            "{\"",
            "{\"a\"",
            "{\"a\":",
            "{\"a\"\u{e9}:1}",
            "[1:2]",
            "{\"a\"x:1}",
            "[",
            "{",
            "\"\\",
            "{\"a\":1:\"b\":2}",
            "[\"\\",
            "{\"\\u12",
            "[,]",
            "{:}",
            "\"\\ud800\"",
        ] {
            walk(bad);
        }
        assert_eq!(Js("[1:2]").items().count(), 1);
        assert_eq!(Js("{\"a\"").get("a").0, "null");
        let mut m = BTreeMap::new();
        m.insert("a", (vec!["x\"<é", "\\"], Some(1.5f64), [true, false]));
        m.insert("class", (vec!["b"], None, [false, true]));
        let good = to_json(&m);
        let mut rng = Rng::new(3);
        for _ in 0..20_000 {
            let mut b = good.clone().into_bytes();
            mutate(&mut rng, &mut b);
            if let Ok(j) = String::from_utf8(b) {
                walk(&j);
            }
        }
    }

    /// `href={:x}` and the like, painted first: a value that would run
    /// script is blocked, as `href={x}` blocks it.
    #[test]
    fn first_paint_blocks_script_urls() {
        let paint = |name: &str, json: &str| {
            let mut s = String::new();
            js_attr(&mut s, name, Js(json));
            s
        };
        for (name, json) in [
            ("href", "\"javascript:alert(1)\""),
            ("src", "\" \\u0001JavaScript:x\""),
            ("formaction", "\"java\\tscript:x\""),
            ("xlink:href", "\"vbscript:x\""),
            ("HREF", "\"javascript:x\""),
        ] {
            assert_eq!(
                paint(name, json),
                format!(" {name}=\"about:invalid#blocked\""),
                "{json}"
            );
        }
        assert_eq!(
            paint("href", "\"/a?b=javascript:c\""),
            " href=\"/a?b=javascript:c\""
        );
        assert_eq!(
            paint("title", "\"javascript:x\""),
            " title=\"javascript:x\""
        );
        assert_eq!(paint("href", "null"), "");
        let mut s = String::new();
        js_attrs(&mut s, Js("{\"href\":\"javascript:x\",\"id\":\"a\"}"));
        assert_eq!(s, " href=\"about:invalid#blocked\" id=\"a\"");
    }

    /// The first paint of `class`, `style` and URL attributes against what
    /// live.js sets, for the values where JavaScript and JSON differ most
    /// (0, "", null, false, true, nesting). Run by Node when there is one;
    /// skipped quietly otherwise. Also checks that the minified runtime parses.
    #[test]
    fn first_paint_matches_the_browser_runtime() {
        let source = wisp_shared::LIVE_JS.replace("\r\n", "\n");
        let from = source.find("// What watchers write.").unwrap();
        let attr = source[from..].find("function attr(").unwrap() + from;
        let to = source[attr..].find("\n}\n").unwrap() + attr + 3;
        let cases: &[(&str, &str)] = &[
            (
                "class",
                "[0,\"\",null,false,true,\"a\",[\"b\",0,[\"\"]],{\"c\":0,\"d\":1,\"e\":\"\",\"f\":[]},5]",
            ),
            ("class", "{\"x\":true,\"y\":null,\"z\":\"0\"}"),
            ("class", "0"),
            ("class", "\"\""),
            ("class", "true"),
            ("class", "[]"),
            (
                "style",
                "{\"color\":\"red\",\"fontSize\":\"2em\",\"--x\":0,\"a\":null,\"b\":false,\"c\":\"\"}",
            ),
            ("style", "\"color: red\""),
            ("title", "0"),
            ("title", "false"),
            ("title", "true"),
            ("title", "null"),
            ("aria-hidden", "false"),
            ("href", "\"javascript:alert(1)\""),
            ("href", "\" \\u0001java\\nscript:x\""),
            ("href", "\"https://a.b/?javascript:x\""),
            ("src", "\"VBScript:x\""),
        ];
        let helpers = source.find("const str = ").unwrap();
        let helpers =
            &source[helpers..source[helpers..].find("// The root's listener").unwrap() + helpers];
        let mut js = String::from("const X = {};\n");
        js.push_str(helpers);
        js.push_str(&source[from..to]);
        js.push_str("\nfor (const [a, j] of ");
        let list: Vec<(&str, &str)> = cases.to_vec();
        list.json(&mut js);
        js.push_str(
            ") { let r = null; const el = { setAttribute: (n, v) => (r = v), removeAttribute: () => (r = null) };\n\
             attr(JSON.parse(j), true, el, a); console.log(r == null ? '-' : '+' + encodeURIComponent(r)); }\n",
        );
        let run = std::process::Command::new("node")
            .args(["-e", &js])
            .output();
        let Ok(run) = run else {
            return; // no Node here
        };
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let out = String::from_utf8(run.stdout).unwrap();
        for ((name, json), line) in cases.iter().zip(out.lines()) {
            let browser = line
                .strip_prefix('+')
                .map(|v| crate::cx::decode(v.as_bytes(), false).into_owned());
            let mut s = String::new();
            js_attr(&mut s, name, Js(json));
            let server = s.strip_prefix(&format!(" {name}")).map(|v| {
                let v = v
                    .strip_prefix("=\"")
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(v);
                v.replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            });
            assert_eq!(server, browser, "{name}={json}");
        }
        assert_eq!(out.lines().count(), cases.len());

        // The runtime as release builds serve it (minified by build.rs)
        // still parses.
        let dir = std::env::temp_dir();
        for (file, min) in [
            (
                "live.mjs",
                include_str!(concat!(env!("OUT_DIR"), "/live.js")),
            ),
            (
                "wisp.js",
                include_str!(concat!(env!("OUT_DIR"), "/wisp.js")),
            ),
        ] {
            let path = dir.join(format!("wisp-check-{}-{file}", std::process::id()));
            std::fs::write(&path, min).unwrap();
            let checked = std::process::Command::new("node")
                .arg("--check")
                .arg(&path)
                .output();
            let _ = std::fs::remove_file(&path);
            let checked = checked.unwrap();
            let why = String::from_utf8_lossy(&checked.stderr);
            assert!(checked.status.success(), "{file}: {why}");
        }
    }

    /// JSON values and whether JavaScript counts them as true.
    const TRUTHY: &[(&str, bool)] = &[
        ("null", false),
        ("false", false),
        ("0", false),
        ("-0", false),
        ("0.0", false),
        ("0e5", false),
        ("\"\"", false),
        ("true", true),
        ("1", true),
        ("-1.5", true),
        ("\"0\"", true),
        ("\"false\"", true),
        ("[]", true),
        ("{}", true),
    ];

    /// `Js::truthy` against the table, and the table against JavaScript's
    /// own truthiness and live.js's `class` names (run by Node when there
    /// is one; skipped quietly otherwise).
    #[test]
    fn truthy_as_javascript() {
        for &(json, want) in TRUTHY {
            assert_eq!(Js(json).truthy(), want, "{json}");
        }
        let source = wisp_shared::LIVE_JS.replace("\r\n", "\n");
        let helpers = source.find("const str = ").unwrap();
        let end = source[helpers..].find("// The root's listener").unwrap() + helpers;
        let mut js = String::from("const X = {};\n");
        js.push_str(&source[helpers..end]);
        js.push_str("\nfor (const j of ");
        let list: Vec<&str> = TRUTHY.iter().map(|t| t.0).collect();
        list.json(&mut js);
        js.push_str(") { const v = JSON.parse(j); console.log(String(!!v), cls({ a: v })); }\n");
        let Ok(run) = std::process::Command::new("node")
            .args(["-e", &js])
            .output()
        else {
            return; // no Node here
        };
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let out = String::from_utf8(run.stdout).unwrap();
        let mut lines = out.lines();
        for &(json, want) in TRUTHY {
            let class = if want { "a" } else { "" };
            assert_eq!(lines.next(), Some(&*format!("{want} {class}")), "{json}");
        }
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn script_strings() {
        let s = "a\"\\\n\r\t<>&\u{1}\u{2028}\u{2029}\u{2027}é";
        let mut out = String::new();
        let mut done = 0;
        script_escape(&mut out, s, &mut done, 0, s.len());
        out.push_str(&s[done..]);
        assert_eq!(
            out,
            "a\\\"\\\\\\n\\r\\t\\u003c\\u003e\\u0026\\u0001\\u2028\\u2029\u{2027}é"
        );
        for c in 0..=255u8 {
            let plain = c >= 0x20 && !matches!(c, b'"' | b'\\' | b'<' | b'>' | b'&' | 0xe2);
            assert_eq!(script_special(c), !plain, "{c}");
        }
    }

    #[test]
    fn versions() {
        assert!(same_version(RUNTIME_VERSION));
        assert!(!same_version("0.0.0-other"));
    }

    #[test]
    fn instances_and_tail() {
        static A: ClientModule = ClientModule {
            id: "t1",
            path: "/_app/c/t1.js",
            url: "/_app/c/t1.js?v=1",
            etag: "\"1\"",
            source: "",
            preload: "",
        };
        static B: ClientModule = ClientModule {
            id: "t2",
            path: "/_app/c/t2.js",
            url: "/_app/c/t2.js?v=2",
            etag: "\"2\"",
            source: "",
            preload: "",
        };
        let mut out = Out::default();
        assert_eq!(tail_of(&out), "");
        // B renders inside the first A; the second A comes after both.
        for (m, blob, ends) in [(&A, "{}", 0), (&B, "{\"x\":1}", 2), (&A, "{}", 1)] {
            let (_, b) = live(&mut out, m);
            b.push_str(blob);
            for _ in 0..ends {
                live_end(&mut out);
            }
        }
        let tail = tail_of(&out);
        assert!(
            tail.starts_with(
                "<script type=\"application/json\" id=\"wisp-live\">{\"m\":{\"t1\":\"/_app/c/t1.js?v=1\",\"t2\":\"/_app/c/t2.js?v=2\"},\
                 \"i\":[[0,\"t1\",-1,{}],[1,\"t2\",0,{\"x\":1}],[2,\"t1\",-1,{}]]}</script><link rel=\"modulepreload\" href=\"/_app/c/t1.js?v=1\">"
            ),
            "{tail}"
        );
        assert!(tail.ends_with(concat!(
            "<script type=\"module\" src=\"/_app/live.js?v=",
            env!("WISP_RUNTIME_V"),
            "\"></script>"
        )));
        out.live.clear();
        assert_eq!(tail_of(&out), "");

        // An island and what renders inside it wait, and nothing loads the
        // runtime or preloads their module; `client:none` sends nothing.
        for (how, m, blob, ends) in [
            (Some("v"), &A, "{}", 0),
            (None, &B, "{}", 2),
            (Some("n"), &B, "{\"secret\":1}", 1),
        ] {
            if let Some(h) = how {
                live_how(&mut out, h);
            }
            let (_, b) = live(&mut out, m);
            b.push_str(blob);
            for _ in 0..ends {
                live_end(&mut out);
            }
        }
        let tail = tail_of(&out);
        assert!(
            tail.contains("\"i\":[[0,\"t1\",-1,{},\"v\"],[1,\"t2\",0,{}]]}</script>")
                && !tail.contains("secret")
                && !tail.contains("modulepreload")
                && !tail.contains("live.js"),
            "{tail}"
        );
    }
}
