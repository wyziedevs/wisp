//! Rust source read as far as Wisp needs, without parsing it: where a
//! literal or a comment ends, and whether code awaits. The build scans
//! pages' Rust by it (`wisp-build`'s `rust_scan`, `template`) and
//! `#[action]` (`wisp-macros`) reads an action's body by it, alike.

/// A byte of an identifier (not its first, which is no digit).
pub const fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// The skip functions take the index of the opening byte and return the index
// of the last byte of the literal (or the end of input if unterminated).

pub fn skip_str(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i,
            _ => i += 1,
        }
    }
    b.len()
}

/// `'x'`, `'\n'`, `'\u{1F600}'` are chars; `'a` in `&'a str` is a lifetime.
pub fn skip_char(b: &[u8], i: usize) -> usize {
    match b.get(i + 1) {
        Some(b'\\') => {
            let mut j = i + 3;
            while j < b.len() && b[j] != b'\'' {
                j += 1;
            }
            j.min(b.len())
        }
        Some(&c) => {
            let len = match c {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            if b.get(i + 1 + len) == Some(&b'\'') {
                i + 1 + len
            } else {
                i
            }
        }
        None => i,
    }
}

/// If `b[i]` starts a raw string (`r"`, `r#"`, `br"`, `cr"`), returns the
/// hash count.
pub fn raw_str_start(b: &[u8], i: usize) -> Option<usize> {
    let prefix_ok = i == 0
        || !is_word(b[i - 1])
        || (matches!(b[i - 1], b'b' | b'c') && (i == 1 || !is_word(b[i - 2])));
    if b[i] != b'r' || !prefix_ok {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    (b.get(j) == Some(&b'"')).then_some(j - i - 1)
}

pub fn skip_raw_str(b: &[u8], i: usize) -> usize {
    let hashes = raw_str_start(b, i).expect("caller checked");
    let mut j = i + 2 + hashes;
    while j < b.len() {
        if b[j] == b'"'
            && b[j + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == b'#')
                .count()
                == hashes
        {
            return j + hashes;
        }
        j += 1;
    }
    b.len()
}

/// If a literal or comment starts at `i`, the index of its last byte;
/// otherwise `i`.
pub fn skip_literal(b: &[u8], i: usize) -> usize {
    match b[i] {
        b'"' => skip_str(b, i),
        b'\'' => skip_char(b, i),
        b'r' if raw_str_start(b, i).is_some() => skip_raw_str(b, i),
        b'b' if i + 1 < b.len() && raw_str_start(b, i + 1).is_some() => skip_raw_str(b, i + 1),
        b'/' if b.get(i + 1) == Some(&b'/') => {
            let mut j = i;
            while j + 1 < b.len() && b[j + 1] != b'\n' {
                j += 1;
            }
            j
        }
        b'/' if b.get(i + 1) == Some(&b'*') => skip_block_comment(b, i),
        _ => i,
    }
}

/// The index of the first byte at or after `i` that is not whitespace or
/// in a comment.
pub fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            // An unclosed comment runs to the end.
            i = (skip_block_comment(b, i) + 1).min(b.len());
        } else {
            break;
        }
    }
    i
}

pub fn skip_block_comment(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0;
    while i + 1 < b.len() {
        if b[i] == b'/' && b[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if b[i] == b'*' && b[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i - 1;
            }
        } else {
            i += 1;
        }
    }
    b.len()
}

/// Whether the expression `code` awaits: `.await` outside its literals.
pub fn awaits(code: &str) -> bool {
    let b = code.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            let at = skip_space(b, i + 1);
            if b[at..].starts_with(b"await") && b.get(at + 5).is_none_or(|&c| !is_word(c)) {
                return true;
            }
        }
        i = skip_literal(b, i) + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_strings_of_each_prefix() {
        // A raw string's `\` escapes nothing, so its `"` ends it.
        for src in [r#"r"\" }"#, r#"br"\" }"#, r#"cr"\" }"#] {
            let b = src.as_bytes();
            let at = src.find('r').unwrap();
            assert_eq!(raw_str_start(b, at), Some(0), "{src}");
            assert_eq!(skip_literal(b, at), src.find(" }").unwrap() - 1, "{src}");
        }
        assert_eq!(raw_str_start(br#"xr"""#, 1), None);
    }
}
