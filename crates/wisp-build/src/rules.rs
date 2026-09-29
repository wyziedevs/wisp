//! `#[validate(…)]`, as the build reads it off an action's parameter (to
//! check the input before the call) and off a field (for the limits in the
//! OpenAPI document). `#[derive(FromJson)]` reads the same rules from its
//! tokens; the checks themselves are `wisp::json::check`'s.

/// One rule, with its value as written (`""` for `email`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rule<'a> {
    pub key: Key,
    pub value: &'a str,
}

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

/// The rules inside `validate(…)`, or what is wrong with them.
pub fn parse(rules: &str) -> Result<Vec<Rule<'_>>, String> {
    let mut out = Vec::new();
    for rule in rules.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        let (name, value) = match rule.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (rule, None),
        };
        let key = match name {
            "min" => Key::Min,
            "max" => Key::Max,
            "min_len" => Key::MinLen,
            "max_len" => Key::MaxLen,
            "len" => Key::Len,
            "email" => Key::Email,
            _ => {
                return Err(format!(
                    "#[validate] has no `{name}`: it takes len, min, max, min_len, max_len and email"
                ));
            }
        };
        let value = match (key, value) {
            (Key::Email, None) => "",
            (Key::Email, Some(_)) => return Err("`email` takes no value".into()),
            (_, None) => return Err(format!("`{name}` needs a value: `{name} = 1`")),
            (Key::Len, Some(v)) if !v.contains("..") => {
                return Err(format!(
                    "`len = {v}` needs a range, such as `len = 1..=100`"
                ));
            }
            (Key::Len, Some(v)) if v.trim_start_matches(['.', '=']).trim().is_empty() => {
                return Err("`len = ..` needs a bound, such as `len = 1..=100`".into());
            }
            (_, Some(v)) => v,
        };
        out.push(Rule { key, value });
    }
    Ok(out)
}

impl Rule<'_> {
    /// The call of `wisp::json::check` that checks the value `v` (a
    /// reference) by this rule: `Some(problem)` when it does not pass.
    pub fn check(&self, v: &str) -> String {
        let x = self.value;
        match self.key {
            Key::Min => format!("::wisp::json::check::min({v}, ({x}) as f64)"),
            Key::Max => format!("::wisp::json::check::max({v}, ({x}) as f64)"),
            Key::MinLen => format!("::wisp::json::check::min_len({v}, {x})"),
            Key::MaxLen => format!("::wisp::json::check::max_len({v}, {x})"),
            Key::Len => format!("::wisp::rt_traits::len({v}, {x})"),
            Key::Email => format!("::wisp::json::check::email({v})"),
        }
    }

    /// The least and most length `len = …` allows, when they are plain
    /// numbers: `1..=100` → (1, 100), `..10` → (None, 9).
    pub fn len_bounds(&self) -> (Option<u64>, Option<u64>) {
        let x: String = self.value.split_whitespace().collect();
        let Some((lo, hi)) = x.split_once("..") else {
            return (None, None);
        };
        let hi = match hi.strip_prefix('=') {
            Some(h) => h.parse().ok(),
            None => hi.parse::<u64>().ok().map(|h| h.saturating_sub(1)),
        };
        (lo.parse().ok(), hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_read() {
        let r = parse("len = 1..=100, email, min = -2").unwrap();
        assert_eq!(
            r[0],
            Rule {
                key: Key::Len,
                value: "1..=100"
            }
        );
        assert_eq!(
            r[1],
            Rule {
                key: Key::Email,
                value: ""
            }
        );
        assert_eq!(r[0].len_bounds(), (Some(1), Some(100)));
        assert_eq!(
            parse("len = ..10").unwrap()[0].len_bounds(),
            (None, Some(9))
        );
        assert_eq!(parse("len = N..").unwrap()[0].len_bounds(), (None, None));
        assert_eq!(
            r[2].check("&x"),
            "::wisp::json::check::min(&x, (-2) as f64)"
        );
        for (bad, why) in [
            ("size = 1", "has no `size`"),
            ("len = 5", "needs a range"),
            ("len = ..", "needs a bound"),
            ("min", "needs a value"),
            ("email = 1", "takes no value"),
        ] {
            assert!(parse(bad).unwrap_err().contains(why), "{bad}");
        }
    }
}
