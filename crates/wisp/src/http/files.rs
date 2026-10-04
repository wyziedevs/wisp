//! Built-in routes and embedded files.

use super::*;

/// Wisp's own addresses: the browser runtime, the API docs and, in dev,
/// the dev tools. `false` for any other path.
/// Out of line: its arms must not cost the routes that never reach it.
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
#[inline(never)]
pub(super) fn internal<A: App>(cx: &Cx, path: &str, reply: &mut Reply) -> bool {
    if !path.starts_with("/_") {
        return false;
    }
    let s = crate::settings();
    let get = matches!(cx.method, Method::Get | Method::Head);
    let dev = get && s.dev;
    let docs = get && s.api_docs && !A::openapi().is_empty();
    let (body, ext, etag): (&'static [u8], _, _) = match path {
        crate::protocol::route::WISP_JS_PATH if get => (CLIENT_JS, "js", Some(CLIENT_JS_ETAG)),
        crate::protocol::route::LIVE_JS_PATH if get => (LIVE_JS, "js", Some(CLIENT_JS_ETAG)),
        "/_app/wisp-dev.js" if dev => (DEV_JS, "js", None),
        "/_app/wisp-ui.css" if dev => (UI_CSS.as_bytes(), "css", None),
        "/_app/wisp-dialog.css" if dev => (DIALOG_CSS, "css", None),
        "/_wisp/openapi.json" if docs => (A::openapi().as_bytes(), "json", None),
        "/_wisp/client.ts" if docs => (A::client_ts().as_bytes(), "txt", None),
        "/_wisp/docs" if docs => (api_docs(), "html", None),
        crate::health::PATH if get => {
            crate::health::answer(reply);
            return true;
        }
        "/_wisp/metrics" if get && crate::obs::serve(cx, reply) => return true,
        // What a page that reads a `.live()` table listens to.
        _ if get && path.starts_with("/_wisp/live/") => {
            let Some(res) = crate::table::live_events(&path["/_wisp/live/".len()..]) else {
                return false;
            };
            put(cx, reply, res);
            return true;
        }
        #[cfg(debug_assertions)]
        "/_app/wisp-devtools.js" if dev => (DEVTOOLS_JS, "js", None),
        #[cfg(debug_assertions)]
        "/_wisp/dev/open" if s.dev && cx.method == Method::Post => {
            let (status, msg) = dev::open(A::ROOT, cx);
            reply.set_plain(status, msg);
            return true;
        }
        _ if s.dev && path.starts_with("/_wisp/") => {
            let (status, msg) = dev::endpoint::<A>(cx.method, path, cx.body(), cx.peer());
            reply.set_plain(status, msg);
            return true;
        }
        _ => return false,
    };
    send_file(reply, cx, Body::Static(body), ext, etag);
    if !A::ELEMENTS.is_empty() && path == crate::protocol::route::LIVE_JS_PATH {
        any_origin(reply);
    }
    true
}

/// The app's files: templates' browser modules, then its assets, embedded
/// or, in dev, read from `static/`. `route`: the route the path matches.
/// `false` if the path is not a file.
#[inline(always)]
pub(super) fn file<A: App>(cx: &Cx, raw: &[u8], route: Option<usize>, reply: &mut Reply) -> bool {
    let routed = route.is_some();
    // The service worker and the manifest, of an app with either.
    if let Some(p) = A::PWA {
        use crate::protocol::{MANIFEST_PATH, SERVICE_WORKER_PATH};
        let got = match cx.path() {
            SERVICE_WORKER_PATH if !p.worker.is_empty() => {
                Some((p.worker, "js", Some(p.worker_etag)))
            }
            MANIFEST_PATH => match p.manifest {
                Some("") => None,
                Some(m) => Some((m, "webmanifest", Some(p.manifest_etag))),
                None => crate::pwa::manifest().map(|m| (m, "webmanifest", None)),
            },
            _ => None,
        };
        if let Some((body, ext, etag)) = got {
            send_file(reply, cx, Body::Static(body.as_bytes()), ext, etag);
            return true;
        }
    }
    // One `wisp dev` swapped in, never cached: its URL names its version.
    #[cfg(debug_assertions)]
    if raw.starts_with(crate::protocol::route::MODULES.as_bytes())
        && let Some(source) = dev::module(cx.path())
    {
        let ext = if cx.path().ends_with(".map") {
            "json"
        } else {
            "js"
        };
        send_file(reply, cx, Body::Static(source.as_bytes()), ext, None);
        return true;
    }
    // Compiled in, in dev too: what `wisp dev` cannot swap is a rebuild.
    if raw.starts_with(crate::protocol::route::MODULES.as_bytes())
        && let Some(m) = A::client_module(cx.path())
    {
        // A module's source map (`t3.js.map`) is JSON.
        let ext = if m.path.ends_with(".map") {
            "json"
        } else {
            "js"
        };
        send_file(
            reply,
            cx,
            Body::Static(m.source.as_bytes()),
            ext,
            Some(m.etag),
        );
        if !A::ELEMENTS.is_empty() {
            any_origin(reply);
        }
        return true;
    }
    if crate::settings().dev {
        let path = cx.path();
        // A page's path goes to the disk only if `static/` had a file there.
        if path != crate::protocol::route::APP_CSS_PATH
            && !path.starts_with(crate::protocol::route::IMAGES)
            && routed
            && !dev::listed(A::ROOT, &decode(path.as_bytes(), false))
        {
            return false;
        }
        let Some((bytes, ext)) = dev::read_file(A::ROOT, path) else {
            return false;
        };
        send_file(reply, cx, Body::Bytes(bytes), &ext, None);
        return true;
    }
    // The build knows which routes no embedded file is at.
    if route.is_some_and(|r| !A::ROUTES[r].files) {
        return false;
    }
    let Some(a) = A::asset(cx.path()) else {
        return false;
    };
    send_file(reply, cx, Body::Static(a.body), a.ext, Some(a.etag));
    true
}

/// Lets a page of any site load this module: a custom element's
/// (`App::ELEMENTS`), and what it imports.
pub(super) fn any_origin(reply: &mut Reply) {
    reply.headers.push((
        Cow::Borrowed("access-control-allow-origin"),
        Cow::Borrowed("*"),
    ));
}

/// A file, cached by its `etag` (forever when the address is versioned:
/// with `?v=`, or an npm module's or an image's, whose path names its
/// version or hash), or never without one.
pub(super) fn send_file(
    reply: &mut Reply,
    cx: &Cx,
    body: Body,
    ext: &str,
    etag: Option<&'static str>,
) {
    let cache = match etag {
        None => "no-store",
        Some(_)
            if cx.query_string().split('&').any(|kv| kv.starts_with("v="))
                || cx.path().starts_with(crate::protocol::route::NPM_MODULES)
                || cx.path().starts_with(crate::protocol::route::IMAGES) =>
        {
            "public, max-age=31536000, immutable"
        }
        Some(_) => "public, max-age=0, must-revalidate",
    };
    if etag.is_some_and(|tag| fresh(cx, tag)) {
        reply.set(304, "", Body::Static(b""));
        reply.headers.clear(); // a 304 describes the file it did not send
    } else {
        reply.set(200, mime(ext), body);
        if ext.starts_with("htm") && crate::settings().secure_headers {
            for (name, value) in crate::headers::missing(cx, &[]) {
                reply
                    .headers
                    .push((Cow::Borrowed(name), Cow::Borrowed(value)));
            }
        }
        if !crate::range::apply(cx, reply, etag) {
            crate::compress::apply(cx, reply);
        }
    }
    if let Some(tag) = etag {
        reply
            .headers
            .push((Cow::Borrowed("etag"), Cow::Borrowed(tag)));
    }
    reply
        .headers
        .push((Cow::Borrowed("cache-control"), Cow::Borrowed(cache)));
}

pub(crate) fn mime(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Percent-decoded static file path, or `None` if it could escape `static/`.
pub(crate) fn safe_relative_path(path: &str) -> Option<String> {
    let decoded = decode(path.as_bytes(), false);
    let rel = decoded.strip_prefix('/')?;
    stays_inside(rel).then(|| rel.to_string())
}

/// Whether `rel`, joined to a folder, names something inside it: no `..`,
/// no empty segment, nothing a drive or an absolute path could use.
pub(crate) fn stays_inside(rel: &str) -> bool {
    rel.split('/')
        .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(['\\', ':', '\0']))
}
