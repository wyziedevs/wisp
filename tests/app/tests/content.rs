//! Markdown pages (`src/routes/blog/*.md`) and the index that lists them.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn markdown_pages() {
    let mut app = client::<Site>();
    let page = app.get("/blog/hello");
    assert_eq!(page.status, 200);
    let text = page.text();
    // The front matter: the title, and the layout given its props.
    assert!(text.contains("<title>Hello, Markdown</title>"), "{text}");
    assert!(
        text.contains(
            "<article class=\"post\">\n<h1>Hello, Markdown</h1>\n<time>2026-10-01</time>"
        ),
        "{text}"
    );
    // Braces are text (as references), a component holds Markdown, code
    // is highlighted.
    assert!(
        text.contains("<p>Some <em>emphasis</em> and &#123;braces&#125;, with <code>code &#123;x&#125;</code>.</p>"),
        "{text}"
    );
    assert!(
        text.contains("<h2>Inside</h2>") && text.contains("<p>2 items</p>"),
        "{text}"
    );
    assert!(
        text.contains("<p>A <strong>card</strong> of Markdown.</p>"),
        "{text}"
    );
    assert!(
        text.contains(
            "<pre><code class=\"language-rust\"><span class=\"hl-k\">fn</span> main() &#123;"
        ),
        "{text}"
    );

    // No front matter title: the first heading. Tables work.
    let text = app.get("/blog/older").text().to_string();
    assert!(text.contains("<title>An older post</title>"), "{text}");
    assert!(text.contains("<td>1</td>"), "{text}");

    // `wisp::pages("blog")`: newest first.
    let text = app.get("/blog").text().to_string();
    assert!(
        text.contains(
            "<li><a href=\"/blog/hello\">Hello, Markdown</a> 2026-10-01</li>\n<li><a href=\"/blog/older\">An older post</a> 2025-01-01</li>"
        ),
        "{text}"
    );
}
