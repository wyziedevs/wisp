//! Translations at run time. The build compiles `src/locales/*.json` into
//! tables, one entry per locale for each key a template uses, and each
//! `{t("key", n)}` into an index into them: no lookup by key here. This is
//! the rest: the messages' parts as the tables hold them, writing one out,
//! and which locale a request gets.

use crate::Cx;
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use wisp_shared::plural;

/// A piece of a message: text, a placeholder (by its index among the
/// key's), or a plural on one, by case.
pub enum Part {
    Text(&'static str),
    Arg(u8),
    Plural(u8, &'static [(Case, &'static [Part])]),
}

/// A plural's case: `=N`, or a CLDR category (`wisp_shared::plural`).
pub enum Case {
    Is(u64),
    Cat(u8),
}

/// A message in one locale, with that locale's plural rule.
pub struct Msg {
    pub rule: u8,
    pub parts: &'static [Part],
}

/// A placeholder's value: a count (what a plural picks its case by), or
/// anything else that displays.
pub enum Arg<'a> {
    Num(i128),
    Text(&'a dyn fmt::Display),
}

/// What a plural counts: whole numbers.
#[diagnostic::on_unimplemented(
    message = "a plural counts by a whole number, not `{Self}`",
    label = "the value of a `{{n, plural, …}}` placeholder"
)]
pub trait Count {
    fn count(&self) -> i128;
}

macro_rules! count {
    ($($t:ty)*) => {$(
        impl Count for $t {
            #[inline]
            fn count(&self) -> i128 {
                *self as i128
            }
        }
    )*};
}
count!(u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize);

impl<T: Count + ?Sized> Count for &T {
    #[inline]
    fn count(&self) -> i128 {
        (**self).count()
    }
}

/// A message with its values, written when displayed: `{t("cart.items",
/// count)}` in a template.
pub struct Tr<'a, const N: usize> {
    msg: &'static Msg,
    args: [Arg<'a>; N],
}

impl<'a, const N: usize> Tr<'a, N> {
    #[inline]
    pub fn new(msg: &'static Msg, args: [Arg<'a>; N]) -> Self {
        Tr { msg, args }
    }
}

impl<const N: usize> fmt::Display for Tr<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write(f, self.msg.parts, self.msg.rule, &self.args)
    }
}

fn write(f: &mut fmt::Formatter<'_>, parts: &[Part], rule: u8, args: &[Arg]) -> fmt::Result {
    for p in parts {
        match p {
            Part::Text(t) => f.write_str(t)?,
            Part::Arg(i) => match args.get(*i as usize) {
                Some(Arg::Num(n)) => write!(f, "{n}")?,
                Some(Arg::Text(v)) => v.fmt(f)?,
                None => {}
            },
            Part::Plural(i, cases) => {
                let n = match args.get(*i as usize) {
                    Some(Arg::Num(n)) => *n,
                    _ => 0,
                };
                let cat =
                    plural::category(rule, u64::try_from(n.unsigned_abs()).unwrap_or(u64::MAX));
                let case = |want: &dyn Fn(&Case) -> bool| cases.iter().find(|(c, _)| want(c));
                let body = case(&|c| matches!(c, Case::Is(x) if i128::from(*x) == n))
                    .or_else(|| case(&|c| matches!(c, Case::Cat(x) if *x == cat)))
                    .or_else(|| case(&|c| matches!(c, Case::Cat(5))));
                if let Some((_, body)) = body {
                    write(f, body, rule, args)?;
                }
            }
        }
    }
    Ok(())
}

/// The app's locales, `src/locales`' file names (see [`ready`]).
static LOCALES: OnceLock<&'static [&'static str]> = OnceLock::new();
/// The one a request gets when nothing names one, by index.
static DEFAULT: AtomicU8 = AtomicU8::new(0);

/// At startup: the app's locales.
pub(crate) fn ready(list: &'static [&'static str]) {
    let _ = LOCALES.set(list);
}

/// The app's locales, in file name order (`src/locales/en.json` is `en`):
/// for a language switcher. Empty without `src/locales`.
pub fn locales() -> &'static [&'static str] {
    LOCALES.get().copied().unwrap_or(&[])
}

/// The locale a request gets when its URL, `lang` cookie and
/// `Accept-Language` name none of the app's: the first, unless this, in
/// `init`, says otherwise. An error for a locale the app does not have.
pub fn default_locale(name: &str) -> crate::Result {
    match locales().iter().position(|l| *l == name) {
        Some(i) => {
            DEFAULT.store(i as u8, Ordering::Relaxed);
            Ok(())
        }
        None => Err(crate::Error::new(
            500,
            format!(
                "default_locale(\"{name}\"): no src/locales/{name}.json (the locales are {:?})",
                locales()
            ),
        )),
    }
}

/// `path` in `locale`: its first segment, if a locale, replaced, else
/// `locale` put first. `localize("/fr/about", "en")` is `/en/about`,
/// `localize("/about", "fr")` is `/fr/about`. For links to a route under
/// `[[lang=locale]]`.
pub fn localize(path: &str, locale: &str) -> String {
    localize_in(locales(), path, locale)
}

fn localize_in(list: &[&str], path: &str, locale: &str) -> String {
    let rest = path.strip_prefix('/').unwrap_or(path);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let tail = match end > 0 && list.contains(&&rest[..end]) {
        true => &rest[end..],
        false => rest,
    };
    match tail.is_empty() || tail.starts_with(['/', '?', '#']) {
        true => format!("/{locale}{tail}"),
        false => format!("/{locale}/{tail}"),
    }
}

/// The locale of the request, by index into [`locales`]: the route's
/// `lang` parameter, else the `lang` cookie, else the best of its
/// `Accept-Language`, else the default.
pub(crate) fn pick(cx: &Cx) -> u8 {
    let list = locales();
    if list.is_empty() {
        return 0;
    }
    let find = |s: &str| list.iter().position(|l| l.eq_ignore_ascii_case(s));
    let named = (cx.route_param("lang").and_then(find))
        .or_else(|| cx.cookie("lang").and_then(find))
        .or_else(|| cx.header("accept-language").and_then(|h| best(h, list)));
    named.map_or_else(|| DEFAULT.load(Ordering::Relaxed), |i| i as u8)
}

/// The locale of `list` an `Accept-Language` value likes most: by `q`,
/// then by order; `fr-CA` takes `fr`, and `fr` takes `fr-FR`.
fn best(header: &str, list: &[&str]) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for item in header.split(',') {
        let mut it = item.split(';');
        let tag = it.next().unwrap_or("").trim();
        let q = it
            .find_map(|p| p.trim().strip_prefix("q="))
            .map_or(1.0, |q| q.trim().parse().unwrap_or(0.0));
        // `NaN`, `inf` and the like parse; a weight is 0 to 1.
        let q = if (0.0..=1.0).contains(&q) { q } else { 0.0 };
        if q <= 0.0 || tag.is_empty() || tag == "*" || best.is_some_and(|(b, _)| b >= q) {
            continue;
        }
        let base = |a: &str, b: &str| {
            let a = a.as_bytes();
            a.len() > b.len()
                && a[..b.len()].eq_ignore_ascii_case(b.as_bytes())
                && matches!(a[b.len()], b'-' | b'_')
        };
        let found = (list.iter().position(|l| l.eq_ignore_ascii_case(tag)))
            .or_else(|| list.iter().position(|l| base(tag, l) || base(l, tag)));
        if let Some(i) = found {
            best = Some((q, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Where the locale goes in the shell's `<html lang="…">`: the value's
/// range, if the shell has one.
pub(crate) fn lang_value(shell: &str) -> Option<(usize, usize)> {
    let html = shell.find("<html")?;
    let tag = &shell[html..html + shell[html..].find('>')?];
    let at = html + tag.find(" lang=\"")? + 7;
    Some((at, at + shell[at..].find('"')?))
}

#[cfg(test)]
mod tests {
    use super::*;

    static ONE: [Part; 2] = [Part::Arg(0), Part::Text(" item")];
    static MANY: [Part; 2] = [Part::Arg(0), Part::Text(" items")];
    static NONE: [Part; 1] = [Part::Text("No items")];
    static CART: Msg = Msg {
        rule: 1,
        parts: &[
            Part::Text("Cart: "),
            Part::Plural(
                0,
                &[
                    (Case::Is(0), &NONE),
                    (Case::Cat(1), &ONE),
                    (Case::Cat(5), &MANY),
                ],
            ),
            Part::Text(", "),
            Part::Arg(1),
        ],
    };

    #[test]
    fn writes_messages() {
        let t =
            |n: i64, who: &str| Tr::new(&CART, [Arg::Num(n.count()), Arg::Text(&who)]).to_string();
        assert_eq!(t(0, "Ann"), "Cart: No items, Ann");
        assert_eq!(t(1, "<b>"), "Cart: 1 item, <b>");
        assert_eq!(t(-5, "x"), "Cart: -5 items, x");
        assert_eq!((&&3u8).count(), 3);
    }

    #[test]
    fn accept_language() {
        let list = ["en", "fr", "pt-BR"];
        let b = |h: &str| best(h, &list).map(|i| list[i]);
        assert_eq!(b("fr-CA,fr;q=0.9,en;q=0.8"), Some("fr"));
        assert_eq!(b("de, en;q=0.5, fr;q=0.7"), Some("fr"));
        assert_eq!(b("pt"), Some("pt-BR"));
        assert_eq!(b("PT-br"), Some("pt-BR"));
        assert_eq!(b("de, *;q=0.1"), None);
        assert_eq!(b("fr;q=0, en"), Some("en"));
        assert_eq!(b(""), None);
        // Not ASCII, not a weight: no panic, no winner.
        assert_eq!(b("日本, fr;q=NaN, en;q=inf, é-x"), None);
        assert_eq!(b("日本語, fr"), Some("fr"));
    }

    #[test]
    fn localizes_paths() {
        let l = |p: &str, to: &str| localize_in(&["en", "fr"], p, to);
        assert_eq!(l("/fr/about?x=1", "en"), "/en/about?x=1");
        assert_eq!(l("/about", "fr"), "/fr/about");
        assert_eq!(l("/fr", "en"), "/en");
        assert_eq!(l("/", "fr"), "/fr");
        assert_eq!(l("/?q=1", "fr"), "/fr?q=1");
        assert_eq!(l("/french/x", "en"), "/en/french/x");
    }

    #[test]
    fn finds_the_shells_lang() {
        let s = "<!doctype html>\n<html lang=\"en\">\n<head>";
        assert_eq!(lang_value(s).map(|(a, b)| &s[a..b]), Some("en"));
        assert_eq!(lang_value("<html><body lang=\"x\">"), None);
        assert_eq!(lang_value("<p>"), None);
    }
}
