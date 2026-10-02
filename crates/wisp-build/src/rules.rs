//! `#[validate(…)]`, in one place: the build reads it off an action's
//! parameter (to check the input before the call), off a field (for the
//! limits in the OpenAPI document) and off both for the attributes a form's
//! field gets (the browser checks before it sends); `#[derive(FromJson)]`
//! (wisp-macros) reads the same rules, with the same messages, from its
//! tokens. The checks themselves are `wisp::json::check`'s. Each rule is
//! one entry of `DEFS`.

use crate::rust_scan::{Items, TypeItem};
use crate::ty::{self, Scalar};

/// One rule, with its value as written (`""` for `email`; a range with no
/// spaces around its `..`, as `1..=100`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rule {
    pub key: Key,
    pub value: String,
}

/// A rule of a value, by its place in `DEFS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Min,
    Max,
    MinLen,
    MaxLen,
    /// A range: `1..=100`, `..10`, `3..`.
    Len,
    Email,
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

/// Every rule, in `Key`'s order.
const DEFS: [Def; 6] = [
    Def {
        key: Key::Min,
        name: "min",
        takes: Takes::Value,
        check: |v, x| format!("::wisp::json::check::min({v}, ({x}) as f64)"),
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
        check: |v, x| format!("::wisp::json::check::max({v}, ({x}) as f64)"),
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
];

/// `max_size = 1 * MB`: not a rule of the value but the most bytes of an
/// upload, which the route's body limit makes room for.
const MAX_SIZE: &str = "max_size";

/// A number as a browser's `min` or `max` takes it: plain and finite.
fn plain(x: &str) -> Option<f64> {
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

/// What is inside `validate(…)`, or what is wrong with it.
pub fn parse(rules: &str) -> Result<Validate, String> {
    let mut out = Validate::default();
    for r in rules.split(',').map(str::trim).filter(|r| !r.is_empty()) {
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
            "#[validate] has no `{name}`: it takes len, min, max, min_len, max_len, email and max_size"
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
    Ok(Rule {
        key: def.key,
        value,
    })
}

impl Rule {
    /// The call of `wisp::json::check` that checks the value `v` (a
    /// reference) by this rule: `Some(problem)` when it does not pass.
    pub fn check(&self, v: &str) -> String {
        (DEFS[self.key as usize].check)(v, &self.value)
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
}

impl Native {
    /// For a field of type `ty` with `rules`; `whole` for a struct's field
    /// (`fn default(post: Post)`), whose blank is missing.
    pub fn of(ty: &str, rules: &[Rule], whole: bool) -> Native {
        let (optional, t) = match ty::option_inner(ty) {
            Some(inner) => (true, inner),
            None => (false, ty),
        };
        let last = ty::last_segment(ty::unref(t));
        let text = ty::is_text(t);
        let number = matches!(
            ty::scalar(t),
            Scalar::Unsigned | Scalar::Signed | Scalar::Float
        );
        let mut n = Native::default();
        if !text && !number && last != "Email" && last != "Image" {
            return n;
        }
        n.email = last == "Email";
        for r in rules {
            (DEFS[r.key as usize].native)(&mut n, &r.value, Kind { text, number });
        }
        let blank_refused = whole || !text || n.email || n.min_len.is_some_and(|l| l > 0);
        n.required = !optional && blank_refused;
        n
    }

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
            _ => {}
        }
        out
    }
}

/// A field of a form that posts to `action`, which reads it as `name`.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    pub action: String,
    pub name: String,
    pub native: Native,
}

/// The fields of the actions in `items` the browser can check: each
/// parameter, and each field of a struct that one reads whole (`fn
/// default(post: Post)`), which the file defines or the app's `shared`
/// modules do. None that is a route parameter in `params`: the route, not
/// the form, gives that.
pub fn fields(items: &Items, params: &[&str], shared: &[TypeItem]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut push = |action: &str, name: String, native: Native| {
        if native != Native::default() && !params.contains(&name.as_str()) {
            out.push(Field {
                action: action.to_string(),
                name,
                native,
            });
        }
    };
    for f in items.fns.iter().filter(|f| f.action) {
        for (p, t) in &f.params {
            let whole = items.types.iter().chain(shared).find(|s| {
                s.name == ty::last_segment(t)
                    && s.derives.iter().any(|d| d == "FromJson" || d == "Rest")
            });
            if let Some(s) = whole {
                for (name, ft) in &s.fields {
                    let name = name.strip_prefix("r#").unwrap_or(name);
                    if s.set_by_wisp(name) {
                        continue;
                    }
                    let rules = (s.rules.iter().filter(|(n, _)| n == name))
                        .flat_map(|(_, r)| parse(r).unwrap_or_default().rules)
                        .collect::<Vec<_>>();
                    push(&f.name, name.to_string(), Native::of(ft, &rules, true));
                }
                continue;
            }
            // `body: T` is the whole JSON body, not a field.
            if p == "body" && !ty::is_maybe_text(t) {
                continue;
            }
            let rules = (f.checks.iter().filter(|(c, _)| c == p))
                .flat_map(|(_, r)| parse(r).unwrap_or_default().rules)
                .collect::<Vec<_>>();
            push(&f.name, p.clone(), Native::of(t, &rules, false));
        }
    }
    out
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
        // `Key` is the place in `DEFS`.
        assert!(DEFS.iter().enumerate().all(|(k, d)| d.key as usize == k));
        // An upload's size is not a rule of the value.
        let v = parse("max_size = 2 * MB").unwrap();
        assert!(v.rules.is_empty() && v.max_size.as_deref() == Some("2 * MB"));
        assert!(rule("max_size", Some("1")).is_err());
        // Each rule: what it checks with, and the error it gives.
        for (rules, want) in [
            ("min = -2", Ok("::wisp::json::check::min(&x, (-2) as f64)")),
            ("max = N", Ok("::wisp::json::check::max(&x, (N) as f64)")),
            ("min_len = 1", Ok("::wisp::json::check::min_len(&x, 1)")),
            ("max_len = 9", Ok("::wisp::json::check::max_len(&x, 9)")),
            ("len = 1..=9", Ok("::wisp::rt_traits::len(&x, 1..=9)")),
            ("email", Ok("::wisp::json::check::email(&x)")),
            (
                "max_size = 2 * MB",
                Ok("::wisp::rt_traits::max_size(&x, (2 * MB) as usize)"),
            ),
            (
                "size = 1",
                Err(
                    "#[validate] has no `size`: it takes len, min, max, min_len, max_len, email and max_size",
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

    #[test]
    fn browsers_check_what_the_server_refuses() {
        // (field: `type | rules`, a struct's when it starts `struct `; tag
        // and its type; the attributes it gets)
        for (field, tag, want) in [
            // Blank text is taken unless a rule refuses it.
            ("String |", "input", ""),
            (
                "String | min_len = 8",
                "input password",
                " required minlength=\"8\"",
            ),
            (
                "String | max_len = 5",
                "input",
                " pattern=\"[\\s\\S]{0,5}\"",
            ),
            (
                "String | len = 1..=100",
                "input",
                " required minlength=\"1\" pattern=\"[\\s\\S]{0,100}\"",
            ),
            ("String | len = 1..=100", "textarea", " required"),
            ("Option<String> | len = 1..", "input", " minlength=\"1\""),
            ("String | email", "input", " type=\"email\" required"),
            ("String | email", "input text", " required"),
            ("Option<Email> |", "input", " type=\"email\""),
            ("Email |", "input", " type=\"email\" required"),
            // A struct's blank field is missing.
            ("struct String |", "input", " required"),
            ("struct Option<String> |", "input", ""),
            (
                "u32 | min = 1, max = 10",
                "input number",
                " required min=\"1\" max=\"10\"",
            ),
            ("f64 | min = 0.5", "input number", " required"),
            ("u32 | min = 1", "input", " required"),
            ("u32 | min = LOW", "input number", " required"),
            ("Image | max_size = 1 * MB", "input file", " required"),
            ("Option<Image> |", "input file", ""),
            ("String | min_len = 1", "select", " required"),
            ("bool |", "input checkbox", ""),
            ("Vec<String> | len = 1..", "select", ""),
            ("char |", "input", ""),
        ] {
            let (whole, field) = match field.strip_prefix("struct ") {
                Some(f) => (true, f),
                None => (false, field),
            };
            let (ty, rules) = field.split_once('|').unwrap();
            let (tag, kind) = tag.split_once(' ').unwrap_or((tag, ""));
            let n = Native::of(ty.trim(), &parse(rules).unwrap().rules, whole);
            assert_eq!(n.attrs(tag, kind, &|_| false), want, "{field} {tag}");
        }
        // What the tag says stays: no second `type`, `required` or `min`.
        let n = Native::of("u32", &parse("min = 1").unwrap().rules, false);
        assert_eq!(n.attrs("input", "number", &|a| a == "value"), " required");
        let n = Native::of("String", &parse("email").unwrap().rules, false);
        assert_eq!(n.attrs("input", "", &|a| a == "type"), " required");
        assert_eq!(n.attrs("select", "", &|a| a == "multiple"), "");
    }

    #[test]
    fn fields_of_actions() {
        let items = crate::rust_scan::scan(
            "#[derive(FromJson)] struct Post { #[validate(len = 1..=9)] title: String, note: Option<String>, done: bool }\n\
             #[action] fn add(#[validate(min_len = 2)] text: String, slug: String, n: Option<u8>) {}\n\
             #[action] fn default(post: Post) {}\n\
             #[action] fn save(mut note: models::Note) {}\n\
             fn helper(x: Email) {}",
        )
        .unwrap();
        // A type of the app's own modules too (`src/models.rs`).
        let shared = crate::rust_scan::scan(
            "#[derive(Rest)] pub struct Note { #[validate(min = 1)] stars: u8, created_at: String }",
        )
        .unwrap()
        .types;
        let got: Vec<(String, String, bool)> = fields(&items, &["slug"], &shared)
            .into_iter()
            .map(|f| (f.action, f.name, f.native.required))
            .collect();
        let want = [
            ("add", "text", true),
            ("default", "title", true),
            ("save", "stars", true),
        ];
        let want: Vec<(String, String, bool)> = want
            .iter()
            .map(|&(a, n, r)| (a.into(), n.into(), r))
            .collect();
        assert_eq!(got, want);
    }
}
