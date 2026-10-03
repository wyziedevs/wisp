//! Just enough JSON for the CLI and the compiler: cargo's messages,
//! package.json, the npm registry's answers. Numbers are kept as written
//! and members in their order, so a document can be written back the same.

#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// A member of an object (the last, if the key is there twice).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A string member of an object.
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    /// The items of an array; nothing for anything else.
    pub fn items(&self) -> std::slice::Iter<'_, Json> {
        match self {
            Json::Arr(a) => a.iter(),
            _ => [].iter(),
        }
    }
}

/// `text` if it is one JSON value, with nothing after it but whitespace.
pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("the end"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, want: &str) -> String {
        let line = self.s[..self.i.min(self.s.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        format!("line {}: expected {want}", line + 1)
    }

    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        let hit = self.s.get(self.i) == Some(&b);
        self.i += hit as usize;
        hit
    }

    fn value(&mut self, depth: u32) -> Result<Json, String> {
        if depth > 64 {
            return Err(self.err("less nesting"));
        }
        self.ws();
        let word = |p: &mut Self, w: &str, v: Json| {
            if p.s[p.i..].starts_with(w.as_bytes()) {
                p.i += w.len();
                Ok(v)
            } else {
                Err(p.err("a value"))
            }
        };
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                if self.eat(b'}') {
                    return Ok(Json::Obj(members));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return Err(self.err("`:`"));
                    }
                    members.push((k, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Ok(Json::Obj(members));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("`,` or `}`"));
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Ok(Json::Arr(items));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("`,` or `]`"));
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => word(self, "true", Json::Bool(true)),
            Some(b'f') => word(self, "false", Json::Bool(false)),
            Some(b'n') => word(self, "null", Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while matches!(
                    self.s.get(self.i),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.i += 1;
                }
                let n = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                n.parse::<f64>()
                    .map(|_| Json::Num(n))
                    .map_err(|_| self.err("a number"))
            }
            _ => Err(self.err("a value")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(self.err("a string"));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else {
                return Err(self.err("`\"`"));
            };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else {
                        return Err(self.err("an escape"));
                    };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut c = self.hex4()?;
                            // A surrogate pair: \ud83d\ude00.
                            if (0xd800..0xdc00).contains(&c) {
                                if !self.s[self.i..].starts_with(b"\\u") {
                                    return Err(self.err("a low surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.err("a low surrogate"));
                                }
                                c = 0x10000 + ((c - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            char::from_u32(c).ok_or_else(|| self.err("a character"))?
                        }
                        _ => return Err(self.err("an escape")),
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                b => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .s
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("4 hex digits"))?;
        let n = std::str::from_utf8(digits)
            .ok()
            .and_then(|d| u32::from_str_radix(d, 16).ok())
            .ok_or_else(|| self.err("4 hex digits"))?;
        self.i += 4;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_json() {
        let v = parse(
            r#" {"a": [1, -1.5e3, true, null], "s": "\u00e9\ud83d\ude00\n", "a": {"b": "c"}} "#,
        )
        .unwrap();
        assert_eq!(v.get("a").and_then(|a| a.str("b")), Some("c"));
        assert_eq!(v.str("s"), Some("é😀\n"));
        let arr = parse("[1, -1.5e3]").unwrap();
        assert_eq!(arr.items().nth(1), Some(&Json::Num("-1.5e3".into())));
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "tru",
            "\"x",
            "{} x",
            r#""\ud83dA""#,
            "\"\\x\"",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(parse("{\n\"a\" 1}").unwrap_err(), "line 2: expected `:`");
    }
}
