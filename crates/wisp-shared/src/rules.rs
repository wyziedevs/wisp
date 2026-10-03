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

/// Every rule.
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
}
