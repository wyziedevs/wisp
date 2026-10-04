//! Numbers, money and dates as a locale writes them, for `cx.locale()`:
//! `{wisp::format_number(total, cx.locale())}`. A few dozen lines of tables
//! in place of ICU: the languages Wisp has plural rules for get their
//! separators and date order, and a locale it has not seen is written the
//! way `en` or ISO 8601 does. Nothing here runs unless a page calls it.

/// A date: what [`format_date`] takes. Seconds since the Unix epoch
/// (`i64`, UTC), a `"2026-10-04"` (or `"2026-10-04T12:00:00Z"`) string,
/// or `(year, month, day)`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a date: use Unix seconds (`i64`), \"2026-10-04\" or (2026, 10, 4)",
    label = "the date of `format_date`"
)]
pub trait AsDate {
    /// The year, month (1-12) and day (1-31), or `None` if it is none.
    fn ymd(&self) -> Option<(i32, u32, u32)>;
}

impl AsDate for i64 {
    fn ymd(&self) -> Option<(i32, u32, u32)> {
        // Days since 1970-01-01 to the civil date (Hinnant's algorithm).
        let z = self.div_euclid(86_400).checked_add(719_468)?;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = i32::try_from(yoe + era * 400 + i64::from(m <= 2)).ok()?;
        Some((y, m, d))
    }
}

impl AsDate for str {
    fn ymd(&self) -> Option<(i32, u32, u32)> {
        let mut it = self.get(..10)?.split('-');
        let y = it.next()?.parse::<i32>().ok()?;
        let m = it.next()?.parse::<u32>().ok()?;
        let d = it.next()?.parse::<u32>().ok()?;
        (y, m, d).ymd()
    }
}

impl AsDate for (i32, u32, u32) {
    fn ymd(&self) -> Option<(i32, u32, u32)> {
        let (y, m, d) = *self;
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let days = match m {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => return None,
        };
        ((1..=days).contains(&d) && (0..=9999).contains(&y)).then_some((y, m, d))
    }
}

impl<T: AsDate + ?Sized> AsDate for &T {
    fn ymd(&self) -> Option<(i32, u32, u32)> {
        (**self).ymd()
    }
}

/// The language of a locale (`pt` of `pt-BR`), lower case, and its region
/// (`BR`), upper case, or `""`.
fn split(locale: &str) -> (String, String) {
    let mut it = locale.split(['-', '_']);
    let lang = it.next().unwrap_or("").to_ascii_lowercase();
    let region = it.find(|r| r.len() == 2).unwrap_or("");
    (lang, region.to_ascii_uppercase())
}

/// A locale's digit group separator, decimal mark, and how many digits
/// the integer part needs before it is grouped at all.
fn separators(lang: &str, region: &str) -> (&'static str, &'static str, usize) {
    match lang {
        "fr" => ("\u{202f}", ",", 4),
        "de" if region == "CH" => ("’", ".", 4),
        "de" | "it" | "nl" | "id" | "tr" | "da" | "el" | "vi" | "ro" | "hr" | "sl" | "ca"
        | "is" => (".", ",", 4),
        "pt" if region == "PT" => ("\u{a0}", ",", 5),
        "pt" => (".", ",", 4),
        "es" => (".", ",", 5),
        "pl" => ("\u{a0}", ",", 5),
        "ru" | "uk" | "cs" | "sk" | "sv" | "nb" | "no" | "fi" | "bg" | "hu" | "lt" | "lv"
        | "et" => ("\u{a0}", ",", 4),
        _ => (",", ".", 4),
    }
}

/// `n` as the locale writes it, with `frac` fraction digits at most (and
/// no trailing zeros unless `fixed`).
fn render(n: f64, frac: usize, fixed: bool, locale: &str) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-∞" } else { "∞" }.into();
    }
    let (lang, region) = split(locale);
    let (group, point, from) = separators(&lang, &region);
    let s = format!("{:.frac$}", n.abs());
    let (int, mut fr) = s.split_once('.').unwrap_or((&s, ""));
    if !fixed {
        fr = fr.trim_end_matches('0');
    }
    let mut out = String::new();
    // Only a number that is not zero in what is written is negative.
    if n < 0.0 && (int.bytes().chain(fr.bytes()).any(|b| b != b'0')) {
        out.push('-');
    }
    let grouped = int.len() >= from;
    for (i, c) in int.chars().enumerate() {
        if grouped && i > 0 && (int.len() - i) % 3 == 0 {
            out.push_str(group);
        }
        out.push(c);
    }
    if !fr.is_empty() {
        out.push_str(point);
        out.push_str(fr);
    }
    out
}

/// `n` as `locale` writes a number, to three decimals at most:
/// `format_number(1234567.891, "en")` is `1,234,567.891`, in `fr`
/// `1 234 567,891` (narrow no-break spaces), in `de` `1.234.567,891`.
/// `{wisp::format_number(total, cx.locale())}`. An integer wider than `i32`
/// goes in as `n as f64`.
pub fn format_number(n: impl Into<f64>, locale: &str) -> String {
    render(n.into(), 3, false, locale)
}

/// `n` as an amount of `currency` (an ISO code, `EUR`) in `locale`'s
/// style: `format_money(1234.5, "USD", "en")` is `$1,234.50`, and
/// `format_money(1234.5, "EUR", "de")` is `1.234,50 €`. Two decimals, none for `JPY`, `KRW` and the other currencies without
/// minor units; a currency without a symbol here is written by its code.
pub fn format_money(n: impl Into<f64>, currency: &str, locale: &str) -> String {
    let n = n.into();
    let code = currency.to_ascii_uppercase();
    let digits = match code.as_str() {
        "JPY" | "KRW" | "VND" | "CLP" | "ISK" => 0,
        _ => 2,
    };
    let symbol = match code.as_str() {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "INR" => "₹",
        "KRW" => "₩",
        "BRL" => "R$",
        "CAD" => "CA$",
        "AUD" => "A$",
        other => other,
    };
    let (lang, _) = split(locale);
    let amount = render(n.abs(), digits, true, locale);
    let sign = if render(n, digits, true, "en").starts_with('-') {
        "-"
    } else {
        ""
    };
    match lang.as_str() {
        "fr" | "de" | "es" | "it" | "ru" | "uk" | "cs" | "sk" | "pl" | "sv" | "nb" | "no"
        | "da" | "fi" | "el" | "vi" | "hu" | "ro" | "bg" | "lt" | "lv" | "et" | "ca" | "hr"
        | "sl" => format!("{sign}{amount}\u{a0}{symbol}"),
        "nl" | "pt" => format!("{symbol}\u{a0}{sign}{amount}"),
        _ => format!("{sign}{symbol}{amount}"),
    }
}

/// How a locale writes a date in digits: the order of day, month and year
/// (`d`, `m`, `y`), what goes between them, whether day and month have two
/// digits, and what ends it.
struct Numeric {
    order: &'static str,
    sep: &'static str,
    pad: bool,
    end: &'static str,
}

fn numeric(lang: &str, region: &str) -> Numeric {
    let n = |order, sep, pad, end| Numeric {
        order,
        sep,
        pad,
        end,
    };
    match lang {
        "en" if region.is_empty() || region == "US" => n("mdy", "/", false, ""),
        "en" | "fr" | "pt" | "ar" | "el" | "hi" | "id" | "vi" | "th" => n("dmy", "/", true, ""),
        "es" | "it" => n("dmy", "/", false, ""),
        "de" | "da" | "nb" | "no" | "fi" | "pl" | "he" => n("dmy", ".", false, ""),
        "ru" | "uk" | "tr" | "ro" | "bg" => n("dmy", ".", true, ""),
        "cs" | "sk" => n("dmy", ". ", false, ""),
        "nl" => n("dmy", "-", false, ""),
        "sv" | "lt" => n("ymd", "-", true, ""),
        "ja" | "zh" => n("ymd", "/", false, ""),
        "ko" => n("ymd", ". ", false, "."),
        _ => n("ymd", "-", true, ""),
    }
}

/// `date` in digits as `locale` writes it: `format_date(1759536000,
/// "en")` and `format_date("2026-10-04", "en")` are `10/4/2026`, in `fr`
/// `04/10/2026`, in `de` `4.10.2026`, in `ja` `2026/10/4`, and a locale
/// not known here gets `2026-10-04`. UTC. An invalid date is `""`.
pub fn format_date(date: impl AsDate, locale: &str) -> String {
    let Some((y, m, d)) = date.ymd() else {
        return String::new();
    };
    let (lang, region) = split(locale);
    let p = numeric(&lang, &region);
    let two = |v: u32| match p.pad {
        true => format!("{v:02}"),
        false => v.to_string(),
    };
    let parts: Vec<String> = (p.order.chars())
        .map(|c| match c {
            'd' => two(d),
            'm' => two(m),
            _ => format!("{y:04}"),
        })
        .collect();
    format!("{}{}", parts.join(p.sep), p.end)
}

const MONTHS: [(&str, [&str; 12]); 7] = [
    (
        "en",
        [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ],
    ),
    (
        "fr",
        [
            "janvier",
            "février",
            "mars",
            "avril",
            "mai",
            "juin",
            "juillet",
            "août",
            "septembre",
            "octobre",
            "novembre",
            "décembre",
        ],
    ),
    (
        "de",
        [
            "Januar",
            "Februar",
            "März",
            "April",
            "Mai",
            "Juni",
            "Juli",
            "August",
            "September",
            "Oktober",
            "November",
            "Dezember",
        ],
    ),
    (
        "es",
        [
            "enero",
            "febrero",
            "marzo",
            "abril",
            "mayo",
            "junio",
            "julio",
            "agosto",
            "septiembre",
            "octubre",
            "noviembre",
            "diciembre",
        ],
    ),
    (
        "it",
        [
            "gennaio",
            "febbraio",
            "marzo",
            "aprile",
            "maggio",
            "giugno",
            "luglio",
            "agosto",
            "settembre",
            "ottobre",
            "novembre",
            "dicembre",
        ],
    ),
    (
        "pt",
        [
            "janeiro",
            "fevereiro",
            "março",
            "abril",
            "maio",
            "junho",
            "julho",
            "agosto",
            "setembro",
            "outubro",
            "novembro",
            "dezembro",
        ],
    ),
    (
        "nl",
        [
            "januari",
            "februari",
            "maart",
            "april",
            "mei",
            "juni",
            "juli",
            "augustus",
            "september",
            "oktober",
            "november",
            "december",
        ],
    ),
];

/// `date` with its month in words as `locale` writes it:
/// `format_date_long("2026-10-04", "en")` is `October 4, 2026`, in `en-GB`
/// `4 October 2026`, in `fr` `4 octobre 2026`, in `de` `4. Oktober 2026`,
/// in `ja` `2026年10月4日`. A locale whose month names are not here (`en`,
/// `fr`, `de`, `es`, `it`, `pt`, `nl` are; `ja`, `zh` and `ko` need none)
/// gets [`format_date`]'s digits. UTC. An invalid date is `""`.
pub fn format_date_long(date: impl AsDate, locale: &str) -> String {
    let Some((y, m, d)) = date.ymd() else {
        return String::new();
    };
    let (lang, region) = split(locale);
    match lang.as_str() {
        "ja" | "zh" => return format!("{y}年{m}月{d}日"),
        "ko" => return format!("{y}년 {m}월 {d}일"),
        _ => {}
    }
    let Some((_, names)) = MONTHS.iter().find(|(l, _)| *l == lang) else {
        return format_date(date, locale);
    };
    let month = names[m as usize - 1];
    match lang.as_str() {
        "en" if region.is_empty() || region == "US" => format!("{month} {d}, {y}"),
        "fr" if d == 1 => format!("1er {month} {y}"),
        "de" | "da" => format!("{d}. {month} {y}"),
        "es" | "pt" => format!("{d} de {month} de {y}"),
        _ => format!("{d} {month} {y}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(format_number(1234567.891, "en"), "1,234,567.891");
        assert_eq!(
            format_number(1234567.891, "fr"),
            "1\u{202f}234\u{202f}567,891"
        );
        assert_eq!(format_number(1234567.891, "de-DE"), "1.234.567,891");
        assert_eq!(format_number(1234, "es"), "1234");
        assert_eq!(format_number(12345, "es"), "12.345");
        assert_eq!(format_number(1234, "en"), "1,234");
        assert_eq!(format_number(999, "en"), "999");
        assert_eq!(format_number(-0.5, "en"), "-0.5");
        assert_eq!(format_number(-0.0001, "en"), "0");
        assert_eq!(format_number(2.0, "ru"), "2");
        assert_eq!(format_number(1.0 / 0.0, "en"), "∞");
        assert_eq!(format_number(f64::NAN, "en"), "NaN");
        assert_eq!(format_number(1e15, "xx"), "1,000,000,000,000,000");
    }

    #[test]
    fn money() {
        assert_eq!(format_money(1234.5, "USD", "en"), "$1,234.50");
        assert_eq!(format_money(-3, "eur", "en-GB"), "-€3.00");
        assert_eq!(format_money(1234.5, "EUR", "de"), "1.234,50\u{a0}€");
        assert_eq!(format_money(1234.5, "EUR", "fr"), "1\u{202f}234,50\u{a0}€");
        assert_eq!(format_money(-1234.5, "EUR", "nl"), "€\u{a0}-1.234,50");
        assert_eq!(format_money(500, "JPY", "ja"), "¥500");
        assert_eq!(format_money(5, "CHF", "en"), "CHF5.00");
    }

    #[test]
    fn dates() {
        let secs = 1_759_579_200i64; // 2025-10-04T12:00:00Z
        assert_eq!(secs.ymd(), Some((2025, 10, 4)));
        assert_eq!(0i64.ymd(), Some((1970, 1, 1)));
        assert_eq!((-86_400i64).ymd(), Some((1969, 12, 31)));
        assert_eq!(951_782_400i64.ymd(), Some((2000, 2, 29)));
        assert_eq!(format_date("2026-10-04T12:00:00Z", "en"), "10/4/2026");
        assert_eq!(format_date("2026-10-04", "en-GB"), "04/10/2026");
        assert_eq!(format_date((2026, 10, 4), "fr"), "04/10/2026");
        assert_eq!(format_date((2026, 10, 4), "de"), "4.10.2026");
        assert_eq!(format_date(secs, "ja"), "2025/10/4");
        assert_eq!(format_date("2026-10-04", "ko"), "2026. 10. 4.");
        assert_eq!(format_date("2026-10-04", "xx"), "2026-10-04");
        assert_eq!(format_date("2026-02-30", "en"), "");
        assert_eq!(format_date("soon", "en"), "");
        assert_eq!(format_date((2026, 13, 1), "en"), "");
        assert_eq!(format_date_long("2026-10-04", "en"), "October 4, 2026");
        assert_eq!(format_date_long("2026-10-04", "en-GB"), "4 October 2026");
        assert_eq!(format_date_long("2026-10-01", "fr"), "1er octobre 2026");
        assert_eq!(format_date_long("2026-10-04", "de"), "4. Oktober 2026");
        assert_eq!(format_date_long("2026-03-04", "es"), "4 de marzo de 2026");
        assert_eq!(format_date_long("2026-10-04", "ja"), "2026年10月4日");
        assert_eq!(format_date_long("2026-10-04", "ru"), "04.10.2026");
    }
}
