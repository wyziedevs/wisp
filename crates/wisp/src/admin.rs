//! An admin page for the app's saved tables (`Table::saved`, `#[derive(Rest)]`
//! types): list, edit a row's JSON, delete. Off unless `WISP_ADMIN_KEY` is
//! set; then `/_wisp/admin` asks for HTTP basic auth with any user name and
//! that key, and the tables are the ones started with the server.
//!
//! It shows and changes the rows themselves: no hooks (`before_update`,
//! `#[validate]`) run, only that the JSON is a row of the table's type
//! (and, for a unique field, free). Keep the key to people who may do that.

#[cfg(not(target_arch = "wasm32"))]
use crate::RateLimit;
use crate::http::{TOKENS_CSS, UI_CSS};
use crate::{Cx, Method, Response, Result};
use std::fmt::Write;

/// Where the admin page is.
pub const PATH: &str = "/_wisp/admin";

/// A saved table, whatever its row type, as the admin page reads it.
pub(crate) trait Admin: Sync {
    fn name(&self) -> &'static str;
    /// How many rows there are.
    fn len(&self) -> usize;
    /// Up to `limit` rows with ids past `after`, each as its id and JSON.
    fn rows(&self, after: u64, limit: usize) -> Vec<(u64, String)>;
    /// Row `id`'s JSON.
    fn row(&self, id: u64) -> Option<String>;
    /// Replaces row `id` with the row `json` is; `false` when there is none.
    fn put(&self, id: u64, json: &str) -> Result<bool>;
    /// Removes row `id`.
    fn drop_row(&self, id: u64) -> Result<bool>;
}

static TABLES: crate::Shared<Vec<&'static dyn Admin>> = crate::Shared::new(Vec::new());

/// Whether the admin page is on.
pub(crate) fn enabled() -> bool {
    crate::env("WISP_ADMIN_KEY").is_some_and(|k| !k.is_empty())
}

pub(crate) fn register(table: &'static dyn Admin) {
    TABLES.lock().push(table);
}

/// The admin page's answer for a request to `path`; `None` when the path is
/// not under `/_wisp/admin` or the page is off.
pub(crate) fn serve(cx: &Cx, path: &str) -> Option<Response> {
    static KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let key = KEY
        .get_or_init(|| crate::env("WISP_ADMIN_KEY").filter(|k| !k.is_empty()))
        .as_deref()?;
    let rest = path.strip_prefix(PATH)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    // Before the key is looked at, so guessing it is slow whoever is right.
    #[cfg(not(target_arch = "wasm32"))]
    {
        static TRIES: RateLimit = RateLimit::per_minute(60);
        if let Some(refused) = tries(&TRIES, cx) {
            return Some(refused);
        }
    }
    Some(answer(cx, rest.trim_matches('/'), key))
}

/// The answer to a client that has used up its requests, if it has.
#[cfg(not(target_arch = "wasm32"))]
fn tries(limit: &RateLimit, cx: &Cx) -> Option<Response> {
    limit.check(cx.client_ip()).err().map(|_| {
        page(429, "Slow down", "<p>Too many requests; wait a minute.</p>")
            .with_header("retry-after", "60")
    })
}

fn answer(cx: &Cx, rest: &str, key: &str) -> Response {
    let ok = cx
        .basic_auth()
        .is_some_and(|(_, pw)| crate::secure_eq(&pw, key.as_bytes()));
    if !ok {
        return page(401, "Sign in", "<p>The admin key, as the password.</p>").with_header(
            "www-authenticate",
            "Basic realm=\"wisp admin\", charset=\"UTF-8\"",
        );
    }
    let post = cx.method == Method::Post;
    if post && !same_site(cx) {
        return page(403, "Refused", "<p>A form from another site.</p>");
    }
    if !matches!(cx.method, Method::Get | Method::Head | Method::Post) {
        return page(405, "Not allowed", "<p>GET or POST.</p>");
    }
    let parts: Vec<&str> = if rest.is_empty() {
        vec![]
    } else {
        rest.split('/').collect()
    };
    let tables = TABLES.lock().clone();
    let table = |name: &str| tables.iter().find(|t| t.name() == name).copied();
    match (parts.as_slice(), post) {
        ([], false) => index(&tables),
        ([t], false) => table(t).map_or_else(not_found, |t| rows(cx, t)),
        ([t, id], false) => match (table(t), id.parse()) {
            (Some(t), Ok(id)) => edit(t, id, None),
            _ => not_found(),
        },
        ([t, id], true) => match (table(t), id.parse()) {
            (Some(t), Ok(id)) => save(cx, t, id),
            _ => not_found(),
        },
        ([t, id, "delete"], true) => match (table(t), id.parse()) {
            (Some(t), Ok(id)) => match t.drop_row(id) {
                Ok(_) => back(&format!("{PATH}/{}", t.name())),
                Err(e) => page(500, "Not deleted", &escaped(e.message())),
            },
            _ => not_found(),
        },
        _ => not_found(),
    }
}

/// Whether a form post comes from this site: its `Origin` is the host asked
/// for. A browser sends the basic-auth credentials along with a form from
/// anywhere, so this is what keeps another site from posting as the admin.
fn same_site(cx: &Cx) -> bool {
    let host = cx.header("host");
    match cx.header("origin") {
        Some(o) => o.split_once("://").is_some_and(|(_, h)| Some(h) == host),
        None => cx.header("sec-fetch-site") == Some("same-origin"),
    }
}

fn escaped(s: &str) -> String {
    let mut out = String::new();
    crate::html::text(&mut out, s);
    out
}

fn back(to: &str) -> Response {
    Response::empty(303).with_header("location", to)
}

fn not_found() -> Response {
    page(
        404,
        "Not found",
        &format!("<p><a href=\"{PATH}\">Tables</a></p>"),
    )
}

fn page(status: u16, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <meta name=robots content=noindex><title>{title}</title>\
         <style>{TOKENS_CSS}{UI_CSS}         body{{margin:0 auto;max-width:60rem;padding:var(--wisp-space-m);background:var(--wisp-paper);color:var(--wisp-ink);font:400 1rem/1.5 var(--wisp-sans)}}         table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid var(--wisp-line);padding:.4rem;text-align:left;vertical-align:top}}         code,textarea{{font:.8125rem/1.5 var(--wisp-mono)}}code{{word-break:break-all}}a{{color:var(--wisp-accent-hover)}}         textarea{{width:100%;min-height:16rem;box-sizing:border-box;padding:.5rem;border:1px solid var(--wisp-line);border-radius:var(--wisp-radius);background:var(--wisp-inset);color:var(--wisp-ink)}}         .problem{{color:var(--wisp-danger)}}</style>         <h1>{title}</h1>{body}"
    );
    Response::html(html)
        .with_status(status)
        .with_header("cache-control", "no-store")
}

fn index(tables: &[&'static dyn Admin]) -> Response {
    let mut body = String::from("<table><tr><th>Table<th>Rows");
    for t in tables {
        let name = escaped(t.name());
        let _ = write!(
            body,
            "<tr><td><a href=\"{PATH}/{name}\">{name}</a><td>{}",
            t.len()
        );
    }
    body.push_str("</table>");
    if tables.is_empty() {
        body.push_str("<p>No saved tables yet. One shows once the server has started with it.</p>");
    }
    page(200, "Tables", &body)
}

/// Rows a page lists.
const PAGE: usize = 100;

fn rows(cx: &Cx, t: &dyn Admin) -> Response {
    let name = escaped(t.name());
    let mut body = format!("<p><a href=\"{PATH}\">Tables</a></p><table><tr><th>Id<th>Row<th>");
    let after = cx.query("after").and_then(|a| a.parse().ok()).unwrap_or(0);
    let mut list = t.rows(after, PAGE + 1);
    let more = list.len() > PAGE;
    list.truncate(PAGE);
    for (id, json) in &list {
        let short: String = json.chars().take(120).collect();
        let more = if short.len() < json.len() { "…" } else { "" };
        let _ = write!(
            body,
            "<tr><td>{id}<td><code>{}{more}</code><td><a href=\"{PATH}/{name}/{id}\">Edit</a> \
             <form method=post action=\"{PATH}/{name}/{id}/delete\" style=display:inline>\
             <button class=\"wisp-button wisp-ghost\" onclick=\"return confirm('Delete row {id}?')\">Delete</button></form>",
            escaped(&short)
        );
    }
    body.push_str("</table>");
    if let (true, Some((last, _))) = (more, list.last()) {
        let _ = write!(
            body,
            "<p><a href=\"{PATH}/{name}?after={last}\">More</a></p>"
        );
    }
    page(200, &name, &body)
}

fn edit(t: &dyn Admin, id: u64, problem: Option<(&str, &str)>) -> Response {
    let name = escaped(t.name());
    let Some(json) = t.row(id) else {
        return not_found();
    };
    let (shown, error) = match problem {
        Some((sent, why)) => (
            sent.to_owned(),
            format!("<p class=problem>{}</p>", escaped(why)),
        ),
        None => (json, String::new()),
    };
    let body = format!(
        "<p><a href=\"{PATH}/{name}\">{name}</a></p>{error}\
         <form method=post action=\"{PATH}/{name}/{id}\"><textarea name=json spellcheck=false>{}</textarea>\
         <p><button class=\"wisp-button wisp-primary\">Save</button></p></form>",
        escaped(&shown)
    );
    page(
        if problem.is_some() { 422 } else { 200 },
        &format!("{name} {id}"),
        &body,
    )
}

fn save(cx: &Cx, t: &dyn Admin, id: u64) -> Response {
    let json = cx.form().get("json").unwrap_or_default();
    match t.put(id, &json) {
        Ok(true) => back(&format!("{PATH}/{}", t.name())),
        Ok(false) => not_found(),
        Err(e) => edit(t, id, Some((&json, e.message()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(std::sync::Mutex<Vec<(u64, String)>>);

    impl Admin for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn len(&self) -> usize {
            self.0.lock().unwrap().len()
        }
        fn rows(&self, after: u64, limit: usize) -> Vec<(u64, String)> {
            let all = self.0.lock().unwrap();
            all.iter()
                .filter(|r| r.0 > after)
                .take(limit)
                .cloned()
                .collect()
        }
        fn row(&self, id: u64) -> Option<String> {
            (self.0.lock().unwrap().iter().find(|r| r.0 == id)).map(|r| r.1.clone())
        }
        fn put(&self, id: u64, json: &str) -> Result<bool> {
            if !json.starts_with('{') {
                return crate::invalid("row", "is not an object");
            }
            let mut rows = self.0.lock().unwrap();
            Ok(rows
                .iter_mut()
                .find(|r| r.0 == id)
                .map(|r| r.1 = json.into())
                .is_some())
        }
        fn drop_row(&self, id: u64) -> Result<bool> {
            let mut rows = self.0.lock().unwrap();
            let before = rows.len();
            rows.retain(|r| r.0 != id);
            Ok(rows.len() < before)
        }
    }

    fn request(method: &str, path: &str, auth: Option<&str>, extra: &str, body: &str) -> Response {
        let auth = auth.map_or(String::new(), |k| {
            let mut encoded = String::new();
            wisp_shared::base64::encode(&mut encoded, format!("admin:{k}").as_bytes(), false);
            format!("authorization: Basic {encoded}\r\n")
        });
        let raw = format!(
            "{method} {path} HTTP/1.1\r\nhost: h.test\r\n{auth}{extra}content-type: application/x-www-form-urlencoded\r\n\r\n{body}"
        );
        let cx = Cx::for_test(&raw, &[]);
        answer(
            &cx,
            path.strip_prefix(PATH).unwrap().trim_matches('/'),
            "sesame",
        )
    }

    #[test]
    fn a_long_table_is_listed_a_page_at_a_time() {
        let many = Fake(std::sync::Mutex::new(
            (1..=150).map(|i| (i, format!("{{\"n\":{i}}}"))).collect(),
        ));
        let at = |target: &str| {
            let raw = format!("GET {target} HTTP/1.1\r\nhost: h.test\r\n\r\n");
            body(&rows(&Cx::for_test(&raw, &[]), &many))
        };
        let first = at("/_wisp/admin/fake");
        assert!(first.contains("Delete row 100?") && !first.contains("Delete row 101?"));
        assert!(first.contains("fake?after=100"));
        let rest = at("/_wisp/admin/fake?after=100");
        assert!(rest.contains("Delete row 150?") && !rest.contains("Delete row 100?"));
        assert!(!rest.contains(">More<"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_client_that_asks_too_much_is_refused() {
        let limit = RateLimit::per_minute(2);
        let cx = Cx::for_test("GET /_wisp/admin HTTP/1.1\r\nhost: h.test\r\n\r\n", &[]);
        assert!(tries(&limit, &cx).is_none() && tries(&limit, &cx).is_none());
        let r = tries(&limit, &cx).expect("refused");
        assert_eq!(r.status, 429);
    }

    fn body(r: &Response) -> String {
        String::from_utf8_lossy(&r.body).into_owned()
    }

    #[test]
    fn lists_edits_and_deletes_behind_the_key() {
        let fake: &'static Fake = Box::leak(Box::new(Fake(std::sync::Mutex::new(vec![
            (1, r#"{"t":"<b>one</b>"}"#.into()),
            (2, r#"{"t":"two"}"#.into()),
        ]))));
        register(fake);
        let get = |path: &str, auth| request("GET", path, auth, "", "");
        let r = get("/_wisp/admin", None);
        assert_eq!(r.status, 401);
        assert!(
            r.headers
                .iter()
                .any(|(n, v)| n == "www-authenticate" && v.starts_with("Basic"))
        );
        assert_eq!(get("/_wisp/admin", Some("wrong")).status, 401);
        let r = get("/_wisp/admin", Some("sesame"));
        assert!(r.status == 200 && body(&r).contains("/_wisp/admin/fake"));
        let r = get("/_wisp/admin/fake", Some("sesame"));
        assert!(
            body(&r).contains("&lt;b&gt;one") && !body(&r).contains("<b>one"),
            "escaped"
        );
        assert_eq!(get("/_wisp/admin/nope", Some("sesame")).status, 404);
        assert_eq!(get("/_wisp/admin/fake/9", Some("sesame")).status, 404);
        let r = get("/_wisp/admin/fake/2", Some("sesame"));
        assert!(
            body(&r).contains("<textarea name=json spellcheck=false>{&#34;t&#34;:&#34;two&#34;}")
                || body(&r).contains("two")
        );

        // Posts need the key and this site's own origin.
        let post = |path: &str, auth, extra: &str, b: &str| request("POST", path, auth, extra, b);
        let own = "origin: http://h.test\r\n";
        assert_eq!(
            post("/_wisp/admin/fake/2", None, own, "json=%7B%7D").status,
            401
        );
        assert_eq!(
            post("/_wisp/admin/fake/2", Some("sesame"), "", "json=%7B%7D").status,
            403
        );
        let other = "origin: http://evil.test\r\n";
        assert_eq!(
            post("/_wisp/admin/fake/2", Some("sesame"), other, "json=%7B%7D").status,
            403
        );
        let same = "sec-fetch-site: same-origin\r\n";
        let r = post(
            "/_wisp/admin/fake/2",
            Some("sesame"),
            own,
            "json=%7B%22t%22%3A%22new%22%7D",
        );
        assert_eq!(r.status, 303);
        assert_eq!(fake.rows(0, 10)[1].1, r#"{"t":"new"}"#);
        let r = post("/_wisp/admin/fake/2", Some("sesame"), same, "json=nope");
        assert!(
            r.status == 422 && body(&r).contains("is not an object"),
            "{}",
            body(&r)
        );
        assert_eq!(fake.rows(0, 10)[1].1, r#"{"t":"new"}"#, "left as it was");
        assert_eq!(
            post("/_wisp/admin/fake/1/delete", Some("sesame"), own, "").status,
            303
        );
        assert_eq!(fake.rows(0, 10).len(), 1);
        assert_eq!(
            request("PUT", "/_wisp/admin/fake/1", Some("sesame"), "", "").status,
            405
        );
    }
}
