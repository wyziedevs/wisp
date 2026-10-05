//! The environment and the server's settings: `env`, `env_or`, `address`,
//! and the `WISP_*` knobs read once at start.

use crate::*;

/// A variable from the host's environment, such as an API key: the
/// process's environment on a server, else `.env` in its working
/// directory (read once, at start); the worker's variables and secrets on
/// an edge host, which has no `.env`.
pub fn env(key: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return var(key, dotenv());
    #[cfg(target_arch = "wasm32")]
    return edge::env(key);
}

/// `key` from the process's environment, else from `file`'s lines.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn var(key: &str, file: &[(String, String)]) -> Option<String> {
    match std::env::var(key) {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotUnicode(v)) => Some(v.to_string_lossy().into_owned()),
        Err(std::env::VarError::NotPresent) => {
            file.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        }
    }
}

/// The variables of `.env` in the working directory, read once: the first
/// setting `run` reads reads it. A line that is not `KEY=value` is skipped,
/// with a warning; no file is none.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dotenv() -> &'static [(String, String)] {
    static VARS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    VARS.get_or_init(|| {
        let text = match std::fs::read_to_string(".env") {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                http::log(format_args!("wisp: .env is not read: {e}"));
                return Vec::new();
            }
        };
        let (vars, bad) = wisp_shared::dotenv::parse(&text);
        for n in bad {
            http::log(format_args!(
                "wisp: .env line {n} is not KEY=value, so it is skipped"
            ));
        }
        vars
    })
}

/// An environment variable parsed as any `FromStr` type, or `default` when
/// it is not set: `let workers: usize = wisp::env_or("WORKERS", 4);`. One
/// that is set but does not parse panics, naming it, so a typo is never
/// quietly replaced by the default (in `init`, that stops the server).
pub fn env_or<T: FromStr>(key: &str, default: T) -> T {
    match env(key) {
        None => default,
        Some(v) => match v.trim().parse() {
            Ok(v) => v,
            Err(_) => panic!(
                "{key} is {v:?}, which is not a {}",
                std::any::type_name::<T>()
            ),
        },
    }
}

/// `$HOST:$PORT`, with the defaults described in [`run`]. `HOST` is an IP
/// address or a name such as `localhost`.
/// Ends the process with a message if either is not valid.
pub fn address() -> SocketAddr {
    let port: u16 = setting("PORT", "a port number from 0 to 65535").unwrap_or(3000);
    let Some(host) = setting::<String>("HOST", "an address") else {
        let ip = if settings().dev {
            Ipv4Addr::LOCALHOST
        } else {
            Ipv4Addr::UNSPECIFIED
        };
        return SocketAddr::from((ip, port));
    };
    let bare = host.trim().trim_start_matches('[').trim_end_matches(']');
    match (bare, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut all| all.next())
    {
        Some(addr) => addr,
        None => fail(&format!(
            "HOST is {host:?}, which is not an IP address or a name that resolves to one\n  \
             Use 127.0.0.1 (this machine only), 0.0.0.0 (every IPv4 address), :: (every IPv6 one) or a name such as localhost."
        )),
    }
}

/// An environment setting: `None` when it is not set. One that is set but
/// is not a `T` ends the process, so a typo is never silently replaced by a
/// default.
pub(crate) fn setting<T: FromStr>(name: &str, what: &str) -> Option<T> {
    let value = env(name)?;
    match value.trim().parse() {
        Ok(v) => Some(v),
        _ => fail(&format!("{name} is {value:?}, which is not {what}")),
    }
}

/// Stops a server that cannot start, with a message (the reason, then what
/// to do indented under it).
pub(crate) fn fail(message: &str) -> ! {
    http::log(format_args!("wisp: {message}"));
    std::process::exit(1);
}

/// Settings read from the environment, once. `run` and `serve` read them
/// before serving, so a bad one stops the server at start.
pub(crate) struct Settings {
    /// `WISP_DEV`: dev mode (`on` in debug builds, `off` in release ones):
    /// 5xx details on error pages, files read from `static/` on each
    /// request, the dev log and tools, a dev secret for signed cookies.
    pub dev: bool,
    /// `WISP_BODY_LIMIT`: the largest request body a route takes unless it
    /// sets its own `BODY_LIMIT`.
    pub body_limit: usize,
    /// `ORIGIN`: the site's own address, as browsers see it
    /// (`https://example.com`), for a proxy that does not pass `Host` on.
    pub origin: Option<String>,
    /// `WISP_CLIENT_IP_HEADER`: where the proxy puts the client's address.
    pub client_ip_header: Option<String>,
    /// `WISP_SECRET`: signs cookies.
    pub secret: Option<String>,
    /// `WISP_SECRET_OLD`: the secret before, still accepted on cookies it
    /// signed while a new one takes over.
    pub old_secret: Option<String>,
    /// `WISP_WS_IDLE`: seconds a WebSocket client may stay quiet (60; 0
    /// never closes). It is pinged halfway.
    pub ws_idle: std::time::Duration,
    /// `WISP_MAX_CONNS`: open connections, WebSockets too, past which the
    /// built-in server answers new ones 503 and closes them (10000; 0 is
    /// no cap).
    #[cfg(not(target_arch = "wasm32"))] // no sockets there
    pub max_conns: usize,
    /// `WISP_API_DOCS`: serve `/_wisp/openapi.json` and `/_wisp/docs`
    /// (`on` in dev, `off` otherwise).
    pub api_docs: bool,
    /// `WISP_REQUEST_ID`: give every request an id, not only those that ask
    /// for one with `cx.request_id()` (`off`).
    pub request_id: bool,
    /// `WISP_SECURE_HEADERS`: `nosniff` and `referrer-policy` on pages (`on`).
    pub secure_headers: bool,
    /// Handlers are timed, for the dev log or `Server-Timing`: `dev` or
    /// `server_timing`, but not in the edge build.
    pub timed: bool,
    /// `WISP_SERVER_TIMING`: a `Server-Timing` header on every answer (`on`
    /// in dev, `off` otherwise).
    pub server_timing: bool,
    /// `WISP_HANDLER_TIMEOUT`, in milliseconds (0 for none).
    pub timeout_ms: u64,
    /// `WISP_PROBLEM_JSON`: JSON errors as RFC 9457 `application/problem+json`
    /// for every client, not only those whose `accept` asks for it (`off`).
    pub problem_json: bool,
}

/// An `on`/`off` setting, `default` when it is not set.
pub(crate) fn switch(name: &str, default: bool) -> bool {
    match setting::<String>(name, "on or off").map(|v| v.to_ascii_lowercase()) {
        None => default,
        Some(v) if matches!(&*v, "on" | "1" | "true") => true,
        Some(v) if !input::on(&v) => false,
        Some(v) => fail(&format!("{name} is {v:?}, which is not on or off")),
    }
}

pub(crate) fn settings() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let body_limit = setting::<Size>("WISP_BODY_LIMIT", "a size such as 1048576, 512KB or 10MB").map_or(MB, |s| s.0);
        let origin = setting::<String>("ORIGIN", "an address").map(|o| {
            let o = o.trim_end_matches('/').to_ascii_lowercase();
            let host = o.strip_prefix("https://").or_else(|| o.strip_prefix("http://"));
            if host.is_none_or(|h| h.is_empty() || h.contains(['/', '?', '#', '@', ' '])) {
                fail(&format!("ORIGIN is {o:?}, which is not a site's address\n  Use the address visitors type, such as https://example.com, with no path."));
            }
            o
        });
        let client_ip_header = setting::<String>("WISP_CLIENT_IP_HEADER", "a header name").map(|h| h.to_ascii_lowercase());
        let secret = |name: &str| {
            let s = setting::<String>(name, "a secret")?;
            if s.len() < 32 {
                fail(&format!(
                    "{name} is {} characters, too short to keep signed cookies safe\n  Use at least 32 random ones: `openssl rand -hex 32` makes some.",
                    s.len()
                ));
            }
            Some(s)
        };
        let (secret, old_secret) = (secret("WISP_SECRET"), secret("WISP_SECRET_OLD"));
        let ws_idle = std::time::Duration::from_secs(setting::<u64>("WISP_WS_IDLE", "a number of seconds").unwrap_or(60));
        #[cfg(not(target_arch = "wasm32"))]
        let max_conns = match setting::<usize>("WISP_MAX_CONNS", "a number of connections") {
            Some(0) => usize::MAX,
            n => n.unwrap_or(10_000),
        };
        let dev = switch("WISP_DEV", cfg!(debug_assertions));
        let api_docs = switch("WISP_API_DOCS", dev);
        let request_id = switch("WISP_REQUEST_ID", false);
        let problem_json = switch("WISP_PROBLEM_JSON", false);
        let secure_headers = switch("WISP_SECURE_HEADERS", true);
        // `WISP_HSTS`: `strict-transport-security` on every answer (`off`).
        headers::HSTS_ON.store(switch("WISP_HSTS", false), std::sync::atomic::Ordering::Relaxed);
        let server_timing = switch("WISP_SERVER_TIMING", dev);
        let timed = cfg!(not(target_arch = "wasm32")) && (dev || server_timing);
        let timeout_ms = setting::<u64>("WISP_HANDLER_TIMEOUT", "a number of seconds").map_or(0, |s| s.saturating_mul(1000));
        Settings {
            dev, body_limit, origin, client_ip_header, secret, old_secret, api_docs, request_id, problem_json, secure_headers, timed, server_timing, timeout_ms,
            ws_idle,
            #[cfg(not(target_arch = "wasm32"))]
            max_conns,
        }
    })
}

/// A byte count, written `1048576`, `512KB`, `10MB` or `1GB` (powers of 1024).
pub(crate) struct Size(usize);

impl FromStr for Size {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Size, ()> {
        let s = s.trim().to_ascii_uppercase();
        let (digits, unit) = [("GB", 1 << 30), ("MB", MB), ("KB", KB), ("B", 1)]
            .into_iter()
            .find_map(|(suffix, unit)| Some((s.strip_suffix(suffix)?, unit)))
            .unwrap_or((&s, 1));
        digits
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_mul(unit))
            .map(Size)
            .ok_or(())
    }
}
