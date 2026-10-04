//! `wisp openapi`: the document printed, written, and checked against the
//! committed file.

use crate::{Dir, fail, has, new_app, read, wisp, write};

#[test]
fn the_document_is_printed_written_and_checked() {
    let cwd = Dir::new("openapi");
    let api = new_app(&cwd, "api", &["--api"]);

    // Printed: pretty, valid, and what the app describes.
    let o = wisp(&api, &["openapi"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "\"openapi\": \"3.1.0\"",
            "\"title\": \"api\"",
            "\"/api/notes\"",
            "\"/api/notes/{id}\"",
            "\"securitySchemes\"",
        ],
    );
    assert!(o.out.starts_with("{\n  \"openapi\""));

    // No file to check yet.
    fail(&api, &["openapi", "--check"], "There is no openapi.json");

    // Written, then up to date.
    let o = wisp(&api, &["openapi", "-o", "openapi.json"]);
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["Wrote openapi.json"]);
    assert_eq!(read(&api, "openapi.json"), wisp(&api, &["openapi"]).out);
    let o = wisp(&api, &["openapi", "--check"]);
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["openapi.json is up to date"]);

    // A new endpoint makes the committed file stale; writing it again heals.
    write(
        &api,
        "src/routes/ping/+server.rs",
        "fn get() -> String { \"pong\".into() }\n",
    );
    fail(&api, &["openapi", "--check"], "openapi.json is out of date");
    let o = wisp(&api, &["openapi", "-o", "openapi.json"]);
    assert!(o.ok, "{}", o.err);
    has(&read(&api, "openapi.json"), &["\"/ping\""]);
    assert!(wisp(&api, &["openapi", "--check"]).ok);

    // A file of another name, in a folder that is not there yet.
    assert!(wisp(&api, &["openapi", "-o", "docs/spec.json"]).ok);
    assert!(wisp(&api, &["openapi", "--check", "-o", "docs/spec.json"]).ok);
}

#[test]
fn the_options_are_checked() {
    let cwd = Dir::new("openapi-options");
    let api = new_app(&cwd, "api", &["--api"]);
    fail(&api, &["openapi", "--yaml"], "Unexpected --yaml");
    fail(&api, &["openapi", "-o"], "-o takes a file");
    let minimal = new_app(&cwd, "min", &["--template", "minimal"]);
    fail(
        &minimal,
        &["openapi"],
        "The app has no endpoints (+server.rs files) or form actions",
    );
}
