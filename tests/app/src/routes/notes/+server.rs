//! A JSON API from a type alone: list and create here, get, put, patch and
//! delete at `/notes/[id]`. Writes need a bearer token: the
//! `CARGO_PKG_NAME` cargo sets when it runs the tests.

#[derive(Rest)]
#[rest(write = "CARGO_PKG_NAME")]
struct Note {
    #[validate(len = 1..=20)]
    title: String,
    done: bool,
}
