//! `+loading.wisp`: what a client navigation shows at once while the next
//! page loads (wisp.js, `wait`). The build writes every folder's loading
//! markup, by the folder's URL pattern, into the shell's head as JSON, so a
//! navigation needs no request for it and an app with none writes nothing.

use crate::routes::Tree;

/// `<script type="application/json" id="wisp-loading">[["/blog","<p>…"]]</script>`
/// from the tree's `+loading.wisp` files, or nothing.
pub fn tag(tree: &Tree, root: &std::path::Path) -> Result<String, String> {
    if tree.loading.is_empty() {
        return Ok(String::new());
    }
    let mut items = Vec::new();
    for (prefix, file) in &tree.loading {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let src = crate::read_source(file).map_err(|e| format!("{rel}: {e}"))?;
        let html = src.trim();
        if html.starts_with("---")
            || html.to_ascii_lowercase().contains("<script")
            || ["{#", "{@", "{:"].iter().any(|b| html.contains(b))
        {
            return Err(format!(
                "{rel}: a loading view is static HTML (a `<style>` is fine): no `---` block, script, `{{…}}` holes or components, which wisp.js shows before the page's own code is there"
            ));
        }
        items.push(format!("[{},{}]", json(prefix), json(html)));
    }
    Ok(format!(
        "<script type=\"application/json\" id=\"wisp-loading\">[{}]</script>\n",
        items.join(",")
    ))
}

/// A JSON string that is safe inside `<script>`: `<` and the line
/// separators are escaped.
fn json(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn writes_the_views_by_folder() {
        let root = std::env::temp_dir().join(format!("wisp-loading-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/routes/blog")).unwrap();
        fs::write(
            root.join("src/routes/blog/+loading.wisp"),
            "<p class=\"s\">Loading \"x\"</p>\n",
        )
        .unwrap();
        fs::write(root.join("src/routes/+page.wisp"), "x").unwrap();
        let tree = crate::routes::scan(&root.join("src/routes")).unwrap();
        let t = tag(&tree, &root).unwrap();
        assert_eq!(
            t,
            "<script type=\"application/json\" id=\"wisp-loading\">[[\"/blog\",\"\\u003cp class=\\\"s\\\">Loading \\\"x\\\"\\u003c/p>\"]]</script>\n"
        );
        let none = Tree::default();
        assert_eq!(tag(&none, &PathBuf::new()).unwrap(), "");
        fs::write(root.join("src/routes/blog/+loading.wisp"), "{#if x}a{/if}").unwrap();
        let tree = crate::routes::scan(&root.join("src/routes")).unwrap();
        assert!(tag(&tree, &root).unwrap_err().contains("static HTML"));
        let _ = fs::remove_dir_all(&root);
    }
}
