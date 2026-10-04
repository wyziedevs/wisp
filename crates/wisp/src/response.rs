//! [`Response`], a complete answer, and what streams one.

use crate::*;

/// A complete response: from a `+server.rs` endpoint, or from an action or
/// the `before` hook, in place of the page.
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// The `Content-Type` header.
    pub content_type: Cow<'static, str>,
    /// Other headers, as `(name, value)`.
    pub headers: Vec<(Cow<'static, str>, String)>,
    /// The body.
    pub body: Vec<u8>,
    /// For [`Response::stream`]: the body, as it is made.
    pub(crate) stream: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    /// For [`Response::websocket`]: what runs once upgraded.
    pub(crate) upgrade: Option<ws::Upgrade>,
    /// Made by [`Response::html`]: it gets the page's security headers.
    pub(crate) page: bool,
}

impl Response {
    /// A `200` with `content_type` and `body`: `Response::new("image/png", bytes)`.
    pub fn new(content_type: impl Into<Cow<'static, str>>, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status: 200,
            content_type: content_type.into(),
            headers: Vec::new(),
            body: body.into(),
            stream: None,
            upgrade: None,
            page: false,
        }
    }

    /// A response whose body is sent while it is being made: a live feed,
    /// a large export. `body` writes it, in a task of its own:
    ///
    /// ```ignore
    /// Response::stream("text/csv", |out| async move {
    ///     for row in rows().await {
    ///         out.send(row.to_csv()).await?; // stops once the client has left
    ///     }
    ///     Ok(())
    /// })
    /// ```
    ///
    /// Each `send` goes out at once; the body ends when `body` returns, or
    /// when the server stops.
    pub fn stream<F, Fut>(content_type: impl Into<Cow<'static, str>>, body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        let (res, tx) = Response::channel(content_type);
        let task = body(tx);
        spawn(async move {
            let _ = task.await; // `Gone`: the client left, which ends it too
        });
        res
    }

    /// Newline-delimited JSON (`application/x-ndjson`), a value per line,
    /// sent as it is made: a big list without holding all of it.
    ///
    /// ```ignore
    /// fn get() -> Response {
    ///     Response::ndjson(|out| async move {
    ///         for note in db::notes().await {
    ///             out.line(&note).await?;
    ///         }
    ///         Ok(())
    ///     })
    /// }
    /// ```
    pub fn ndjson<F, Fut>(body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        Response::stream("application/x-ndjson", body)
    }

    /// A streamed response, and the [`Sender`] that writes its body until
    /// it is dropped.
    pub(crate) fn channel(content_type: impl Into<Cow<'static, str>>) -> (Response, Sender) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut res = Response::new(content_type, Vec::new());
        res.stream = Some(rx);
        (res, Sender(tx))
    }

    /// No body, only a status (and the headers added to it):
    /// `Response::empty(204)`.
    pub fn empty(status: u16) -> Response {
        Response::new("", Vec::new()).with_status(status)
    }

    /// `body` as a file the browser saves as `name`, typed by its extension:
    /// `Response::download("report.csv", csv)`.
    pub fn download(name: &str, body: impl Into<Vec<u8>>) -> Response {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        // The name goes in a quoted header value: nothing in it may end the quotes or the line.
        let safe: String = name
            .chars()
            .map(|c| {
                if c == '"' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        Response::new(http::mime(&ext), body).with_header(
            "content-disposition",
            format!("attachment; filename=\"{safe}\""),
        )
    }

    /// The file `name` in the directory `dir`, such as an upload saved
    /// earlier, typed by its extension:
    /// `Response::file_in("uploads", cx.param("name")).await?`.
    ///
    /// `name` may come from the URL: one that would reach outside `dir`
    /// (`..`, an absolute path, a drive) is a 404, as is a file that does not
    /// exist. It is read without blocking the thread.
    pub async fn file_in(dir: impl AsRef<std::path::Path>, name: &str) -> Result<Response> {
        if !http::stays_inside(name) {
            return Err(Error::new(404, "Not Found"));
        }
        let path = dir.as_ref().join(name);
        // Edge hosts have no files: there it is the 500 of an unsupported
        // operation.
        #[cfg(not(target_arch = "wasm32"))]
        let read = tokio::fs::read(&path).await;
        #[cfg(target_arch = "wasm32")]
        let read = std::fs::read(&path);
        match read {
            Ok(body) => {
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                Ok(Response::new(http::mime(&ext), body))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::IsADirectory
                        | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Err(Error::new(404, "Not Found"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Server-sent events, which a page receives with `listen(url, …)` or
    /// `new EventSource(url)`: a stream that no proxy or browser caches or
    /// holds back. `body` sends them, each with [`Sender::event`], in a task
    /// of its own:
    ///
    /// ```ignore
    /// Response::events(|out| async move {
    ///     loop {
    ///         out.event(&now()).await?; // stops once the client has left
    ///         wisp::sleep(Duration::from_secs(1)).await;
    ///     }
    /// })
    /// ```
    pub fn events<F, Fut>(body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        // `x-accel-buffering` stops nginx from holding events back.
        Response::stream("text/event-stream", body)
            .with_header("cache-control", "no-store")
            .with_header("x-accel-buffering", "no")
    }

    /// A `200` with `text/plain; charset=utf-8`: `Response::text("ok")`.
    pub fn text(body: impl IntoText) -> Response {
        Response::new("text/plain; charset=utf-8", body.into_text())
    }

    /// A `200` with `text/html; charset=utf-8`: `Response::html("<p>hi</p>")`. Not escaped.
    pub fn html(body: impl IntoText) -> Response {
        let mut res = Response::new("text/html; charset=utf-8", body.into_text());
        res.page = true;
        res
    }

    /// Serialize with whatever you like; this only sets the content type.
    pub fn json(body: impl IntoText) -> Response {
        Response::new("application/json", body.into_text())
    }

    /// `value` as JSON, with `#[derive(Json)]` or one of the built-in impls.
    pub fn json_of(value: &(impl Json + ?Sized)) -> Response {
        let mut out = String::from_utf8(http::spare()).unwrap_or_default();
        value.json(&mut out);
        Response::json(out)
    }

    /// `value` as JSON with 201 Created, the answer to a POST that made
    /// something: `Response::created(&note)`.
    pub fn created(value: &(impl Json + ?Sized)) -> Response {
        Response::json_of(value).with_status(201)
    }

    /// The same response with `status` (100 to 999; panics outside it): `Response::text("gone").with_status(410)`.
    pub fn with_status(mut self, status: u16) -> Response {
        assert!((100..=999).contains(&status), "invalid status {status}");
        self.status = status;
        self
    }

    /// The same response with a header: `.with_header("x-robots-tag", "none")`.
    /// A single-valued one (`content-type`, `cache-control`, `location`,
    /// `etag`, any case) replaces what was set before instead of going out
    /// twice, so `Response::text(xml).with_header("content-type",
    /// "application/rss+xml")` sends one `content-type`. `content-length`
    /// and `transfer-encoding` are the server's and are left out.
    /// Panics on CR/LF in the value (header injection).
    pub fn with_header(
        mut self,
        name: impl Into<Cow<'static, str>>,
        value: impl Into<String>,
    ) -> Response {
        let (name, value) = (name.into(), value.into());
        assert!(
            cx::valid_header(&name, &value),
            "invalid header {name:?}: {value:?}"
        );
        if name.eq_ignore_ascii_case("content-type") {
            self.content_type = Cow::Owned(value);
            return self;
        }
        if crate::headers::single(&name) {
            self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        }
        self.headers.push((name, value));
        self
    }
}

/// The text of [`Response::text`], [`Response::html`] and
/// [`Response::json`]. A `String` becomes the body as it is; borrowed text
/// is copied into a buffer that an earlier response on the thread left, so
/// a warm server allocates nothing for it.
#[diagnostic::on_unimplemented(
    message = "a response's text is a `String` or a `&str`, not `{Self}`"
)]
pub trait IntoText {
    /// The bytes of the body.
    fn into_text(self) -> Vec<u8>;
}

impl IntoText for String {
    fn into_text(self) -> Vec<u8> {
        self.into_bytes()
    }
}

impl IntoText for &str {
    fn into_text(self) -> Vec<u8> {
        let mut body = http::spare();
        body.extend_from_slice(self.as_bytes());
        body
    }
}

impl IntoText for &mut str {
    fn into_text(self) -> Vec<u8> {
        (&*self).into_text()
    }
}

impl IntoText for &String {
    fn into_text(self) -> Vec<u8> {
        self.as_str().into_text()
    }
}

impl IntoText for Box<str> {
    fn into_text(self) -> Vec<u8> {
        self.into_string().into_bytes()
    }
}

impl IntoText for Cow<'_, str> {
    fn into_text(self) -> Vec<u8> {
        match self {
            Cow::Owned(s) => s.into_bytes(),
            Cow::Borrowed(s) => s.into_text(),
        }
    }
}

impl IntoText for char {
    fn into_text(self) -> Vec<u8> {
        self.encode_utf8(&mut [0; 4]).into_text()
    }
}

/// Writes the body of a [`Response::stream`].
pub struct Sender(pub(crate) tokio::sync::mpsc::Sender<Vec<u8>>);

impl Sender {
    /// Sends `chunk` to the client. Fails once the client has gone (or the
    /// server is stopping): stop making the body then.
    pub async fn send(&self, chunk: impl Into<Vec<u8>>) -> Result<(), Gone> {
        self.0.send(chunk.into()).await.map_err(|_| Gone)
    }

    /// Sends one server-sent event, which the page's `EventSource` gets as a
    /// `message` whose `data` is `data`. Each line of it is sent as its own
    /// `data:` field, as the format requires, so any text arrives whole.
    pub async fn event(&self, data: &str) -> Result<(), Gone> {
        self.send(event_text(data)).await
    }

    /// Sends `value` as JSON and a newline: a line of [`Response::ndjson`].
    pub async fn line(&self, value: &(impl Json + ?Sized)) -> Result<(), Gone> {
        let mut out = String::with_capacity(64);
        value.json(&mut out);
        out.push('\n');
        self.send(out).await
    }

    /// Whether the client has gone, so nothing more can reach it.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// `data` as one server-sent event: a `data:` field per line. A line ends
/// at CR LF, LF or a CR alone, as the browser reads it, so text with a CR
/// in it cannot start a field of its own (`event:`, `id:`, `retry:`).
pub(crate) fn event_text(data: &str) -> String {
    let mut out = String::with_capacity(data.len() + 8);
    let mut rest = data;
    loop {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        out.push_str("data: ");
        out.push_str(&rest[..end]);
        out.push('\n');
        if end == rest.len() {
            break;
        }
        let eol = if rest[end..].starts_with("\r\n") {
            2
        } else {
            1
        };
        rest = &rest[end + eol..];
    }
    out.push('\n');
    out
}

/// The client of a streamed response has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl fmt::Display for Gone {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("the client has gone")
    }
}

impl std::error::Error for Gone {}
