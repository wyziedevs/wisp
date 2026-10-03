//! Static export of the hand-written app: which files are written, and what
//! is left out.

mod common;
#[path = "../../../tests/shared/temp.rs"]
mod temp;

use common::Lab;
use temp::Temp;

fn export(spa: bool) -> (Temp, std::io::Result<()>) {
    let dir = Temp::new(if spa { "export-spa" } else { "export" });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let done = runtime.block_on(wisp::export::<Lab>(&dir, spa));
    (dir, done)
}

/// `--spa`: a page the browser draws whose route has parameters and no
/// `entries` is written once, and the fallback, `index.html`, lists it.
#[test]
fn the_spa_fallback_lists_the_pages_the_browser_draws() {
    let (dir, done) = export(true);
    done.unwrap();
    let read =
        |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    assert!(read("_app/spa/0.html").contains("<p>drawn 0</p>"));
    let index = read("index.html");
    assert!(
        index.contains(
            "<script type=\"application/json\" id=\"wisp-spa\">[[\"/drawn/[id]\",\"/_app/spa/0.html\"]]</script></body>"
        ),
        "{index}"
    );
    // Without --spa, it is not written.
    let (plain, done) = export(false);
    done.unwrap();
    assert!(!plain.join("_app/spa").exists() && !plain.join("index.html").exists());
}

#[test]
fn every_page_is_written_where_a_static_host_serves_it() {
    let (dir, done) = export(false);
    done.unwrap();
    let read =
        |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    assert!(read("hello/index.html").contains("<h1>hello</h1>"));
    // A route with parameters is written once for each entry: a name with a dot is a file.
    assert_eq!(read("item/a/index.html"), "item a 0");
    assert_eq!(read("item/b.txt"), "item b.txt 0");
    // Actions need a server, but the page is still written.
    assert!(read("status/index.html").contains("status"));
    // The error page, for the host to show for a missing address, and what the pages link to.
    assert!(read("404.html").contains("[404:"));
    assert!(read("_app/wisp.js").len() > 1000);

    // Not pages, or not exportable: nothing written.
    // (`other` has an entry that would leave its folder, which refuses the route's whole list;
    // `opt` answers 404, and `bad` has an entry that does not fit.)
    for gone in [
        "nowhere",
        "thing",
        "other",
        "opt",
        "bad",
        "q",
        "item/index.html",
    ] {
        assert!(!dir.join(gone).exists(), "{gone} should not be written");
    }
}

#[test]
fn a_folder_that_cannot_be_written_is_an_error() {
    let file = Temp::new("not-a-folder");
    std::fs::write(&*file, "x").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let done = runtime.block_on(wisp::export::<Lab>(&file, false));
    assert!(done.is_err());
}
