use wisp::prelude::*;

#[test]
fn downloads_and_empty_responses() {
    let csv = Response::download("my \"report\".csv", "a,b\n");
    assert_eq!((csv.status, &*csv.content_type), (200, "text/csv"));
    assert_eq!(
        csv.headers,
        [(
            "content-disposition".into(),
            "attachment; filename=\"my _report_.csv\"".to_string()
        )]
    );
    assert_eq!(csv.body, b"a,b\n");
    let none = Response::empty(204);
    assert_eq!(
        (none.status, &*none.content_type, none.body.len()),
        (204, "", 0)
    );
}

#[test]
fn errors_and_redirects_return_early() {
    fn guess(g: &str) -> Result<()> {
        if g.len() != 5 {
            return error(400, "A guess is five letters.");
        }
        redirect("/next")
    }
    let short = guess("abc").unwrap_err();
    assert_eq!(
        (short.status(), short.message()),
        (400, "A guess is five letters.")
    );
    assert_eq!(guess("crane").unwrap_err().status(), 303);
}

#[test]
fn redirects_and_channels() {
    let r = Response::redirect("/next");
    assert_eq!(r.status, 303);
    assert_eq!(r.headers, [("location".into(), "/next".to_string())]);
    // The channel forms stay for a body written from elsewhere.
    let (res, tx) = Response::channel("text/csv");
    assert_eq!((res.status, &*res.content_type), (200, "text/csv"));
    assert!(!tx.is_closed());
    let (events, _tx) = Response::event_channel();
    assert_eq!(&*events.content_type, "text/event-stream");
    assert_eq!(events.headers.len(), 2);
}
