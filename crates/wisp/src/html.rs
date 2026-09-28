//! HTML escaping. Every `{expr}` in a template ends up here.

use std::fmt::{self, Display, Write};

/// Replacement index per byte; 0 means "copy as is".
static ESCAPE: [u8; 256] = {
    let mut t = [0u8; 256];
    t[b'&' as usize] = 1;
    t[b'<' as usize] = 2;
    t[b'>' as usize] = 3;
    t[b'"' as usize] = 4;
    t[b'\'' as usize] = 5;
    t
};
const REPLACEMENT: [&str; 6] = ["", "&amp;", "&lt;", "&gt;", "&quot;", "&#39;"];

/// Appends `s` with `& < > " '` escaped: safe in text and in quoted attributes.
#[inline]
pub fn escape(out: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let r = ESCAPE[b as usize];
        if r != 0 {
            // Escaped bytes are ASCII, so `i` is always a char boundary.
            out.push_str(&s[start..i]);
            out.push_str(REPLACEMENT[r as usize]);
            start = i + 1;
        }
    }
    out.push_str(&s[start..]);
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

/// `{expr}`
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let mut s = String::new();
        escape(&mut s, r#"<a href="x">Tom & 'Jerry'</a> ünïcödé"#);
        assert_eq!(s, "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt; ünïcödé");
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
}
