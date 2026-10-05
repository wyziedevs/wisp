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
    /// Literal text.
    Text(&'static str),
    /// Placeholder number `n` (an index among the key's arguments).
    Arg(u8),
    /// Plural on argument `n`: the first case that matches its count.
    Plural(u8, &'static [(Case, &'static [Part])]),
}

/// A plural's case: `=N`, or a CLDR category (`wisp_shared::plural`).
pub enum Case {
    /// Matches exactly this count (`=N`).
    Is(u64),
    /// Matches a CLDR plural category by number (`wisp_shared::plural`).
    Cat(u8),
}

/// A message in one locale, with that locale's plural rule.
pub struct Msg {
    /// The locale's plural rule, as `wisp_shared::plural` numbers them.
    pub rule: u8,
    /// The message, in pieces.
    pub parts: &'static [Part],
}

/// A placeholder's value: a count (what a plural picks its case by), or
/// anything else that displays.
pub enum Arg<'a> {
    /// A whole number: what a plural counts.
    Num(i128),
    /// Anything that displays.
    Text(&'a dyn fmt::Display),
}

/// What a plural counts: whole numbers.
#[diagnostic::on_unimplemented(
    message = "a plural counts by a whole number, not `{Self}`",
    label = "the value of a `{{n, plural, …}}` placeholder"
)]
pub trait Count {
    /// The value as a whole number.
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
    /// A translated message with its arguments; made by the generated code.
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

/// How `[[lang=locale]]` shows in URLs: `prefix` in `i18n = [...]` of
/// `[package.metadata.wisp]` (see [`prefix`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefix {
    /// `/about` and `/fr/about` both answer, neither redirects. What an app
    /// gets that says nothing.
    Optional,
    /// Every page has its locale: `/about` redirects (307) to the
    /// visitor's, `/fr/about`.
    Always,
    /// The default locale has none, `/about`, the others do, `/fr/about`;
    /// `/en/about` redirects (308) to `/about`. A page without a prefix is
    /// the default locale's whatever the cookie or `Accept-Language` says.
    AsNeeded,
}

/// How the app's URLs name locales: [`Prefix::Optional`] unless
/// `i18n = ["prefix always"]` or `"prefix as-needed"` in Cargo.toml says
/// otherwise.
pub fn prefix() -> Prefix {
    match PREFIX.load(Ordering::Relaxed) {
        1 => Prefix::Always,
        2 => Prefix::AsNeeded,
        _ => Prefix::Optional,
    }
}

/// `prefix`, as [`Prefix`] by number.
static PREFIX: AtomicU8 = AtomicU8::new(0);
/// Each domain of `i18n = ["domain example.fr fr"]` and its locale's index.
static DOMAINS: OnceLock<&'static [(&'static str, u8)]> = OnceLock::new();

/// At startup, from `i18n = [...]` in Cargo.toml: the default locale, the
/// prefix and the domains.
pub(crate) fn setup(default: u8, prefix: u8, domains: &'static [(&'static str, u8)]) {
    DEFAULT.store(default, Ordering::Relaxed);
    PREFIX.store(prefix, Ordering::Relaxed);
    let _ = DOMAINS.set(domains);
}

fn domains() -> &'static [(&'static str, u8)] {
    DOMAINS.get().copied().unwrap_or(&[])
}

/// The default locale's name, `""` without locales.
pub(crate) fn default_name() -> &'static str {
    locales()
        .get(DEFAULT.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or("")
}

/// The locale a request for `host` gets, by its domain.
fn by_host(host: &str) -> Option<u8> {
    host_in(domains(), host)
}

/// [`by_host`] among `domains`; a port in either may be left out.
fn host_in(domains: &[(&str, u8)], host: &str) -> Option<u8> {
    let bare = match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    let is = |d: &str| d.eq_ignore_ascii_case(host) || d.eq_ignore_ascii_case(bare);
    domains.iter().find(|(d, _)| is(d)).map(|(_, i)| *i)
}

/// The domain of `locale`, if `i18n` gave it one.
fn domain_of(locale: &str) -> Option<&'static str> {
    let list = locales();
    let has = |i: &u8| list.get(*i as usize) == Some(&locale);
    domains().iter().find(|(_, i)| has(i)).map(|(d, _)| *d)
}

/// `path` in `locale`: its first segment, if a locale, replaced, else
/// `locale` put first. `localize("/fr/about", "en")` is `/en/about`,
/// `localize("/about", "fr")` is `/fr/about`. For links to a route under
/// `[[lang=locale]]`. With `prefix as-needed` the default locale gets none
/// (`/about`), and a locale with a `domain` gets that, `//example.fr/about`.
pub fn localize(path: &str, locale: &str) -> String {
    let at = path_in(locales(), path, locale, bare(locale));
    match domain_of(locale) {
        Some(host) => format!("//{host}{at}"),
        None => at,
    }
}

/// Whether `locale` has no prefix: the default one with `prefix as-needed`,
/// and any with a domain.
fn bare(locale: &str) -> bool {
    domain_of(locale).is_some() || (prefix() == Prefix::AsNeeded && locale == default_name())
}

/// `path` with its locale segment, if it has one, taken out: what every
/// locale's is made of, and never one that begins `//`.
fn strip(list: &[&str], path: &str) -> String {
    let rest = path.strip_prefix('/').unwrap_or(path);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let tail = match end > 0 && list.contains(&&rest[..end]) {
        true => &rest[end..],
        false => rest,
    };
    format!("/{}", tail.trim_start_matches(['/', '\\']))
}

/// `path` in `locale`: [`localize`] without the domain. `bare`: no prefix.
fn path_in(list: &[&str], path: &str, locale: &str, bare: bool) -> String {
    let all = strip(list, path);
    match (bare, all.as_bytes().get(1)) {
        (true, _) => all,
        (false, None | Some(b'?' | b'#')) => format!("/{locale}{}", &all[1..]),
        (false, _) => format!("/{locale}{all}"),
    }
}

/// The locale of the request, by index into [`locales`]: the route's
/// `lang` parameter, else its host's `domain`, else the `lang` cookie,
/// else the best of its `Accept-Language`, else the default. With `prefix
/// as-needed` a page without a prefix is the default's, cookie and header
/// not asked.
pub(crate) fn pick(cx: &Cx) -> u8 {
    let list = locales();
    if list.is_empty() {
        return 0;
    }
    let find = |s: &str| list.iter().position(|l| l.eq_ignore_ascii_case(s));
    if let Some(i) = cx.route_param("lang").and_then(find) {
        return i as u8;
    }
    if let Some(i) = cx.host().and_then(by_host) {
        return i;
    }
    let default = DEFAULT.load(Ordering::Relaxed);
    if prefix() == Prefix::AsNeeded {
        return default;
    }
    let named = (cx.cookie("lang").and_then(find))
        .or_else(|| cx.header("accept-language").and_then(|h| best(h, list)));
    named.map_or(default, |i| i as u8)
}

/// For a page under `[[lang=locale]]`: the redirect `prefix always` and
/// `as-needed` ask for, if this request needs one. None for `optional`,
/// for an app with `domain`s, and for anything but GET and HEAD.
pub(crate) fn redirect(cx: &Cx) -> crate::Result {
    let p = prefix();
    let get = matches!(cx.method, crate::Method::Get | crate::Method::Head);
    if p == Prefix::Optional || !get || !domains().is_empty() {
        return Ok(());
    }
    let list = locales();
    let given = cx.route_param("lang").filter(|l| !l.is_empty());
    let picked = list.get(pick(cx) as usize).copied().unwrap_or("");
    let Some((to, status)) = redirect_for(p, list, default_name(), given, picked, cx.path()) else {
        return Ok(());
    };
    let query = cx.query_string();
    let to = match query.is_empty() {
        true => to,
        false => format!("{to}?{query}"),
    };
    Err(crate::Error::redirect(status, to))
}

/// Where a request for `path` goes under `p`, and with what status:
/// `given` is its `[[lang=locale]]`, `picked` the locale it gets. A 307
/// is not kept by a cache, which suits an answer that depends on who asks.
fn redirect_for(
    p: Prefix,
    list: &[&str],
    default: &str,
    given: Option<&str>,
    picked: &str,
    path: &str,
) -> Option<(String, u16)> {
    match (p, given) {
        (Prefix::Always, None) => Some((path_in(list, path, picked, false), 307)),
        (Prefix::AsNeeded, Some(l)) if l.eq_ignore_ascii_case(default) => {
            Some((strip(list, path), 308))
        }
        _ => None,
    }
}

/// `ltr` or `rtl`: the direction `locale` is written in, for `<html dir>`
/// (the build sets it there) or a `dir` attribute of your own:
/// `<p dir={wisp::dir(cx.locale())}>`.
pub fn dir(locale: &str) -> &'static str {
    match wisp_shared::dir::is_rtl(locale) {
        true => "rtl",
        false => "ltr",
    }
}

/// `https://example.fr/app` for `locale`, from `base` (`https://example.com/app`):
/// its domain's host in place of the base's, if `i18n` gave it one.
pub(crate) fn site_for(base: &str, locale: &str) -> String {
    site_in(domain_of(locale), base)
}

/// `base` with `host` (if any) as its host.
fn site_in(host: Option<&str>, base: &str) -> String {
    let Some(host) = host else {
        return base.to_string();
    };
    let at = base.find("://").map_or(0, |i| i + 3);
    let end = base[at..].find('/').map_or(base.len(), |i| at + i);
    format!("{}{host}{}", &base[..at], &base[end..])
}

/// `<link>` tags for a page's head, which tell a crawler what the page is
/// and what its other languages are: its `canonical` address, an
/// `alternate` with `hreflang` per locale and `x-default` (the default
/// locale's). On a page that is not under `[[lang=locale]]`, the canonical
/// one alone. Addresses start with `SITE_URL`, else the request's host;
/// nothing without either. `{@html wisp::alternates(cx)}`; the sitemap
/// lists each page in each locale, too.
pub fn alternates(cx: &Cx) -> String {
    let Some(base) = crate::seo::base(cx) else {
        return String::new();
    };
    let list = locales();
    let mut out = String::new();
    let mut tag = |rel: &str, lang: Option<&str>, href: &str| {
        out.push_str(&format!("<link rel=\"{rel}\""));
        if let Some(l) = lang {
            out.push_str(&format!(" hreflang=\"{}\"", l.replace('_', "-")));
        }
        out.push_str(" href=\"");
        crate::html::text(&mut out, href);
        out.push_str("\">\n");
    };
    if cx.route_param("lang").is_none() || list.is_empty() {
        tag("canonical", None, &format!("{base}{}", cx.path()));
        return out;
    }
    let href = |l: &str| {
        let at = path_in(list, cx.path(), l, bare(l));
        format!("{}{at}", site_for(&base, l))
    };
    tag("canonical", None, &href(cx.locale()));
    for l in list {
        tag("alternate", Some(l), &href(l));
    }
    tag("alternate", Some("x-default"), &href(default_name()));
    out
}

/// A language switcher: a `<nav class="wisp-locales" aria-label="Language">`
/// of links to this page in each locale, named in their own language
/// (`Français`), the current one `aria-current`. Empty with fewer than two
/// locales. `{@html wisp::switcher(cx)}`; for another look, loop over
/// [`locales`] with [`localize`] and [`native_name`].
pub fn switcher(cx: &Cx) -> String {
    let list = locales();
    if list.len() < 2 {
        return String::new();
    }
    let me = cx.locale();
    let mut out = String::from("<nav class=\"wisp-locales\" aria-label=\"Language\">");
    for l in list {
        let to = localize(cx.path(), l);
        let to = match to.starts_with("//") {
            true => to,
            false => crate::protocol::based(&to).into_owned(),
        };
        out.push_str("<a href=\"");
        crate::html::text(&mut out, &to);
        let tag = l.replace('_', "-");
        out.push_str(&format!("\" lang=\"{tag}\" hreflang=\"{tag}\""));
        if *l == me {
            out.push_str(" aria-current=\"true\"");
        }
        out.push('>');
        crate::html::text(&mut out, &native_name(l));
        out.push_str("</a>");
    }
    out.push_str("</nav>");
    out
}

/// A locale's name in its own language: `Français` for `fr`, `Português
/// (BR)` for `pt-BR`; the code itself for one Wisp has no name for.
pub fn native_name(locale: &str) -> String {
    let mut parts = locale.split(['-', '_']);
    let lang = parts.next().unwrap_or("").to_ascii_lowercase();
    let name = match lang.as_str() {
        "en" => "English",
        "fr" => "Français",
        "de" => "Deutsch",
        "es" => "Español",
        "it" => "Italiano",
        "pt" => "Português",
        "nl" => "Nederlands",
        "sv" => "Svenska",
        "da" => "Dansk",
        "nb" | "no" => "Norsk",
        "fi" => "Suomi",
        "pl" => "Polski",
        "cs" => "Čeština",
        "tr" => "Türkçe",
        "ru" => "Русский",
        "uk" => "Українська",
        "el" => "Ελληνικά",
        "ar" => "العربية",
        "he" => "עברית",
        "fa" => "فارسی",
        "hi" => "हिन्दी",
        "th" => "ไทย",
        "vi" => "Tiếng Việt",
        "id" => "Bahasa Indonesia",
        "ja" => "日本語",
        "ko" => "한국어",
        "zh" => "中文",
        _ => return locale.to_string(),
    };
    match parts.next() {
        Some(region) => format!("{name} ({})", region.to_ascii_uppercase()),
        None => name.to_string(),
    }
}

/// What a locale's page has in `[[lang=locale]]`: its name, or nothing for
/// the one that has no prefix (see [`Prefix::AsNeeded`]). For the sitemap
/// and the static export.
pub(crate) fn segment(locale: &str) -> &str {
    match bare(locale) {
        true => "",
        false => locale,
    }
}

/// The values of `[[lang=locale]]` for the pages `wisp build --static`
/// writes: each locale's, and the unprefixed too unless `prefix always`.
pub(crate) fn variants() -> Vec<&'static str> {
    let list = locales();
    if list.is_empty() {
        return vec![""];
    }
    let mut out: Vec<&str> = list.iter().map(|l| segment(l)).collect();
    if prefix() == Prefix::Optional {
        out.insert(0, "");
    }
    out.dedup();
    out
}

/// The locale of `list` an `Accept-Language` value likes most: by `q`,
/// then by order; `fr-CA` takes `fr`, and `fr` takes `fr-FR`.
fn best(header: &str, list: &[&str]) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for item in header.split(',') {
        let mut it = item.split(';');
        let tag = it.next().unwrap_or("").trim();
        let q = it
            .find_map(|p| {
                let p = p.trim();
                // As bytes: the name is ASCII, the rest of the item need not be.
                (p.len() > 2 && p.as_bytes()[..2].eq_ignore_ascii_case(b"q=")).then(|| &p[2..])
            })
            .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
        // `NaN`, `inf` or above 1 is no weight a client may send.
        let q = if (0.0..=1.0).contains(&q) { q } else { 0.0 };
        if q <= 0.0 || tag.is_empty() || tag == "*" || best.is_some_and(|(b, _)| b >= q) {
            continue;
        }
        // As bytes: a tag of the client's need not split where a locale does.
        // `_` and `-` are one: a file may be named `pt_BR.json`.
        let eq = |a: &[u8], b: &[u8]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    x.eq_ignore_ascii_case(y)
                        || (matches!(x, b'-' | b'_') && matches!(y, b'-' | b'_'))
                })
        };
        let base = |a: &str, b: &str| {
            let (a, b) = (a.as_bytes(), b.as_bytes());
            a.len() > b.len() && eq(&a[..b.len()], b) && matches!(a[b.len()], b'-' | b'_')
        };
        let found = (list.iter().position(|l| eq(l.as_bytes(), tag.as_bytes())))
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
        let list = ["en", "pt_BR", "zh_Hant"];
        let u = |h: &str| best(h, &list).map(|i| list[i]);
        assert_eq!(u("pt-BR"), Some("pt_BR"), "a file named pt_BR.json");
        assert_eq!(u("zh-Hant-TW"), Some("zh_Hant"));
        assert_eq!(u("pt"), Some("pt_BR"));
        assert_eq!(b("de, *;q=0.1"), None);
        assert_eq!(b("fr;q=0, en"), Some("en"));
        assert_eq!(b(""), None);
        assert_eq!(b("e\u{e9}-x"), None, "a split inside a character, no panic");
        assert_eq!(b("fr;q=NaN, en;q=0.5"), Some("en"));
        assert_eq!(b("fr;q=9, en;q=0.5"), Some("en"));
        assert_eq!(
            b("fr;Q=0, en"),
            Some("en"),
            "the parameter name is not case-sensitive"
        );
        assert_eq!(b("en;q=0.5, fr;Q=0.9"), Some("fr"));
    }

    #[test]
    fn localizes_paths() {
        let l = |p: &str, to: &str| path_in(&["en", "fr"], p, to, false);
        let bare = |p: &str| path_in(&["en", "fr"], p, "en", true);
        assert_eq!(l("/fr/about?x=1", "en"), "/en/about?x=1");
        assert_eq!(l("/about", "fr"), "/fr/about");
        assert_eq!(l("/fr", "en"), "/en");
        assert_eq!(l("/", "fr"), "/fr");
        assert_eq!(l("/?q=1", "fr"), "/fr?q=1");
        assert_eq!(l("/french/x", "en"), "/en/french/x");
        // No prefix, and never a path that is another host's.
        assert_eq!(bare("/en/about?x=1"), "/about?x=1");
        assert_eq!(bare("/en"), "/");
        assert_eq!(bare("/en//evil.com"), "/evil.com");
        assert_eq!(bare("/\\evil.com"), "/evil.com");
        let go = |p, given, picked, path| redirect_for(p, &["en", "fr"], "en", given, picked, path);
        assert_eq!(
            go(Prefix::Always, None, "fr", "/about"),
            Some(("/fr/about".into(), 307))
        );
        assert_eq!(
            go(Prefix::Always, None, "en", "/"),
            Some(("/en".into(), 307))
        );
        assert_eq!(go(Prefix::Always, Some("fr"), "fr", "/fr/about"), None);
        assert_eq!(
            go(Prefix::AsNeeded, Some("en"), "en", "/en/about"),
            Some(("/about".into(), 308))
        );
        assert_eq!(go(Prefix::AsNeeded, Some("fr"), "fr", "/fr/about"), None);
        assert_eq!(go(Prefix::AsNeeded, None, "en", "/about"), None);
        assert_eq!(go(Prefix::Optional, Some("en"), "en", "/en/about"), None);
        let d = [("example.fr", 1), ("localhost:3000", 0)];
        assert_eq!(host_in(&d, "EXAMPLE.fr:8080"), Some(1));
        assert_eq!(host_in(&d, "localhost:3000"), Some(0));
        assert_eq!(host_in(&d, "example.com"), None);
        assert_eq!(
            site_in(Some("example.fr"), "https://example.com/app"),
            "https://example.fr/app"
        );
        assert_eq!(
            site_in(Some("example.fr"), "http://a:1"),
            "http://example.fr"
        );
        assert_eq!(site_in(None, "https://x.org"), "https://x.org");
        assert_eq!(dir("ar"), "rtl");
        assert_eq!(dir("pt-BR"), "ltr");
        assert_eq!(native_name("pt-br"), "Português (BR)");
        assert_eq!(native_name("xx"), "xx");
    }

    #[test]
    fn finds_the_shells_lang() {
        let s = "<!doctype html>\n<html lang=\"en\">\n<head>";
        assert_eq!(lang_value(s).map(|(a, b)| &s[a..b]), Some("en"));
        assert_eq!(lang_value("<html><body lang=\"x\">"), None);
        assert_eq!(lang_value("<p>"), None);
    }
}
