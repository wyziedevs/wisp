//! `src/app.html`: the document around every page.

use crate::fnv1a;

pub const DEFAULT: &str = "<!doctype html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
%wisp.head%
</head>
<body>
%wisp.body%
</body>
</html>
";

/// Every valid shell has the same shape: text, head, text, body, text.
pub const SHAPE: u64 = fnv1a(b"wisp-shell-v1");

/// Splits the shell around `%wisp.head%` and `%wisp.body%`.
pub fn split(src: &str) -> Result<[String; 3], String> {
    let one = |hay: &str, what: &str| -> Result<usize, String> {
        let at = hay.find(what).ok_or_else(|| format!("missing {what}"))?;
        if hay[at + what.len()..].contains(what) {
            return Err(format!("{what} appears more than once"));
        }
        Ok(at)
    };
    let (head, body) = (one(src, "%wisp.head%")?, one(src, "%wisp.body%")?);
    // After it whole: `%wisp.head%wisp.body%` shares a `%`.
    if body < head + "%wisp.head%".len() {
        return Err("%wisp.head% must come before %wisp.body%".into());
    }
    Ok([
        src[..head].to_string(),
        src[head + "%wisp.head%".len()..body].to_string(),
        src[body + "%wisp.body%".len()..].to_string(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_default() {
        let [a, b, c] = split(DEFAULT).unwrap();
        assert!(a.ends_with("initial-scale=1\">\n"));
        assert_eq!(b, "\n</head>\n<body>\n");
        assert_eq!(c, "\n</body>\n</html>\n");
    }

    #[test]
    fn rejects_bad_shells() {
        assert!(split("<html>%wisp.body%</html>").is_err());
        assert!(split("%wisp.body%%wisp.head%").is_err());
        assert!(split("%wisp.head%%wisp.head%%wisp.body%").is_err());
        assert!(split("%wisp.head%wisp.body%").is_err());
    }
}
