//! Just enough Rust lexing to list a file's top-level functions.
//!
//! The build step needs to know whether `+page.rs` defines `load`, which
//! functions carry `#[action]`, and which HTTP methods `+server.rs` defines.
//! Types and signatures are left to rustc: if a function has the wrong
//! signature, the generated call site fails to compile with a normal error.

use crate::template::{raw_str_start, skip_char, skip_raw_str, skip_str};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnItem {
    pub name: String,
    pub action: bool,
}

/// Top-level `fn` items in source order. Nested functions, functions inside
/// `impl`/`mod` blocks, comments and string contents are ignored.
pub fn top_level_fns(src: &str) -> Vec<FnItem> {
    let b = src.as_bytes();
    let mut fns = Vec::new();
    let mut depth = 0u32;
    let mut action = false; // saw #[action] since the last item ended
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => i = skip_block_comment(b, i),
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            b'#' if depth == 0 => {
                // Attribute: #[path] or #![path]. Record the marker, skip the rest.
                let mut j = i + 1;
                if b.get(j) == Some(&b'!') {
                    j += 1;
                }
                if b.get(j) == Some(&b'[') {
                    let end = matching_bracket(b, j);
                    let path: String = src[j + 1..end].chars().filter(|c| !c.is_whitespace()).collect();
                    action |= path == "action" || path == "wisp::action";
                    i = end;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    action = false;
                }
            }
            b';' if depth == 0 => action = false,
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if depth == 0 && &src[start..i] == "fn" {
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    let name_start = i;
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                        i += 1;
                    }
                    if i > name_start {
                        fns.push(FnItem { name: src[name_start..i].to_string(), action });
                    }
                    action = false;
                }
                continue; // `i` already points past the identifier.
            }
            _ => {}
        }
        i += 1;
    }
    fns
}

fn skip_block_comment(b: &[u8], mut i: usize) -> usize {
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

fn matching_bracket(b: &[u8], open: usize) -> usize {
    let mut depth = 0;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'"' => i = skip_str(b, i),
            _ => {}
        }
        i += 1;
    }
    b.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<(String, bool)> {
        top_level_fns(src).into_iter().map(|f| (f.name, f.action)).collect()
    }

    #[test]
    fn finds_load_and_actions() {
        let src = r#"
            use wisp::prelude::*;
            // fn commented_out() {}
            /* fn also_not() { /* nested */ } */
            pub struct Data { pub n: i32 }

            /// Loads things.
            pub async fn load(cx: &mut Cx) -> Result<Data> {
                fn inner() {}
                let s = "fn fake() {";
                let c = '{';
                Ok(Data { n: 1 })
            }

            #[action]
            pub async fn like(cx: &mut Cx) -> Result<()> { Ok(()) }

            #[wisp::action]
            #[allow(unused)]
            pub(crate) async fn delete_all<'a>(cx: &'a mut Cx) -> Result<()> { Ok(()) }

            pub async fn helper() {}
            impl Data { pub fn method(&self) {} }
        "#;
        assert_eq!(
            names(src),
            [
                ("load".into(), false),
                ("like".into(), true),
                ("delete_all".into(), true),
                ("helper".into(), false)
            ]
        );
    }

    #[test]
    fn action_marker_does_not_leak_past_items() {
        let src = "#[action] const X: i32 = 1;\npub async fn load() {}";
        assert_eq!(names(src), [("load".into(), false)]);
    }
}
