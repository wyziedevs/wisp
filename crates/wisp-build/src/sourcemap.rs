//! Version 3 source maps (ECMA-426), a segment per line: enough for the
//! browser's stack traces and DevTools to show the `.wisp` (or `src/lib`)
//! line a module's line came from. Written in dev, and in release with
//! `wisp build --sourcemap`.

use crate::json_str;

/// Where a generated line starts in its source: a 0-based line and column,
/// or `None` for code of Wisp's own.
pub type Line = Option<(u32, u32)>;

/// The map of `file`, whose lines came from the one `source` (named
/// `wisp:///src/…`, its text in the map so DevTools needs no request).
pub fn encode(file: &str, source: &str, content: &str, lines: &[Line]) -> String {
    let mut m = String::new();
    // Fields after the first of a line are deltas from the last segment.
    let (mut line, mut col) = (0i64, 0i64);
    let end = lines.iter().rposition(Option::is_some).map_or(0, |k| k + 1);
    for (k, at) in lines[..end].iter().enumerate() {
        if k > 0 {
            m.push(';');
        }
        let Some((l, c)) = *at else { continue };
        let (l, c) = (i64::from(l), i64::from(c));
        // Generated column 0, source 0, then where in it.
        m.push_str("AA");
        vlq(&mut m, l - line);
        vlq(&mut m, c - col);
        (line, col) = (l, c);
    }
    format!(
        "{{\"version\":3,\"file\":{},\"sources\":[{}],\"sourcesContent\":[{}],\"names\":[],\"mappings\":\"{m}\"}}",
        json_str(file),
        json_str(source),
        json_str(content)
    )
}

/// The lines of a file served as it was written, but for a few lines
/// added at its end (`extra`): each is its own.
pub fn same(text: &str, extra: usize) -> Vec<Line> {
    let n = text.matches('\n').count() + 1;
    (0..n as u32)
        .map(|k| Some((k, 0)))
        .chain(std::iter::repeat_n(None, extra))
        .collect()
}

/// The comment that ends a module whose map is at `name` beside it.
pub fn comment(name: &str) -> String {
    format!("//# sourceMappingURL={name}.map\n")
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 VLQ: the sign in the lowest bit, then 5 bits a digit, lowest
/// first, with bit 6 set on all but the last.
fn vlq(out: &mut String, v: i64) {
    let mut n = (v.unsigned_abs() << 1) | u64::from(v < 0);
    loop {
        let digit = (n & 31) as usize;
        n >>= 5;
        out.push(B64[digit | if n > 0 { 32 } else { 0 }] as char);
        if n == 0 {
            break;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// The segments of `mappings`, a list per generated line, each field
    /// absolute: `[column, source, line, column]`.
    pub fn decode(mappings: &str) -> Vec<Vec<[i64; 4]>> {
        let mut abs = [0i64; 4];
        let mut out = Vec::new();
        for line in mappings.split(';') {
            abs[0] = 0;
            let mut segs = Vec::new();
            for seg in line.split(',').filter(|s| !s.is_empty()) {
                let (mut field, mut shift, mut n) = (0, 0, 0i64);
                for c in seg.bytes() {
                    let d = B64.iter().position(|&b| b == c).unwrap() as i64;
                    n |= (d & 31) << shift;
                    shift += 5;
                    if d & 32 == 0 {
                        let v = if n & 1 == 1 { -(n >> 1) } else { n >> 1 };
                        abs[field] += v;
                        field += 1;
                        (shift, n) = (0, 0);
                    }
                }
                segs.push(abs);
            }
            out.push(segs);
        }
        out
    }

    /// The `mappings` of a map `encode` wrote.
    pub fn mappings(map: &str) -> &str {
        let i = map.find("\"mappings\":\"").unwrap() + 12;
        &map[i..map.len() - 2]
    }

    #[test]
    fn vlq_digits() {
        let enc = |v| {
            let mut s = String::new();
            vlq(&mut s, v);
            s
        };
        // The examples of the spec and of the common implementations.
        assert_eq!(enc(0), "A");
        assert_eq!(enc(1), "C");
        assert_eq!(enc(-1), "D");
        assert_eq!(enc(15), "e");
        assert_eq!(enc(16), "gB");
        assert_eq!(enc(-16), "hB");
        assert_eq!(enc(123), "2H");
        assert_eq!(enc(1000), "w+B");
        for v in [-123_456_789, -31, 31, 32, 1 << 40] {
            assert_eq!(decode(&format!("AA{}A", enc(v)))[0][0][2], v);
        }
    }

    #[test]
    fn maps_round_trip() {
        let lines = [None, Some((4, 2)), Some((5, 0)), None, Some((1, 7)), None];
        let map = encode("t1.js", "wisp:///src/routes/+page.wisp", "a\n\"b\"", &lines);
        assert!(map.starts_with("{\"version\":3,\"file\":\"t1.js\",\"sources\":[\"wisp:///src/routes/+page.wisp\"],\"sourcesContent\":[\"a\\n\\\"b\\\"\"]"), "{map}");
        let segs = decode(mappings(&map));
        // Trailing lines with nothing are left out.
        assert_eq!(segs.len(), 5);
        for (k, at) in lines.iter().take(5).enumerate() {
            match at {
                None => assert!(segs[k].is_empty()),
                Some((l, c)) => assert_eq!(segs[k], [[0, 0, i64::from(*l), i64::from(*c)]]),
            }
        }
        assert_eq!(same("a\nb", 1), [Some((0, 0)), Some((1, 0)), None]);
    }
}
