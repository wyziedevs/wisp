//! [`Error`], an HTTP error or redirect, and the helpers that make one.

use crate::*;

/// An HTTP error or redirect. `?` converts any `std::error::Error` into a
/// 500 whose details are shown only in dev builds.
pub struct Error {
    pub(crate) status: u16,
    pub(crate) message: Cow<'static, str>,
    /// `location` for redirects, `allow` for 405, `retry-after` for 429.
    pub(crate) header: Option<Box<(&'static str, String)>>,
    pub(crate) source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// For a 422: what is wrong, by field.
    pub(crate) fields: Vec<(String, String)>,
    /// What an API client matches on, from [`Error::with_code`]; else one
    /// for the status.
    pub(crate) code: Option<&'static str>,
}

impl Error {
    /// An error with an HTTP `status` and a `message` shown to the visitor: `Error::new(403, "not yours")`.
    pub fn new(status: u16, message: impl Into<Cow<'static, str>>) -> Error {
        assert!(
            (400..=599).contains(&status),
            "error status must be 4xx or 5xx, got {status}"
        );
        Error::raw(status, message.into())
    }

    /// An error of any status, a redirect's too: [`Error::new`] checks it.
    pub(crate) fn raw(status: u16, message: Cow<'static, str>) -> Error {
        Error {
            status,
            message,
            header: None,
            source: None,
            fields: Vec::new(),
            code: None,
        }
    }

    /// A code for API clients to match on, in place of the status's own
    /// (`not_found`, `invalid`...): `Error::new(409, "Taken").with_code("email_taken")`.
    pub fn with_code(mut self, code: &'static str) -> Error {
        self.code = Some(code);
        self
    }

    /// The error's code: the one given, or the status's: `bad_request`
    /// `unauthorized` `forbidden` `not_found` `method_not_allowed`
    /// `conflict` `precondition_failed` `too_large` `unsupported_media_type`
    /// `invalid` `rate_limited` `internal` `unavailable`...
    pub fn code(&self) -> &'static str {
        self.code.unwrap_or(match self.status {
            400 => "bad_request",
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not_found",
            405 => "method_not_allowed",
            406 => "not_acceptable",
            408 | 504 => "timeout",
            409 => "conflict",
            410 => "gone",
            412 => "precondition_failed",
            413 => "too_large",
            415 => "unsupported_media_type",
            422 => "invalid",
            428 => "precondition_required",
            429 => "rate_limited",
            431 => "headers_too_large",
            500 => "internal",
            501 => "not_implemented",
            502 => "bad_gateway",
            503 => "unavailable",
            s if s < 500 => "client_error",
            _ => "server_error",
        })
    }

    /// A 422 for input that does not pass: `field` and what is wrong with
    /// it. [`invalid`] returns one; [`Error::and`] adds more.
    pub fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Error {
        Error::invalid_fields(vec![(field.into(), problem.into())])
    }

    /// Another field that does not pass, on a 422 from [`Error::invalid`].
    pub fn and(mut self, field: impl Into<String>, problem: impl Into<String>) -> Error {
        self.fields.push((field.into(), problem.into()));
        self.message = Cow::Owned(Error::summary(&self.fields));
        self
    }

    pub(crate) fn invalid_fields(fields: Vec<(String, String)>) -> Error {
        Error {
            message: Cow::Owned(Error::summary(&fields)),
            fields,
            ..Error::new(422, "")
        }
    }

    /// `title: must have at least 1 character; age: is required`.
    fn summary(fields: &[(String, String)]) -> String {
        let all: Vec<String> = fields.iter().map(|(f, p)| format!("{f}: {p}")).collect();
        all.join("; ")
    }

    /// What is wrong, by field, for a 422 about input: `("title", "is required")`.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    /// A header to send with the error, such as `retry-after` on a 429.
    /// A single-valued one (`content-type`, `cache-control`, `location`,
    /// `etag`) replaces the error page's own. Panics on CR/LF in the value.
    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Error {
        let value = value.into();
        assert!(
            codec::valid_header(name, &value),
            "invalid header {name:?}: {value:?}"
        );
        self.header = Some(Box::new((name, value)));
        self
    }

    /// The error as JSON, for an API client: `{"status":422,"code":"invalid",
    /// "error":"...","errors":{"title":"is required"}}` (`errors` only when
    /// there are some). As a `problem` (RFC 9457) it is `{"type":"about:blank",
    /// "title":"Unprocessable Content","status":422,"detail":"...",…}`.
    pub(crate) fn json(&self, message: &str, problem: bool) -> String {
        let mut out = String::with_capacity(96);
        if problem {
            out.push_str("{\"type\":\"about:blank\",\"title\":");
            http::reason(self.status).json(&mut out);
            out.push(',');
        } else {
            out.push('{');
        }
        out.push_str("\"status\":");
        self.status.json(&mut out);
        out.push_str(",\"code\":");
        self.code().json(&mut out);
        out.push_str(if problem {
            ",\"detail\":"
        } else {
            ",\"error\":"
        });
        message.json(&mut out);
        if !self.fields.is_empty() {
            out.push_str(",\"errors\":{");
            for (k, (field, problem)) in self.fields.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                field.json(&mut out);
                out.push(':');
                problem.json(&mut out);
            }
            out.push('}');
        }
        out.push('}');
        out
    }

    /// A redirect with a status other than [`redirect`]'s 303, such as 308
    /// for a page that moved for good: `return Err(Error::redirect(308, "/new"))`.
    /// A `location` with CR/LF (header injection) is refused, not sent: the
    /// visitor gets a 500 and the reason is logged. A status outside
    /// 300..=308 is logged with the location and becomes 303 See Other.
    /// A path stays on the site: `//evil.example` or `/\evil.example`
    /// (another site to a browser) is sent as `/evil.example`, so
    /// `redirect(next)` from a `?next=` that starts with `/` is no open
    /// redirect. Another site is named with its scheme: `https://…`.
    pub fn redirect(status: u16, location: impl Into<String>) -> Error {
        let location = location.into();
        let status = match (300..=308).contains(&status) {
            true => status,
            false => {
                crate::http::log(format_args!(
                    "wisp: redirect status {status} to {location:?} is not 3xx, sent 303"
                ));
                303
            }
        };
        if !codec::valid_header("location", &location) {
            crate::http::log(format_args!(
                "wisp: refused redirect {status} to {location:?}: CR/LF in the location"
            ));
            return Error::raw(500, Cow::Borrowed("Internal Server Error"));
        }
        // A path stays one: browsers read `//host`, `/\host` (and a tab in
        // between, which they drop) as another site, so an app that checked
        // `next.starts_with('/')` would send visitors there (open redirect).
        let location = match location.starts_with(['/', '\\']) {
            true => {
                let rest = location.trim_start_matches(['/', '\\', '\t', '\n', '\r', ' ']);
                match rest.len() + 1 == location.len() && location.starts_with('/') {
                    true => location,
                    false => format!("/{rest}"),
                }
            }
            false => location,
        };
        // A path of the app's own is under its base path, when it has one.
        let location = match crate::protocol::BASE.is_empty() {
            true => location,
            false => crate::protocol::based(&location).into_owned(),
        };
        Error::raw(status, Cow::Borrowed("")).with_header("location", location)
    }

    /// The HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The message shown to the visitor.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The message and, when it came from another error, that error: for
    /// logs, never for pages.
    pub(crate) fn detail(&self) -> String {
        match &self.source {
            Some(s) => format!("{} ({s:?})", self.message),
            None => self.message.to_string(),
        }
    }
}

/// `return error(404, "No such post")`: stops the `load`, action or
/// endpoint, and the nearest `+error.wisp` shows `message`.
/// [`Error::new`] is the error itself, for `map_err` and the like.
pub fn error<T>(status: u16, message: impl Into<Cow<'static, str>>) -> Result<T> {
    Err(Error::new(status, message))
}

/// `return invalid("email", "is already taken")`: a 422 for input that does
/// not pass, by field, as `#[validate]` gives one. An API client gets
/// `{"errors":{"email":"is already taken"}}`; [`Error::and`] adds fields.
pub fn invalid<T>(field: impl Into<String>, problem: impl Into<String>) -> Result<T> {
    Err(Error::invalid(field, problem))
}

/// `return redirect("/login")`: 303 See Other, which sends the browser
/// to `location` with a GET, whether it came with a form post or a link.
/// [`Error::redirect`] takes other statuses. CR/LF in `location` is refused
/// with a 500, not a panic.
pub fn redirect<T>(location: impl Into<String>) -> Result<T> {
    Err(Error::redirect(303, location))
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(e: E) -> Error {
        let message = Cow::Owned(e.to_string());
        Error {
            source: Some(Box::new(e)),
            ..Error::raw(500, message)
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.status, self.message)?;
        if let Some((name, value)) = self.header.as_deref() {
            write!(f, " ({name}: {value})")?;
        }
        if let Some(s) = &self.source {
            write!(f, " ({s:?})")?;
        }
        Ok(())
    }
}

/// Turns `Option`/`Result` into HTTP errors: `db.find(id).await?.or_404()?`.
pub trait OrStatus<T> {
    /// `Err` with `status` (and a default message) when `self` is `None` or `Err`: `x.or_status(409)?`.
    fn or_status(self, status: u16) -> Result<T>;

    /// `or_status(404)`.
    fn or_404(self) -> Result<T>
    where
        Self: Sized,
    {
        self.or_status(404)
    }
}

impl<T> OrStatus<T> for Option<T> {
    fn or_status(self, status: u16) -> Result<T> {
        self.ok_or_else(|| Error::new(status, http::reason(status)))
    }
}

impl<T, E: fmt::Display> OrStatus<T> for std::result::Result<T, E> {
    fn or_status(self, status: u16) -> Result<T> {
        self.map_err(|e| Error::new(status, e.to_string()))
    }
}
