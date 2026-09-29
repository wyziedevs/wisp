//! HTML escaping. Every `{expr}` in a template ends up here.

use std::fmt::{self, Display, Write};

/// Appends `s` with `& < > " '` escaped: safe in text and in quoted attributes.
pub fn escape(out: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut done = 0; // `s[..done]` is in `out` already
    // 16 bytes at a time. The loop has no branch or early exit, so it
    // compiles to a few vector compares, and a chunk with nothing to escape
    // (the usual case) is skipped without looking at its bytes one by one:
    // 3.5x the speed of a byte loop on plain text.
    let (chunks, rest) = bytes.as_chunks::<16>();
    for (n, chunk) in chunks.iter().enumerate() {
        let mut hit = 0u8;
        for &b in chunk {
            hit |= special(b) as u8;
        }
        if hit != 0 {
            escape_bytes(out, s, &mut done, n * 16, n * 16 + 16);
        }
    }
    escape_bytes(out, s, &mut done, bytes.len() - rest.len(), bytes.len());
    out.push_str(&s[done..]);
}

/// `& ' < > "` in three compares: `&` and `'` differ only in bit 0, `<` and
/// `>` only in bit 1.
#[inline(always)]
fn special(b: u8) -> bool {
    ((b | 1) == b'\'') | ((b | 2) == b'>') | (b == b'"')
}

/// Escapes `s[from..to]`, appending what precedes each escaped byte first.
#[inline(always)]
fn escape_bytes(out: &mut String, s: &str, done: &mut usize, from: usize, to: usize) {
    for (i, &b) in s.as_bytes()[from..to].iter().enumerate() {
        let replacement = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        // Escaped bytes are ASCII, so `from + i` is a char boundary.
        out.push_str(&s[*done..from + i]);
        out.push_str(replacement);
        *done = from + i + 1;
    }
}

/// Adapter so any `Display` value is escaped as it is formatted, without an
/// intermediate `String`.
struct Escaper<'a>(&'a mut String);

impl Write for Escaper<'_> {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        escape(self.0, s);
        Ok(())
    }
}

/// `{expr}` in a template compiles to `(&Text(&expr)).put(out)`. Strings,
/// integers, `bool` and `char` are written directly; anything else goes
/// through its `Display`, which costs a formatter per value (the numbers of
/// a 13-row table were 5% of its request). Which one runs is decided at
/// compile time: method lookup tries the receiver `&Text` before `&&Text`,
/// so a type with a `Direct` impl never reaches `Formatted`.
pub struct Text<'a, T: ?Sized>(pub &'a T);

pub trait Direct {
    fn put(&self, out: &mut String);
}

pub trait Formatted {
    fn put(&self, out: &mut String);
}

impl<T: Display + ?Sized> Formatted for &Text<'_, T> {
    #[inline]
    fn put(&self, out: &mut String) {
        text(out, self.0);
    }
}

impl Direct for Text<'_, str> {
    #[inline]
    fn put(&self, out: &mut String) {
        escape(out, self.0);
    }
}

impl Direct for Text<'_, String> {
    #[inline]
    fn put(&self, out: &mut String) {
        escape(out, self.0);
    }
}

impl Direct for Text<'_, std::borrow::Cow<'_, str>> {
    #[inline]
    fn put(&self, out: &mut String) {
        escape(out, self.0);
    }
}

impl Direct for Text<'_, bool> {
    #[inline]
    fn put(&self, out: &mut String) {
        out.push_str(if *self.0 { "true" } else { "false" });
    }
}

impl Direct for Text<'_, char> {
    #[inline]
    fn put(&self, out: &mut String) {
        escape(out, self.0.encode_utf8(&mut [0; 4]));
    }
}

/// `&str`, `&String`, `&u32`...: whatever the referent writes.
impl<T: ?Sized> Direct for Text<'_, &T>
where
    for<'a> Text<'a, T>: Direct,
{
    #[inline]
    fn put(&self, out: &mut String) {
        Text(*self.0).put(out);
    }
}

macro_rules! unsigned {
    ($($t:ty)*) => {$(
        impl Direct for Text<'_, $t> {
            #[inline]
            fn put(&self, out: &mut String) {
                decimal(out, *self.0 as u64);
            }
        }
    )*};
}

macro_rules! signed {
    ($($t:ty)*) => {$(
        impl Direct for Text<'_, $t> {
            #[inline]
            fn put(&self, out: &mut String) {
                if *self.0 < 0 {
                    out.push('-');
                }
                decimal(out, self.0.unsigned_abs() as u64);
            }
        }
    )*};
}

unsigned!(u8 u16 u32 u64 usize);
signed!(i8 i16 i32 i64 isize);

fn decimal(out: &mut String, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&digits[i..]).expect("digits are ASCII"));
}

/// `href={expr}` in a template compiles to `(&Attr(&expr)).get()`: the
/// attribute is written for `Some(v)` and left out for `None`. An `Option`
/// is unwrapped and every other value is always there; which is decided at
/// compile time, as for `Text`.
pub struct Attr<'a, T: ?Sized>(pub &'a T);

pub trait Maybe {
    type Value: ?Sized;
    fn get(&self) -> Option<&Self::Value>;
}

pub trait Always {
    type Value: ?Sized;
    fn get(&self) -> Option<&Self::Value>;
}

impl<T> Maybe for Attr<'_, Option<T>> {
    type Value = T;
    fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

/// A field bound by reference, as the template sees non-`Copy` fields.
impl<T> Maybe for Attr<'_, &Option<T>> {
    type Value = T;
    fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

impl<T: ?Sized> Always for &Attr<'_, T> {
    type Value = T;
    fn get(&self) -> Option<&T> {
        Some(self.0)
    }
}

/// Any `Display` value, escaped.
#[inline]
pub fn text<T: Display + ?Sized>(out: &mut String, value: &T) {
    // Writing to a String cannot fail; a Display impl that errors just stops.
    let _ = write!(Escaper(out), "{value}");
}

/// `{@html expr}`
#[inline]
pub fn raw<T: Display + ?Sized>(out: &mut String, value: &T) {
    let _ = write!(out, "{value}");
}

/// Ends the value of a URL attribute (`href`, `src`, `action`...) that
/// began at `out[start..]` and whose scheme an expression chose. A value
/// that would run script when followed is replaced, as React and Angular
/// do, so `href={link}` is safe whatever `link` holds.
pub fn guard_url(out: &mut String, start: usize) {
    if runs_script(&out[start..]) {
        out.truncate(start);
        out.push_str("about:invalid#blocked");
    }
}

/// Reads `url` (escaped, as it stands in the page) the way a browser does:
/// leading spaces and controls dropped, tabs and newlines ignored, the
/// scheme being what comes before a `:` if every byte before it can be in
/// one. A character reference the browser would decode there counts as
/// script, except the ones `escape` writes, none of which can be in a
/// scheme.
fn runs_script(url: &str) -> bool {
    let mut scheme = [0u8; 10];
    let mut n = 0;
    let b = url.trim_start_matches(|c: char| c <= ' ').as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\t' | b'\n' | b'\r' => {}
            b':' => return matches!(&scheme[..n], b"javascript" | b"vbscript"),
            b'&' => {
                let escaped = ["amp;", "lt;", "gt;", "quot;", "#39;"]
                    .iter()
                    .any(|e| b[i + 1..].starts_with(e.as_bytes()));
                return !escaped;
            }
            _ if c.is_ascii_alphabetic()
                || (n > 0 && (c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.'))) =>
            {
                if n == scheme.len() {
                    return false; // longer than any scheme that runs script
                }
                scheme[n] = c.to_ascii_lowercase();
                n += 1;
            }
            _ => return false, // not a scheme's byte: a relative URL
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let mut s = String::new();
        escape(&mut s, r#"<a href="x">Tom & 'Jerry'</a> ünïcödé"#);
        assert_eq!(
            s,
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt; ünïcödé"
        );

        // Escapes on both sides of the 16-byte chunk edges, and none at all.
        for (input, want) in [
            ("", ""),
            ("0123456789abcde<", "0123456789abcde&lt;"),
            ("0123456789abcdef<", "0123456789abcdef&lt;"),
            (
                "<123456789abcdef0123456789abcdef>",
                "&lt;123456789abcdef0123456789abcdef&gt;",
            ),
            (
                "ünïcödé ünïcödé ünïcödé & ünïcödé",
                "ünïcödé ünïcödé ünïcödé &amp; ünïcödé",
            ),
            (
                "nothing to escape in this long line at all",
                "nothing to escape in this long line at all",
            ),
        ] {
            let mut s = String::new();
            escape(&mut s, input);
            assert_eq!(s, want, "{input}");
        }
    }

    #[test]
    #[allow(clippy::needless_borrow)] // The `&` is what lets a template pick `Direct` or `Formatted`.
    fn direct_values() {
        struct Shown;
        impl Display for Shown {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("<shown>")
            }
        }
        let s = &mut String::new();
        let (text, owned, number) = ("a<b", String::from("&"), 42u32);
        (&Text(&text)).put(s);
        (&Text(&owned)).put(s);
        (&Text(&&owned)).put(s);
        (&Text(&number)).put(s);
        (&Text(&&number)).put(s);
        (&Text(&-9_i64)).put(s);
        (&Text(&i64::MIN)).put(s);
        (&Text(&u64::MAX)).put(s);
        (&Text(&0u8)).put(s);
        (&Text(&true)).put(s);
        (&Text(&'"')).put(s);
        (&Text(&1.5f64)).put(s);
        (&Text(&Shown)).put(s);
        assert_eq!(
            s,
            "a&lt;b&amp;&amp;4242-9-9223372036854775808184467440737095516150true&quot;1.5&lt;shown&gt;"
        );
    }

    #[test]
    fn urls_that_run_script_are_blocked() {
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " \x01javascript:x",
            "java\tscript:x",
            "java\nscript:x",
            "vbscript:x",
            "javascript&#58;x",
            "javascript&colon;x",
        ] {
            let mut s = String::from("<a href=\"");
            let start = s.len();
            s.push_str(bad);
            guard_url(&mut s, start);
            assert_eq!(s, "<a href=\"about:invalid#blocked", "{bad:?}");
        }
        for good in [
            "https://x.com/a?b=javascript:c",
            "/javascript:x",
            "mailto:a@b.c",
            "don&#39;t:x",
            "a&amp;b",
            "",
            "#top",
            "1javascript:x",
            "javascripts:x",
        ] {
            let mut s = String::from(good);
            guard_url(&mut s, 0);
            assert_eq!(s, good);
        }
    }

    #[test]
    fn display_values() {
        let mut s = String::new();
        text(&mut s, &42);
        text(&mut s, "<b>");
        text(&mut s, &format_args!("{}&{}", 1, 2));
        raw(&mut s, "<i>");
        assert_eq!(s, "42&lt;b&gt;1&amp;2<i>");
    }

    // The `&` is what makes method lookup try `Maybe` before `Always`.
    #[allow(clippy::needless_borrow)]
    #[test]
    fn options_leave_attributes_out() {
        let some = Some("a\"b");
        assert_eq!((&Attr(&some)).get(), Some(&"a\"b"));
        assert_eq!((&Attr(&None::<u8>)).get(), None);
        assert_eq!((&Attr(&&Some(1u8))).get(), Some(&1));
        assert_eq!((&Attr(&"x")).get(), Some(&"x"));
        assert_eq!((&Attr(&7)).get(), Some(&7));
    }
}
