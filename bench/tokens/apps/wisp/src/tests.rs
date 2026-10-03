//! Each of the five features the token count is for does what it says.

use super::App;
use wisp::test::client;

#[test]
fn list_layout_and_api() {
    let mut app = client::<App>();
    let page = app.get("/").text().to_string();
    assert!(page.contains("<li>Coffee: $4</li>"), "{page}");
    assert!(page.contains("<a href=\"/search\">Search</a>"), "{page}");
    assert!(page.contains("<title>Items</title>"), "{page}");
    let api = app.get("/api/items").text().to_string();
    assert!(
        api.starts_with("[{\"id\":1,\"name\":\"Tea\",\"price\":3}"),
        "{api}"
    );
}

#[test]
fn form() {
    let mut app = client::<App>();
    let bad = app.post_form("/contact", &[("name", "Ann"), ("email", "ann")]);
    assert_eq!(bad.status, 422);
    let page = bad.text().to_string();
    // The browser's own checks from the rules, what was sent, and what was
    // wrong with it, after the input.
    assert!(
        page.contains(
            "<label>Name <input name=\"name\" required minlength=\"1\" pattern=\"[\\s\\S]{0,50}\" value=\"Ann\"></label>"
        ),
        "{page}"
    );
    assert!(
        page.contains(
            "<label>Email <input name=\"email\" type=\"email\" required value=\"ann\"><small class=\"problem\">"
        ),
        "{page}"
    );
    assert_eq!(
        page.matches("<small class=\"problem\">").count(),
        1,
        "{page}"
    );
    let fine = app.post_form("/contact", &[("name", "Ann"), ("email", "ann@example.com")]);
    assert_eq!(fine.status, 303);
    assert!(!app.get("/contact").text().contains("problem"));
}

#[test]
fn search() {
    let mut app = client::<App>();
    let page = app.get("/search").text().to_string();
    let at = page.find("/_app/c/").expect("the page's module");
    let url = &page[at..at + page[at..].find(['"', '?']).unwrap()];
    let module = app.get(url).text().to_string();
    // `bind:value="q"` declared `q`, as state, with no script.
    assert!(module.contains("let q = __wisp_s()"), "{module}");
    // `items` is the block's `items`, sent as it is.
    assert!(
        module.contains("__wisp_props(__wisp_p, [\"items\"])"),
        "{module}"
    );
    assert!(page.contains("\"items\":[{\"id\":1,"), "{page}");
}
