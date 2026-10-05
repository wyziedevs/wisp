//! `.env` files: what the build reads for browser code's `env.PUBLIC_X`
//! and the server for `wisp::env`, by the same rules.

/// One pass over a double-quoted value: `\\`, `\n` and `\"` unescape, any
/// other backslash stays as written.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match (c, it.peek()) {
            ('\\', Some('n')) => out.push('\n'),
            ('\\', Some(&e @ ('\\' | '"'))) => out.push(e),
            _ => {
                out.push(c);
                continue;
            }
        }
        it.next();
    }
    out
}

/// The `KEY=value` lines of a `.env` file: `#` comments, `export KEY=…`,
/// and values in '…' or "…" (where `\n` is a line break); then the numbers
/// of the lines that are none of these (blank lines and comments aside),
/// which are skipped.
pub fn parse(text: &str) -> (Vec<(String, String)>, Vec<usize>) {
    let mut vars = Vec::new();
    let mut bad = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            bad.push(n + 1);
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || k.contains(char::is_whitespace) {
            bad.push(n + 1);
            continue;
        }
        let quoted = |q: char| v.strip_prefix(q)?.strip_suffix(q).filter(|_| v.len() > 1);
        let v = match (quoted('"'), quoted('\'')) {
            (Some(d), _) => unescape(d),
            (_, Some(s)) => s.to_string(),
            _ => v.split(" #").next().unwrap_or("").trim_end().to_string(),
        };
        // A name given twice is its last value, as in a shell.
        match vars.iter_mut().find(|(n, _)| n == k) {
            Some(old) => old.1 = v,
            None => vars.push((k.to_string(), v)),
        }
    }
    (vars, bad)
}

#[cfg(test)]
mod tests {
    #[test]
    fn lines() {
        let text = "\u{feff}# a comment\nPUBLIC_A=1\r\nexport PUBLIC_B = two words # note\n\nPUBLIC_C=\"x\\ny # kept\"\nPUBLIC_D='$raw\\n'\nPUBLIC_E=\nnot a line\n=x\nPUBLIC_F=a=b\ntwo words=x\nPUBLIC_A=2";
        let (vars, bad) = super::parse(text);
        assert_eq!(
            vars,
            [
                ("PUBLIC_A", "2"),
                ("PUBLIC_B", "two words"),
                ("PUBLIC_C", "x\ny # kept"),
                ("PUBLIC_D", "$raw\\n"),
                ("PUBLIC_E", ""),
                ("PUBLIC_F", "a=b"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert_eq!(bad, [8, 9, 11]);
    }

    #[test]
    fn escapes() {
        let get = |v: &str| super::parse(&format!("K=\"{v}\"")).0.remove(0).1;
        assert_eq!(get(r"a\\b"), r"a\b");
        assert_eq!(get(r"C:\\new"), r"C:\new");
        assert_eq!(get(r#"say \"hi\""#), r#"say "hi""#);
        assert_eq!(get(r"a\nb"), "a\nb");
        assert_eq!(get(r"a\"), r"a\");
        assert_eq!(get(r"a\tb"), r"a\tb");
        assert_eq!(super::parse(r"K='a\\b'").0[0].1, r"a\\b");
    }
}
