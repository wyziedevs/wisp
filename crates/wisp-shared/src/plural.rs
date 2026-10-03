//! CLDR plural rules for whole numbers, for the languages most apps ship:
//! which case of `{n, plural, one {…} other {…}}` a count takes. The build
//! checks a message's cases against its locale's rule by it, and the
//! runtime picks one by it.

/// The cases, by number: a rule's category is an index into this.
pub const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

const ZERO: u8 = 0;
const ONE: u8 = 1;
const TWO: u8 = 2;
const FEW: u8 = 3;
const MANY: u8 = 4;
const OTHER: u8 = 5;

/// The rules, by number: the languages each covers (the part of a locale
/// before `-` or `_`, `pt-PT` apart) and the cases it has.
const RULES: [(&[&str], &[u8]); 14] = [
    // 0: no plural.
    (
        &[
            "ja", "zh", "ko", "vi", "th", "id", "ms", "lo", "my", "km", "yo",
        ],
        &[OTHER],
    ),
    // 1: one = 1.
    (
        &[
            "en", "de", "nl", "sv", "da", "nb", "nn", "no", "fi", "et", "el", "hu", "tr", "bg",
            "gl", "eu", "af", "sw", "ur", "sq", "az", "ka", "kk", "ky", "mn", "uz", "ml", "ta",
            "te", "ne", "mr", "is", "fy", "lb",
        ],
        &[ONE, OTHER],
    ),
    // 2: one = 1; many = a non-zero multiple of a million.
    (&["es", "it", "ca", "pt-pt"], &[ONE, MANY, OTHER]),
    // 3: one = 0 or 1; many as 2.
    (&["fr", "pt"], &[ONE, MANY, OTHER]),
    // 4: one = 0 or 1.
    (&["hi", "bn", "fa", "gu", "kn", "zu", "am"], &[ONE, OTHER]),
    // 5: one = …1 but …11; few = …2-4 but …12-14; many = the rest.
    (&["ru", "uk", "be"], &[ONE, FEW, MANY, OTHER]),
    // 6: one = 1; few as 5; many = the rest.
    (&["pl"], &[ONE, FEW, MANY, OTHER]),
    // 7: one = 1; few = 2-4.
    (&["cs", "sk"], &[ONE, FEW, MANY, OTHER]),
    // 8: zero, one, two; few = …03-10; many = …11-99.
    (&["ar"], &[ZERO, ONE, TWO, FEW, MANY, OTHER]),
    // 9: one = 1; two = 2.
    (&["he"], &[ONE, TWO, OTHER]),
    // 10: one = …1 but …11; few = …2-4 but …12-14.
    (&["hr", "sr", "bs"], &[ONE, FEW, OTHER]),
    // 11: one = 1; few = 0 or …02-19.
    (&["ro"], &[ONE, FEW, OTHER]),
    // 12: one = …1 but …11-19; few = …2-9 but …12-19.
    (&["lt"], &[ONE, FEW, MANY, OTHER]),
    // 13: zero = …0 or …11-19; one = …1 but …11.
    (&["lv"], &[ZERO, ONE, OTHER]),
];

/// The rule of `locale` (`en`, `pt-BR`, `zh_Hant`), if this table has its
/// language.
pub fn rule(locale: &str) -> Option<u8> {
    let lower = locale.to_ascii_lowercase().replace('_', "-");
    let lang = lower.split('-').next().unwrap_or("");
    let full = (lower == "pt-pt").then_some("pt-pt");
    let k = RULES
        .iter()
        .position(|(langs, _)| langs.contains(&full.unwrap_or(lang)))?;
    Some(k as u8)
}

/// The cases rule `r` has, as indexes into [`CATEGORIES`].
pub fn categories(r: u8) -> &'static [u8] {
    RULES.get(r as usize).map_or(&[OTHER], |(_, c)| c)
}

/// The case whole number `n` takes under rule `r` (negative numbers take
/// their size's), as an index into [`CATEGORIES`].
pub fn category(r: u8, n: u64) -> u8 {
    let (m10, m100) = (n % 10, n % 100);
    let million = n != 0 && n.is_multiple_of(1_000_000);
    match r {
        1 | 2 | 6 | 7 | 8 | 9 | 11 if n == 1 => ONE,
        2 | 3 if million => MANY,
        3 | 4 if n <= 1 => ONE,
        5 | 10 if m10 == 1 && m100 != 11 => ONE,
        5 | 6 | 10 if (2..=4).contains(&m10) && !(12..=14).contains(&m100) => FEW,
        5 | 6 => MANY,
        7 if (2..=4).contains(&n) => FEW,
        8 if n == 0 => ZERO,
        8 | 9 if n == 2 => TWO,
        8 if (3..=10).contains(&m100) => FEW,
        8 if m100 >= 11 => MANY,
        11 if n == 0 || (2..=19).contains(&m100) => FEW,
        12 if m10 == 1 && !(11..=19).contains(&m100) => ONE,
        12 if m10 >= 2 && !(11..=19).contains(&m100) => FEW,
        13 if m10 == 0 || (11..=19).contains(&m100) => ZERO,
        13 if m10 == 1 => ONE,
        _ => OTHER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(locale: &str, n: u64) -> &'static str {
        CATEGORIES[category(rule(locale).unwrap(), n) as usize]
    }

    #[test]
    fn common_languages() {
        let table: [(&str, &[(u64, &str)]); 9] = [
            ("en", &[(0, "other"), (1, "one"), (2, "other")]),
            (
                "fr",
                &[(0, "one"), (1, "one"), (2, "other"), (2_000_000, "many")],
            ),
            ("pt-BR", &[(0, "one"), (1, "one")]),
            ("pt_PT", &[(0, "other"), (1, "one"), (1_000_000, "many")]),
            (
                "ru",
                &[
                    (1, "one"),
                    (21, "one"),
                    (11, "many"),
                    (3, "few"),
                    (13, "many"),
                    (5, "many"),
                ],
            ),
            ("pl", &[(1, "one"), (21, "many"), (22, "few"), (12, "many")]),
            ("cs", &[(1, "one"), (3, "few"), (5, "other")]),
            (
                "ar",
                &[
                    (0, "zero"),
                    (1, "one"),
                    (2, "two"),
                    (5, "few"),
                    (11, "many"),
                    (100, "other"),
                ],
            ),
            ("ja", &[(1, "other")]),
        ];
        for (locale, cases) in table {
            for &(n, want) in cases {
                assert_eq!(case(locale, n), want, "{locale} {n}");
            }
        }
        assert_eq!(rule("xx"), None);
        // Every case a rule picks is one it lists.
        for r in 0..RULES.len() as u8 {
            for n in 0..2_000 {
                assert!(categories(r).contains(&category(r, n)), "rule {r}, {n}");
            }
        }
    }
}
