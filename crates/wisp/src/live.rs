//! Browser code: the values a page sends to its client scripts, as JSON,
//! and the record of which template instances a response rendered.
//!
//! A template with a client script or directives is compiled into an ES
//! module (`/_app/c/ID.js`). Each time it renders, it is an *instance*: its
//! elements are marked `data-w="I.G"`, and the page ends with the list of
//! instances, the server values each one's code reads, and the runtime
//! (`client/live.js`) that starts them. See `write_page` in `http.rs`.

use crate::Out;
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
}

/// The instances a response rendered, kept in its `Out`.
#[derive(Default)]
pub(crate) struct Live {
    count: u32,
    modules: Vec<&'static ClientModule>,
    /// `[0,"t3",-1,{…}],[1,"t5",0,{…}]`: id, module, the instance it
    /// renders inside (for context), and its server values.
    instances: String,
    /// The instances rendering now, innermost last.
    open: Vec<u32>,
}

impl Live {
    pub(crate) fn clear(&mut self) {
        self.count = 0;
        self.modules.clear();
        self.instances.clear();
        self.open.clear();
    }

    /// What goes at the end of the page's body when it has instances: the
    /// instances and the modules they need, the modules preloaded, and the
    /// runtime that starts them. Empty otherwise.
    pub(crate) fn tail(&self) -> String {
        if self.count == 0 {
            return String::new();
        }
        let mut s = String::with_capacity(self.instances.len() + 200 * self.modules.len() + 200);
        s.push_str("<script type=\"application/json\" id=\"wisp-live\">{\"m\":{");
        for (k, m) in self.modules.iter().enumerate() {
            let comma = if k > 0 { "," } else { "" };
            let _ = write!(s, "{comma}\"{}\":\"{}\"", m.id, m.url);
        }
        s.push_str("},\"i\":[");
        s.push_str(&self.instances);
        s.push_str("]}</script>");
        for m in &self.modules {
            let _ = write!(s, "<link rel=\"modulepreload\" href=\"{}\">", m.url);
        }
        s.push_str(concat!(
            "<script type=\"module\" src=\"/_app/live.js?v=",
            env!("CARGO_PKG_VERSION"),
            "\"></script>"
        ));
        s
    }
}

/// Starts an instance of `module`: records it and returns its id, with the
/// buffer its blob (a JSON object of the server values it reads) goes into.
/// The caller writes the blob and then the `]` that closes the record.
pub fn live<'a>(out: &'a mut Out, module: &'static ClientModule) -> (u32, &'a mut String) {
    let l = &mut out.live;
    if !l.modules.iter().any(|m| m.id == module.id) {
        l.modules.push(module);
    }
    let i = l.count;
    l.count += 1;
    if i > 0 {
        l.instances.push(',');
    }
    let parent = l.open.last().map_or(-1, |&p| p as i64);
    l.open.push(i);
    let _ = write!(l.instances, "[{i},\"{}\",{parent},", module.id);
    (i, &mut l.instances)
}

/// The instance `live` started last has rendered everything inside it.
pub fn live_end(out: &mut Out) {
    out.live.open.pop();
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
    let mut j = String::new();
    value.json(&mut j);
    let Some(inner) = j.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        if j != "null" {
            crate::html::escape(out, &j);
        }
        return;
    };
    // Undo the JSON string's escapes. `Json` writes only these.
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
    crate::html::escape(out, &s);
}

/// Whether `build`, the version of wisp-build that generated an app's code,
/// is this crate's: modules import the runtime by its version.
pub const fn same_version(build: &str) -> bool {
    let (a, b) = (build.as_bytes(), env!("CARGO_PKG_VERSION").as_bytes());
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
fn string(out: &mut String, s: &str) {
    out.push('"');
    let mut done = 0;
    for (i, c) in s.char_indices() {
        let esc = match c {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            '<' => "\\u003c",
            '>' => "\\u003e",
            '&' => "\\u0026",
            '\u{2028}' => "\\u2028",
            '\u{2029}' => "\\u2029",
            c if (c as u32) < 0x20 => {
                out.push_str(&s[done..i]);
                let _ = write!(out, "\\u{:04x}", c as u32);
                done = i + 1;
                continue;
            }
            _ => continue,
        };
        out.push_str(&s[done..i]);
        out.push_str(esc);
        done = i + c.len_utf8();
    }
    out.push_str(&s[done..]);
    out.push('"');
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

macro_rules! integers {
    ($($t:ty)*) => {$(
        impl Json for $t {
            fn json(&self, out: &mut String) {
                let _ = write!(out, "{self}");
            }
        }
    )*};
}

integers!(u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize);

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

    fn to_json<T: Json + ?Sized>(v: &T) -> String {
        let mut s = String::new();
        v.json(&mut s);
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
    fn versions() {
        assert!(same_version(env!("CARGO_PKG_VERSION")));
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
        };
        static B: ClientModule = ClientModule {
            id: "t2",
            path: "/_app/c/t2.js",
            url: "/_app/c/t2.js?v=2",
            etag: "\"2\"",
            source: "",
        };
        let mut out = Out::default();
        assert_eq!(out.live.tail(), "");
        // B renders inside the first A; the second A comes after both.
        for (m, blob, ends) in [(&A, "{}]", 0), (&B, "{\"x\":1}]", 2), (&A, "{}]", 1)] {
            let (_, b) = live(&mut out, m);
            b.push_str(blob);
            for _ in 0..ends {
                live_end(&mut out);
            }
        }
        let tail = out.live.tail();
        assert!(
            tail.starts_with(
                "<script type=\"application/json\" id=\"wisp-live\">{\"m\":{\"t1\":\"/_app/c/t1.js?v=1\",\"t2\":\"/_app/c/t2.js?v=2\"},\
                 \"i\":[[0,\"t1\",-1,{}],[1,\"t2\",0,{\"x\":1}],[2,\"t1\",-1,{}]]}</script><link rel=\"modulepreload\" href=\"/_app/c/t1.js?v=1\">"
            ),
            "{tail}"
        );
        assert!(tail.ends_with(concat!(
            "<script type=\"module\" src=\"/_app/live.js?v=",
            env!("CARGO_PKG_VERSION"),
            "\"></script>"
        )));
        out.live.clear();
        assert_eq!(out.live.tail(), "");
    }
}
