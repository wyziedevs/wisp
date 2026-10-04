//! What the build reads from a Rust type's text, in one place: the OpenAPI
//! document and the TypeScript client describe types by it, routing needs
//! to know an integer `id`, and the checks that name a mistake against the
//! user's line read it. How a value is read or answered at run time is
//! rustc's to decide (see `wisp::rt_traits`), not this file's.

/// `t` with no whitespace but one space between two words: `Option < u8 >`
/// is `Option<u8>`, `&'a  mut T` is `&'a mut T`. Types are compared as
/// text squeezed this way.
pub fn squeeze(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut gap = false;
    for c in t.trim().chars() {
        if c.is_whitespace() {
            gap = true;
            continue;
        }
        let word = |c: char| c.is_ascii() && is_word(c as u8);
        if gap && out.chars().next_back().is_some_and(word) && word(c) {
            out.push(' ');
        }
        gap = false;
        out.push(c);
    }
    out
}

pub use wisp_shared::rust::is_word;

/// A plain identifier: `slug`, `_x`, `r2`; not `r#type`, `2a` or ``.
pub fn is_ident(s: &str) -> bool {
    s.bytes().next().is_some_and(|c| !c.is_ascii_digit()) && s.bytes().all(is_word)
}

/// Whether `s` is a Rust keyword (or `_`), which a `let` binds as `r#s`.
pub fn is_keyword(s: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "_", "Self", "abstract", "as", "async", "await", "become", "box", "break", "const",
        "continue", "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for",
        "gen", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut",
        "override", "priv", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
        "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while",
        "yield",
    ];
    KEYWORDS.contains(&s)
}

/// Whether `s` has no raw form: `r#self` is not a name.
pub fn is_unrawable(s: &str) -> bool {
    matches!(s, "_" | "self" | "Self" | "crate" | "super")
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

/// What is inside the outer `<...>` of `t`: `Vec<Option<u8>>` →
/// `Option<u8>`.
pub fn inner(t: &str) -> Option<&str> {
    let open = t.find('<')?;
    t[open + 1..].trim_end().strip_suffix('>')
}

/// The first type argument: `Result<Option<Response>, E>` →
/// `Option<Response>`.
pub fn first_arg(t: &str) -> Option<&str> {
    let inner = inner(t)?;
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

/// `T` of an `Option<T>`.
pub fn option_inner(t: &str) -> Option<&str> {
    (last_segment(t) == "Option")
        .then(|| first_arg(t))
        .flatten()
}

/// `t` without a leading reference: `&'static str` → `str`, `&mut T` → `T`.
pub fn unref(t: &str) -> &str {
    let Some(r) = t.trim().strip_prefix('&') else {
        return t.trim();
    };
    let r = r.trim_start();
    let r = match r.strip_prefix('\'') {
        Some(life) => life.trim_start_matches(|c: char| c.is_ascii() && is_word(c as u8)),
        None => r,
    };
    let r = r.trim_start();
    r.strip_prefix("mut ").unwrap_or(r).trim()
}

/// The simple types the build tells apart, by their last segment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    Unsigned,
    Signed,
    Float,
    Bool,
    Char,
    /// `String`, `str`, `Cow<str>`.
    Text,
    Unit,
    Other,
}

/// What simple type `t` (squeezed or not) is.
pub fn scalar(t: &str) -> Scalar {
    let t = t.trim();
    if t == "()" {
        return Scalar::Unit;
    }
    match last_segment(t) {
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Scalar::Unsigned,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Scalar::Signed,
        "f32" | "f64" => Scalar::Float,
        "bool" => Scalar::Bool,
        "char" => Scalar::Char,
        "String" | "str" | "Cow" => Scalar::Text,
        _ => Scalar::Other,
    }
}

/// The values integer type `t` (a name alone: `u8`) holds; `None` for
/// another type. `usize` may be 32 bits (wasm).
pub fn int_range(t: &str) -> Option<std::ops::RangeInclusive<i128>> {
    let (lo, hi) = match t {
        "u8" => (0, u8::MAX.into()),
        "u16" => (0, u16::MAX.into()),
        "u32" | "usize" => (0, u32::MAX.into()),
        "u64" => (0, u64::MAX.into()),
        "u128" => (0, i128::MAX),
        "i8" => (i8::MIN.into(), i8::MAX.into()),
        "i16" => (i16::MIN.into(), i16::MAX.into()),
        "i32" | "isize" => (i32::MIN.into(), i32::MAX.into()),
        "i64" => (i64::MIN.into(), i64::MAX.into()),
        "i128" => (i128::MIN, i128::MAX),
        _ => return None,
    };
    Some(lo..=hi)
}

/// An integer type.
pub fn is_integer(t: &str) -> bool {
    matches!(scalar(t), Scalar::Unsigned | Scalar::Signed)
}

/// Text, borrowed or owned: `String`, `&str`, `&'a str`, `Cow<str>`.
pub fn is_text(t: &str) -> bool {
    scalar(unref(t)) == Scalar::Text
}

/// A shared reference: `&T`, `&'a T`, not `&mut T`.
pub fn is_shared_ref(t: &str) -> bool {
    let t = squeeze(t);
    t.starts_with('&') && !t.starts_with("&mut ") && !t.contains(" mut ")
}

/// `&str` or `&'a str`: a parameter the generated call lends a `String` to.
pub fn is_str_ref(t: &str) -> bool {
    is_shared_ref(t) && unref(t) == "str"
}

/// Text, or an `Option` of it: a parameter named `body` of such a type is
/// a form field, not the whole JSON body.
pub fn is_maybe_text(t: &str) -> bool {
    is_text(t) || option_inner(t).is_some_and(is_text)
}

/// A type certainly `Copy`: a number, `bool`, `char`, a shared reference,
/// or an `Option` or tuple of those. Others are borrowed.
pub fn is_copy(t: &str) -> bool {
    let t = t.trim();
    if let Some(inner) = option_inner(t) {
        return is_copy(inner);
    }
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return inner.split(',').all(|p| p.trim().is_empty() || is_copy(p));
    }
    is_shared_ref(t)
        || (!t.contains(['<', ':'])
            && matches!(
                scalar(t),
                Scalar::Unsigned | Scalar::Signed | Scalar::Float | Scalar::Bool | Scalar::Char
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_read_from_text() {
        assert_eq!(squeeze(" Option < u8 > "), "Option<u8>");
        assert_eq!(squeeze("&'a  mut\nT"), "&'a mut T");
        assert!(is_ident("_a1") && !is_ident("1a") && !is_ident("") && !is_ident("r#x"));
        assert_eq!(
            first_arg("Result<Option<Response>, E>"),
            Some("Option<Response>")
        );
        assert_eq!(
            option_inner("std::option::Option<(u8, u8)>"),
            Some("(u8, u8)")
        );
        assert_eq!(option_inner("Vec<u8>"), None);
        assert_eq!(unref("&'static str"), "str");
        assert_eq!(unref("& 'a mut Note"), "Note");
        assert!(is_integer("u64") && is_integer("std::primitive::i8") && !is_integer("f32"));
        assert!(is_text("&'a str") && is_text("String") && is_text("Cow<'_, str>"));
        assert!(!is_text("Vec<String>"));
        assert!(is_str_ref("& 'a str") && is_str_ref("&str") && !is_str_ref("String"));
        assert!(is_maybe_text("Option<&str>") && !is_maybe_text("Note"));
        assert!(is_copy("&'static str") && is_copy("Option<(u8, char)>") && is_copy("f32"));
        assert!(!is_copy("&mut u8") && !is_copy("Vec<u8>") && !is_copy("Option<String>"));
        assert_eq!(scalar("()"), Scalar::Unit);
    }
}
