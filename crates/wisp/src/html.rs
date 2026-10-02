//! What a template's holes write: every `{expr}` ends up here, escaped by
//! `contexts.rs`.

use crate::contexts::escape;
use std::fmt::{self, Display, Write};

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

/// `{cx.problem("email")}`: the value when there is one, else nothing.
impl<T: Display> Direct for Text<'_, Option<T>> {
    #[inline]
    fn put(&self, out: &mut String) {
        if let Some(v) = self.0 {
            text(out, v);
        }
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
                crate::decimal(out, *self.0 as u64);
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
                crate::decimal(out, self.0.unsigned_abs() as u64);
            }
        }
    )*};
}

unsigned!(u8 u16 u32 u64 usize);
signed!(i8 i16 i32 i64 isize);

/// A float prints digits, `.`, `-`, `e`, `inf` or `NaN`: nothing to escape.
macro_rules! float {
    ($($t:ty)*) => {$(
        impl Direct for Text<'_, $t> {
            #[inline]
            fn put(&self, out: &mut String) {
                let _ = write!(out, "{}", self.0);
            }
        }
    )*};
}

float!(f32 f64);

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

#[cfg(test)]
mod tests {
    use super::*;

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
