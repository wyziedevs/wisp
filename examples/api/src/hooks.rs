/// What the app reads from its environment once, at start.
pub struct Config {
    /// Needed, as a bearer token, to change anything.
    pub api_key: String,
}

/// Changes allowed per client, a minute.
static WRITES: RateLimit = RateLimit::per_minute(60);

fn init() {
    // `API_KEY=... wisp dev`; the default is for trying it out.
    let api_key = wisp::env("API_KEY").unwrap_or_else(|| "dev-key".into());
    wisp::provide(Config { api_key });
}

fn before(cx: &mut Cx) -> Result<Option<Response>> {
    // Any site's pages may call the API from the browser.
    if let Some(preflight) = cx.cors("*") {
        return Ok(Some(preflight));
    }
    let reads = matches!(cx.method, Method::Get | Method::Head | Method::Options);
    if !reads && cx.path().starts_with("/api/") {
        WRITES.check(cx.client_ip())?;
        let key = &wisp::state::<Config>().api_key;
        if !cx.bearer().is_some_and(|token| wisp::secure_eq(token, key)) {
            let e = Error::new(401, "Send the API key: Authorization: Bearer <key>");
            return Err(e.with_header("www-authenticate", "Bearer"));
        }
    }
    Ok(None)
}
