//! `src/routes/t/todos`: the todo app of the wispweb.dev home page, as
//! written there: `#[model(saved)]` declares `TODOS`, `{#each TODOS as todo}`
//! walks it, `<form action="?/add" fields />` writes the input and button.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn the_home_page_todo_app() {
    wisp::test::fresh();
    let mut app = client::<Site>();
    let page = app.get("/t/todos").text().to_string();
    assert!(page.contains("<title>Todos (0)</title>"), "{page}");
    assert!(
        page.contains(r#"<input name="text" required minlength="1" pattern="[\s\S]{0,100}">"#),
        "{page}"
    );
    assert!(page.contains("<button>Add</button>"), "{page}");

    // A valid add is a 303 back to the page, which lists it.
    let ok = app.post_form("/t/todos?/add", &[("text", "Milk")]);
    assert_eq!(ok.status, 303);
    assert_eq!(ok.location(), Some("/t/todos"));
    let page = app.get("/t/todos").text().to_string();
    assert!(page.contains("<title>Todos (1)</title>"), "{page}");
    assert!(page.contains("<p>Milk <button"), "{page}");
    assert!(page.contains("?/remove&amp;id=1"), "{page}");

    // A blank one is a 422 with the problem beside the input, the rest kept.
    let bad = app.post_form("/t/todos?/add", &[("text", "")]);
    assert_eq!(bad.status, 422);
    let html = bad.text();
    assert!(
        html.contains(r#"<small class="problem">is required</small>"#),
        "{html}"
    );
    assert!(html.contains("<p>Milk <button"), "{html}");
    let long = "x".repeat(101);
    let bad = app.post_form("/t/todos?/add", &[("text", &long)]);
    assert_eq!(bad.status, 422);
    assert!(bad.text().contains("at most 100"), "{}", bad.text());

    // `update` too, a 404 for an id no row has.
    let changed = app.post_form("/t/todos?/update&id=1", &[("text", "Oat milk")]);
    assert_eq!(changed.status, 303);
    assert!(app.get("/t/todos").text().contains("<p>Oat milk <button"));
    assert_eq!(
        app.post_form("/t/todos?/update&id=9", &[("text", "x")])
            .status,
        404
    );

    // Remove by id.
    let gone = app.post_form("/t/todos?/remove&id=1", &[]);
    assert_eq!(gone.status, 303);
    let page = app.get("/t/todos").text().to_string();
    assert!(
        page.contains("<title>Todos (0)</title>") && !page.contains("Milk"),
        "{page}"
    );
}
