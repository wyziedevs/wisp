//! `#[validate(…)]`'s rules, in one place: `wisp-build` reads them off an
//! action's parameter (to check the input before the call), off a field
//! (for the limits in the OpenAPI document) and off both for the
//! attributes a form's field gets (the browser checks before it sends);
//! `#[derive(FromJson)]` (`wisp-macros`) reads the same rules, with the
//! same messages, from its tokens. The checks themselves are
//! `wisp::json::check`'s. Each rule is one entry of `DEFS`.

/// One rule, with its value as written (`""` for `email`; a range with no
/// spaces around its `..`, as `1..=100`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rule {
    pub key: Key,
    pub value: String,
}

/// A rule of a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Min,
    Max,
    MinLen,
    MaxLen,
    /// A range: `1..=100`, `..10`, `3..`.
    Len,
    Email,
    /// An absolute http(s) address.
    Url,
    /// One of the words of a quoted, space-separated list.
    OneOf,
    /// The whole text matches a pattern (`wisp::json::check::pattern`).
    Pattern,
    /// A function of the app's: `with = ok_name`, `fn(&T) -> Option<String>`.
    With,
}

/// What a rule takes after its name.
enum Takes {
    Nothing,
    Value,
    Range,
}

/// What a form's field is, as far as a browser's checks go.
#[derive(Clone, Copy)]
struct Kind {
    text: bool,
    number: bool,
}

/// A rule whole: its name, what it takes, the check the server runs and
/// what a browser can check of it.
struct Def {
    key: Key,
    name: &'static str,
    takes: Takes,
    /// The call of `wisp::json::check` that checks the value `v` (a
    /// reference) by the rule's value `x`: `Some(problem)` when it does
    /// not pass.
    check: fn(v: &str, x: &str) -> String,
    /// What it tells a browser of a field of kind `k` by value `x`.
    native: fn(n: &mut Native, x: &str, k: Kind),
}

/// A `min` or `max` bound as Rust: an integer literal as an `i128`, so one
/// past `i32` compiles and integer fields compare to it exactly; a float or
/// a const as it is.
fn bound(x: &str) -> String {
    let t = x.trim();
    let digits = t.strip_prefix('-').unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        format!("{t}i128")
    } else {
        t.to_string()
    }
}

/// Every rule.
const DEFS: [Def; 10] = [
    Def {
        key: Key::Min,
        name: "min",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::min({v}, {})", bound(x)),
        native: |n, x, k| {
            if k.number {
                n.min = plain(x);
            }
        },
    },
    Def {
        key: Key::Max,
        name: "max",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::max({v}, {})", bound(x)),
        native: |n, x, k| {
            if k.number {
                n.max = plain(x);
            }
        },
    },
    Def {
        key: Key::MinLen,
        name: "min_len",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::min_len({v}, {x})"),
        native: |n, x, k| {
            if k.text {
                n.min_len = x.parse().ok();
            }
        },
    },
    Def {
        key: Key::MaxLen,
        name: "max_len",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::max_len({v}, {x})"),
        native: |n, x, k| {
            if k.text {
                n.max_len = x.parse().ok();
            }
        },
    },
    Def {
        key: Key::Len,
        name: "len",
        takes: Takes::Range,
        check: |v, x| format!("::wisp::rt_traits::len({v}, {x})"),
        native: |n, x, k| {
            if k.text {
                (n.min_len, n.max_len) = bounds(x);
            }
        },
    },
    Def {
        key: Key::Email,
        name: "email",
        takes: Takes::Nothing,
        check: |v, _| format!("::wisp::json::check::email({v})"),
        native: |n, _, k| n.email |= k.text,
    },
    Def {
        key: Key::Url,
        name: "url",
        takes: Takes::Nothing,
        check: |v, _| format!("::wisp::json::check::url({v})"),
        native: |_, _, _| {},
    },
    Def {
        key: Key::OneOf,
        name: "one_of",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::one_of({v}, {x})"),
        // A literal list is the options of the `<select>` `fields` writes.
        native: |n, x, k| {
            let list = x.trim().strip_prefix('"').and_then(|x| x.strip_suffix('"'));
            if let Some(list) = list.filter(|l| k.text && !l.contains(['\\', '{', '<', '&'])) {
                n.choices = list.split_whitespace().map(str::to_string).collect();
            }
        },
    },
    Def {
        key: Key::Pattern,
        name: "pattern",
        takes: Takes::Value,
        check: |v, x| {
            // Read once, at the first request: a static of this check's own.
            format!(
                "::wisp::json::check::pattern_once({v}, {{ static P: ::wisp::json::check::Pattern = ::wisp::json::check::Pattern::new(); &P }}, {x})"
            )
        },
        native: |_, _, _| {},
    },
    Def {
        key: Key::With,
        name: "with",
        takes: Takes::Value,
        check: |v, x| format!("({x})({v})"),
        native: |_, _, _| {},
    },
];

/// `max_size = 1 * MB`: not a rule of the value but the most bytes of an
/// upload, which the route's body limit makes room for.
const MAX_SIZE: &str = "max_size";

/// The fewest characters a `Password` is held to when no `min_len` or `len`
/// rule of its own says: `#[validate(min_len = 8)]` written for you.
pub const PASSWORD_MIN_LEN: u64 = 8;

/// Whether `rules` set a least length (`min_len`, or a `len` range).
pub fn sets_min_len(rules: &[Rule]) -> bool {
    rules
        .iter()
        .any(|r| matches!(r.key, Key::MinLen | Key::Len))
}

/// A type that is a `Password`, or an `Option` of one, as written.
pub fn is_password(ty: &str) -> bool {
    // As the scanner writes it, or as a token stream prints it (`Option < Password >`).
    let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
    let t = t
        .strip_prefix("Option<")
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(&t)
        .trim_start_matches('&');
    t.rsplit("::").next() == Some("Password")
}

/// A number as a browser's `min` or `max` (and JSON) takes it: plain and
/// finite.
pub fn plain(x: &str) -> Option<f64> {
    x.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// The least and most a range allows, when they are plain numbers:
/// `1..=100` → (1, 100), `..10` → (None, 9).
fn bounds(range: &str) -> (Option<u64>, Option<u64>) {
    let Some((lo, hi)) = range.split_once("..") else {
        return (None, None);
    };
    let hi = match hi.strip_prefix('=') {
        Some(h) => h.parse().ok(),
        None => hi.parse::<u64>().ok().map(|h| h.saturating_sub(1)),
    };
    (lo.parse().ok(), hi)
}

/// What is inside `validate(…)`: the rules of the value, and apart from
/// them an upload's `max_size` (the code of its bytes).
#[derive(Default, Debug)]
pub struct Validate {
    pub rules: Vec<Rule>,
    pub max_size: Option<String>,
}

impl Validate {
    /// The calls that check the value `v` (a reference): each rule's, then
    /// the upload's size.
    pub fn checks(&self, v: &str) -> Vec<String> {
        let mut out: Vec<String> = self.rules.iter().map(|r| r.check(v)).collect();
        if let Some(x) = &self.max_size {
            out.push(format!("::wisp::rt_traits::max_size({v}, ({x}) as usize)"));
        }
        out
    }
}

/// `rules` cut at the commas that are not inside a string: `pattern = "a{1,2}"`.
fn split(rules: &str) -> Vec<&str> {
    let (mut out, mut from, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in rules.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&rules[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(&rules[from..]);
    out
}

/// What is inside `validate(…)`, or what is wrong with it.
pub fn parse(rules: &str) -> Result<Validate, String> {
    let mut out = Validate::default();
    for r in split(rules)
        .into_iter()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        let (name, value) = match r.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (r, None),
        };
        if name == MAX_SIZE {
            let v = value.ok_or("`max_size` needs a value: `max_size = 1 * MB`")?;
            out.max_size = Some(v.to_string());
        } else {
            out.rules.push(rule(name, value)?);
        }
    }
    Ok(out)
}

/// The rule `name`, with its value as written if it has one, or what is
/// wrong with it.
pub fn rule(name: &str, value: Option<&str>) -> Result<Rule, String> {
    let Some(def) = DEFS.iter().find(|d| d.name == name) else {
        if name == MAX_SIZE {
            return Err("`max_size` is for an upload: an action's `Image` parameter".into());
        }
        return Err(format!(
            "#[validate] has no `{name}`: it takes len, min, max, min_len, max_len, email, url, one_of, pattern, with and max_size"
        ));
    };
    let value = match (&def.takes, value.map(str::trim)) {
        (Takes::Nothing, None) => String::new(),
        (Takes::Nothing, Some(_)) => return Err(format!("`{name}` takes no value")),
        (_, None) => return Err(format!("`{name}` needs a value: `{name} = 1`")),
        (Takes::Range, Some(v)) => {
            let x: String = v.split_whitespace().collect();
            if !x.contains("..") {
                return Err(format!(
                    "`{name} = {x}` needs a range, such as `{name} = 1..=100`"
                ));
            }
            if x.trim_matches(['.', '=']).is_empty() {
                return Err(format!(
                    "`{name} = ..` needs a bound, such as `{name} = 1..=100`"
                ));
            }
            // As wisp-macros has it, tokens with spaces between: `1 ..= 100`.
            let (lo, hi) = v.split_once("..").unwrap_or((v, ""));
            match hi.trim_start().strip_prefix('=') {
                Some(hi) => format!("{}..={}", lo.trim(), hi.trim()),
                None => format!("{}..{}", lo.trim(), hi.trim()),
            }
        }
        (Takes::Value, Some(v)) => v.to_string(),
    };
    // A pattern that cannot be read is the build's error, not a visitor's.
    if def.key == Key::Pattern
        && let Some(text) = literal(&value)
        && crate::pattern::parse(&text).is_none()
    {
        return Err(format!(
            "`pattern = {value}` cannot be read: it takes literals, `.`, classes, groups, `|` and `? * + {{n}} {{n,m}}`"
        ));
    }
    Ok(Rule {
        key: def.key,
        value,
    })
}

/// The text of a string literal as written in a rule (`"a"` or `r"a"`);
/// `None` for anything else, or an escape this does not know.
fn literal(v: &str) -> Option<String> {
    if let Some(raw) = v.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let inner = raw[hashes..].strip_prefix('"')?;
        let inner = inner.strip_suffix(&format!("\"{}", "#".repeat(hashes)))?;
        return Some(inner.to_string());
    }
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            '\\' => '\\',
            '"' => '"',
            '\'' => '\'',
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(out)
}

impl Rule {
    /// The call of `wisp::json::check` that checks the value `v` (a
    /// reference) by this rule: `Some(problem)` when it does not pass.
    pub fn check(&self, v: &str) -> String {
        (self.def().check)(v, &self.value)
    }

    fn def(&self) -> &'static Def {
        DEFS.iter()
            .find(|d| d.key == self.key)
            .expect("every key has its def")
    }

    /// What it tells a browser of a field that is `text` or a `number`.
    pub fn native(&self, n: &mut Native, text: bool, number: bool) {
        (self.def().native)(n, &self.value, Kind { text, number })
    }

    /// The least and most length `len = …` allows, when they are plain
    /// numbers: `1..=100` → (1, 100), `..10` → (None, 9).
    pub fn len_bounds(&self) -> (Option<u64>, Option<u64>) {
        bounds(&self.value)
    }
}

/// What a browser can check of a form's field before sending it, from the
/// field's type and rules: only what the server refuses too, so the
/// browser never stops a value the server would take. The server still
/// checks everything.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Native {
    /// Blank is refused: by a struct's field (blank is missing), a number,
    /// an address or an image (blank does not parse), or text whose rules
    /// refuse it.
    pub required: bool,
    /// `type="email"`: `check::email` is WHATWG's valid email address.
    pub email: bool,
    /// The least characters. `minlength` counts UTF-16 units, never fewer
    /// than characters, so it refuses only what the server does.
    pub min_len: Option<u64>,
    /// The most characters. `maxlength` counts UTF-16 units (an emoji is
    /// two), so it is a `pattern`, whose `[\s\S]` is a whole character.
    pub max_len: Option<u64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// An `Image`: its form is `multipart/form-data`, its input takes images.
    pub upload: bool,
    /// `one_of = "draft live"`: the choices, so `<form fields>` writes a
    /// `<select>`.
    pub choices: Vec<String>,
}

impl Native {
    /// The attributes for a `tag` (`input`, `textarea`, `select`) whose
    /// `type` is `kind` (lowercase, `""` when none is written), but those
    /// it has: ` required minlength="8"`.
    pub fn attrs(&self, tag: &str, kind: &str, has: &dyn Fn(&str) -> bool) -> String {
        let mut out = String::new();
        let mut add = |name: &str, value: &str| {
            if !has(name) {
                out.push(' ');
                out.push_str(name);
                if !value.is_empty() {
                    out.push_str("=\"");
                    out.push_str(value);
                    out.push('"');
                }
            }
        };
        let mut kind = kind;
        if tag == "input" && self.email && kind.is_empty() && !has("type") {
            add("type", "email");
            kind = "email";
        }
        if self.required && !has("multiple") {
            add("required", "");
        }
        if tag == "textarea" {
            // A pattern does not apply to it, so no maximum; a minimum of
            // one is `required`.
            if let Some(n) = self.min_len.filter(|&n| n > 1) {
                add("minlength", &n.to_string());
            }
            return out;
        }
        if tag != "input" {
            return out;
        }
        match kind {
            "" | "text" | "search" | "url" | "tel" | "email" | "password" => {
                if let Some(n) = self.min_len.filter(|&n| n > 0) {
                    add("minlength", &n.to_string());
                }
                if let Some(n) = self.max_len {
                    add("pattern", &format!("[\\s\\S]{{0,{n}}}"));
                }
            }
            "number" => {
                // `min` is where the steps start: a whole number keeps the
                // default step's whole numbers, unless a `step` or `value`
                // set them elsewhere.
                if let Some(m) = self.min
                    && m.fract() == 0.0
                    && !has("step")
                    && !has("value")
                {
                    add("min", &m.to_string());
                }
                if let Some(m) = self.max {
                    add("max", &m.to_string());
                }
            }
            "file" if self.upload => add("accept", "image/*"),
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_read() {
        let r = parse("len = 1..=100, email, min = -2").unwrap().rules;
        let want = |key, value: &str| Rule {
            key,
            value: value.into(),
        };
        assert_eq!(r[0], want(Key::Len, "1..=100"));
        assert_eq!(r[1], want(Key::Email, ""));
        assert_eq!(r[0].len_bounds(), (Some(1), Some(100)));
        let q = parse(r#"pattern = "a{1,2}, b", url"#).unwrap().rules;
        assert_eq!((q.len(), q[0].value.as_str()), (2, r#""a{1,2}, b""#));
        let first = |rules| parse(rules).unwrap().rules[0].len_bounds();
        assert_eq!(first("len = ..10"), (None, Some(9)));
        assert_eq!(first("len = N.."), (None, None));
        // As wisp-macros has them: tokens with spaces between.
        let r = rule("len", Some("1 ..= N as u64")).unwrap();
        assert_eq!(r, want(Key::Len, "1..=N as u64"));
        assert_eq!(
            rule("len", Some("1 ..= 100")).unwrap().len_bounds(),
            (Some(1), Some(100))
        );
        // Each key has its rule.
        assert!(
            DEFS.iter()
                .all(|d| DEFS.iter().filter(|e| e.key == d.key).count() == 1)
        );
        // An upload's size is not a rule of the value.
        let v = parse("max_size = 2 * MB").unwrap();
        assert!(v.rules.is_empty() && v.max_size.as_deref() == Some("2 * MB"));
        assert!(rule("max_size", Some("1")).is_err());
        // Each rule: what it checks with, and the error it gives.
        for (rules, want) in [
            ("min = -2", Ok("::wisp::json::check::min(&x, -2i128)")),
            ("max = N", Ok("::wisp::json::check::max(&x, N)")),
            ("max = 1.5", Ok("::wisp::json::check::max(&x, 1.5)")),
            ("min_len = 1", Ok("::wisp::json::check::min_len(&x, 1)")),
            ("max_len = 9", Ok("::wisp::json::check::max_len(&x, 9)")),
            ("len = 1..=9", Ok("::wisp::rt_traits::len(&x, 1..=9)")),
            ("email", Ok("::wisp::json::check::email(&x)")),
            ("url", Ok("::wisp::json::check::url(&x)")),
            (
                "one_of = \"a b\"",
                Ok("::wisp::json::check::one_of(&x, \"a b\")"),
            ),
            (
                "pattern = \"[a-z]+\"",
                Ok(
                    "::wisp::json::check::pattern_once(&x, { static P: ::wisp::json::check::Pattern = ::wisp::json::check::Pattern::new(); &P }, \"[a-z]+\")",
                ),
            ),
            ("with = ok_name", Ok("(ok_name)(&x)")),
            (
                "max_size = 2 * MB",
                Ok("::wisp::rt_traits::max_size(&x, (2 * MB) as usize)"),
            ),
            (
                "size = 1",
                Err(
                    "#[validate] has no `size`: it takes len, min, max, min_len, max_len, email, url, one_of, pattern, with and max_size",
                ),
            ),
            (
                "len = 5",
                Err("`len = 5` needs a range, such as `len = 1..=100`"),
            ),
            (
                "len = ..",
                Err("`len = ..` needs a bound, such as `len = 1..=100`"),
            ),
            (
                "len = . . =",
                Err("`len = ..` needs a bound, such as `len = 1..=100`"),
            ),
            ("min", Err("`min` needs a value: `min = 1`")),
            (
                "pattern = \"[a-\"",
                Err(
                    "`pattern = \"[a-\"` cannot be read: it takes literals, `.`, classes, groups, `|` and `? * + {n} {n,m}`",
                ),
            ),
            ("email = 1", Err("`email` takes no value")),
        ] {
            let got = parse(rules).map(|v| v.checks("&x").swap_remove(0));
            assert_eq!(
                got.as_deref(),
                want.map_err(String::from).as_deref(),
                "{rules}"
            );
        }
    }
}
