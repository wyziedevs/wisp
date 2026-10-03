//! What `#[derive(Config)]` reads: a variable of the environment (or `.env`)
//! per field, every one wrong named at once. The value is never in the
//! message: it may be a secret.

use crate::Error;
use std::str::FromStr;

/// A required field `key`, or `None` and what is wrong in `bad`.
pub fn config<T: FromStr>(key: &str, bad: &mut Vec<String>) -> Option<T> {
    match crate::env(key) {
        Some(v) => parse(key, &v, bad),
        None => {
            bad.push(format!("{key} is not set"));
            None
        }
    }
}

/// An `Option` field: unset is `Some(None)`, a value that does not parse is
/// `None`.
pub fn config_opt<T: FromStr>(key: &str, bad: &mut Vec<String>) -> Option<Option<T>> {
    match crate::env(key) {
        Some(v) => parse(key, &v, bad).map(Some),
        None => Some(None),
    }
}

fn parse<T: FromStr>(key: &str, v: &str, bad: &mut Vec<String>) -> Option<T> {
    let parsed = v.trim().parse().ok();
    if parsed.is_none() {
        let ty = std::any::type_name::<T>();
        bad.push(format!("{key} is not a {ty}"));
    }
    parsed
}

/// Why the app cannot start.
pub fn config_error(bad: &[String]) -> Error {
    Error::new(500, format!("the environment is not right: {}", bad.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_wrong_variable_is_named_and_no_value_is_shown() {
        let mut bad = Vec::new();
        assert_eq!(config::<String>("WISP_CONF_UNSET", &mut bad), None);
        assert_eq!(config_opt::<u16>("WISP_CONF_UNSET", &mut bad), Some(None));
        assert_eq!(parse::<u16>("PORT", "http", &mut bad), None);
        assert_eq!(parse::<u16>("PORT", " 80 ", &mut bad), Some(80));
        let e = config_error(&bad).message().to_string();
        assert!(e.contains("WISP_CONF_UNSET is not set; PORT is not a u16"), "{e}");
        assert!(!e.contains("http"), "{e}");
    }
}
