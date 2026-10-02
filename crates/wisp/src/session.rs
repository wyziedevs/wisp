//! Who is signed in: `cx.sign_in(id)` when a password checks,
//! `let id = cx.signed_in()?;` (or `let user = cx.user(&USERS)?;`) in a page
//! only members see, `cx.sign_out()` to leave.
//!
//! The id is kept in the signed cookie `session` with the time it was
//! given (see `sign.rs`: `WISP_SECRET` signs it), for 30 days. A visitor
//! who is not signed in is sent (303) to `/login`, or the page
//! `wisp::sign_in_page` names; a JSON client gets a 401 instead.
//!
//! `wisp::sign_out_everywhere(id)` ends every session of `id` made before
//! it, a stolen cookie's too: it counts up the id's sign-outs in a saved
//! table, and a session made after carries the count (`id.time.count`).
//! Until an app calls it, a session is read with nothing looked up.

use crate::cx::CookieOptions;
use crate::{Cx, Error, Result, Row, Table, unix_now};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const COOKIE: &str = "session";

/// How many times each id has signed out everywhere, by id, saved.
static SIGN_OUTS: Table<u64> = Table::saved("wisp_sign_outs");

/// Whether [`SIGN_OUTS`] has any row: until one does, reading a session
/// looks nothing up. `UNKNOWN` before it is first read.
static ANY: AtomicU8 = AtomicU8::new(UNKNOWN);
const UNKNOWN: u8 = 0;
const NONE: u8 = 1;
const SOME: u8 = 2;

/// How long a sign-in lasts.
const DAYS: u64 = 30;

static SIGN_IN_PAGE: RwLock<&'static str> = RwLock::new("/login");

/// Where a visitor who is not signed in is sent, `/login` unless this says
/// otherwise: `wisp::sign_in_page("/account/enter");` in `init`.
pub fn sign_in_page(path: &'static str) {
    assert!(
        path.starts_with('/'),
        "a sign-in page is a path, such as \"/login\""
    );
    *SIGN_IN_PAGE.write().unwrap_or_else(|e| e.into_inner()) = path;
}

/// Ends every session of `id` made before now, on every device, a stolen
/// cookie's included: after a password change, or from an admin page.
/// `cx.sign_in(id)` after it starts a new one. Kept in the app's store, so
/// it holds after a restart; a server reads it at its first session, so
/// other instances of the app see it when they next start. `id` is a row
/// id (from 1). A store that fails is the request's 500, as for any table.
pub fn sign_out_everywhere(id: u64) {
    assert!(id != 0, "sign_out_everywhere takes a row id, from 1");
    let mut rows = SIGN_OUTS.write();
    let count = rows.map.get(&id).map_or(1, |n| n + 1);
    SIGN_OUTS.save(&mut rows, id, Some(&count.to_string()));
    rows.map.insert(id, count);
    drop(rows);
    ANY.store(SOME, Ordering::Release);
}

/// How many times `id` has signed out everywhere: what its sessions must
/// carry. No lookup while no one ever has.
fn sign_outs(id: u64) -> u64 {
    match ANY.load(Ordering::Acquire) {
        NONE => return 0,
        UNKNOWN => {
            // A store that cannot be read (a read-only folder, say) leaves
            // sessions as they were before sign-outs, rather than failing
            // each one; `sign_out_everywhere` then fails, saying why.
            let read = std::panic::catch_unwind(|| SIGN_OUTS.is_empty());
            let any = if read.unwrap_or(true) { NONE } else { SOME };
            // A sign-out meanwhile has said SOME, which stays.
            let _ = ANY.compare_exchange(UNKNOWN, any, Ordering::AcqRel, Ordering::Acquire);
            if any == NONE {
                return 0;
            }
        }
        _ => {}
    }
    SIGN_OUTS.with(id, |n| *n).unwrap_or(0)
}

impl Cx {
    /// Signs the visitor in as `id` (a row id of the app's users, say) for
    /// 30 days, from the response to this request on. The session is a new
    /// one whatever the visitor sent, so one planted before sign-in
    /// (fixation) is gone.
    pub fn sign_in(&mut self, id: u64) {
        let options = CookieOptions {
            max_age: Some(Duration::from_secs(DAYS * 86_400)),
            signed: true,
            ..CookieOptions::default()
        };
        let (now, count) = (unix_now(), sign_outs(id));
        if count == 0 {
            self.set_cookie_with(COOKIE, format_args!("{id}.{now}"), options);
        } else {
            self.set_cookie_with(COOKIE, format_args!("{id}.{now}.{count}"), options);
        }
    }

    /// Signs the visitor out, on this browser; [`sign_out_everywhere`] for
    /// every one.
    pub fn sign_out(&mut self) {
        self.delete_cookie(COOKIE);
    }

    /// The id the visitor signed in as; signed out, the error that sends
    /// them to sign in: `let id = cx.signed_in()?;`. `cx.signed_in().ok()`
    /// asks without sending them anywhere.
    pub fn signed_in(&self) -> Result<u64> {
        let id = self.signed_cookie(COOKIE).and_then(|v| {
            let mut parts = v.split('.');
            let (Some(id), Some(at), count, None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return None;
            };
            let (id, at) = (id.parse().ok()?, at.parse().ok()?);
            let count: u64 = count.map_or(Some(0), |c| c.parse().ok())?;
            let fresh = unix_now().saturating_sub(at) <= DAYS * 86_400;
            (fresh && count == sign_outs(id)).then_some(id)
        });
        id.ok_or_else(|| self.signed_out())
    }

    /// The row of `table` the visitor signed in as: `let user =
    /// cx.user(&USERS)?;`. Signed out, or a row since removed, sends them
    /// to sign in, as [`Cx::signed_in`] does.
    pub fn user<T: Clone>(&self, table: &Table<T>) -> Result<Row<T>> {
        table
            .get(self.signed_in()?)
            .ok_or_else(|| self.signed_out())
    }

    fn signed_out(&self) -> Error {
        if self.api() || crate::input::asks_json(self) {
            return Error::new(401, "Sign in first").with_code("signed_out");
        }
        let page = *SIGN_IN_PAGE.read().unwrap_or_else(|e| e.into_inner());
        Error::redirect(303, page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request that sends back the cookie `cx` set.
    fn again(cx: &Cx, head: &str) -> Cx {
        let (_, set) = cx
            .out_headers()
            .iter()
            .rev()
            .find(|(n, _)| n == "set-cookie")
            .unwrap();
        let cookie = set.split(';').next().unwrap();
        Cx::for_test(
            &format!("GET /me HTTP/1.1\r\n{head}cookie: {cookie}\r\n\r\n"),
            &[],
        )
    }

    fn location(e: Error) -> (u16, String) {
        let status = e.status();
        (status, e.header.map(|h| h.1).unwrap_or_default())
    }

    #[test]
    fn sign_in_and_out() {
        let mut cx = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        assert_eq!(
            location(cx.signed_in().unwrap_err()),
            (303, "/login".into())
        );
        sign_in_page("/enter");
        assert_eq!(
            location(cx.signed_in().unwrap_err()),
            (303, "/enter".into())
        );
        sign_in_page("/login");
        cx.sign_in(42);
        assert_eq!(cx.signed_in().unwrap(), 42, "at once, in the same request");
        let mut next = again(&cx, "");
        assert_eq!(next.signed_in().unwrap(), 42);
        next.sign_out();
        assert!(next.signed_in().is_err());
        assert!(again(&next, "").signed_in().is_err());

        let json = Cx::for_test("GET /me HTTP/1.1\r\naccept: application/json\r\n\r\n", &[]);
        assert_eq!(json.signed_in().unwrap_err().status(), 401);

        // A forged id, or a sign-in older than 30 days, is nobody.
        let forged = Cx::for_test("GET /me HTTP/1.1\r\ncookie: session=1.1\r\n\r\n", &[]);
        assert!(forged.signed_in().is_err());
        let mut old = Cx::for_test("GET /me HTTP/1.1\r\n\r\n", &[]);
        let then = unix_now() - DAYS * 86_400 - 1;
        old.set_signed_cookie(COOKIE, format_args!("7.{then}"));
        assert!(again(&old, "").signed_in().is_err());

        let users: Table<&str> = Table::new();
        let ada = users.add("ada");
        let mut cx = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        cx.sign_in(ada);
        assert_eq!(cx.user(&users).unwrap().value, "ada");
        users.remove(ada);
        assert_eq!(cx.user(&users).unwrap_err().status(), 303);
    }

    #[test]
    fn sign_out_everywhere_ends_older_sessions() {
        let mut first = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        first.sign_in(77);
        let mut other = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        other.sign_in(78);
        assert_eq!(again(&first, "").signed_in().unwrap(), 77);

        sign_out_everywhere(77);
        assert!(again(&first, "").signed_in().is_err(), "a stolen copy too");
        assert_eq!(again(&other, "").signed_in().unwrap(), 78, "others stay");

        // Signing in again, in the same second even, starts one that holds.
        let mut next = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        next.sign_in(77);
        let kept = again(&next, "");
        assert_eq!(kept.signed_in().unwrap(), 77);
        sign_out_everywhere(77);
        assert!(again(&next, "").signed_in().is_err());
        assert!(std::panic::catch_unwind(|| sign_out_everywhere(0)).is_err());
    }

    /// Signing in replaces whatever session the visitor came with, so one
    /// planted on them before (fixation) never becomes theirs.
    #[test]
    fn sign_in_starts_a_new_session() {
        let mut planted = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        planted.sign_in(5);
        let mut cx = again(&planted, "");
        assert_eq!(cx.signed_in().unwrap(), 5);
        cx.sign_in(6);
        assert_eq!(cx.signed_in().unwrap(), 6);
        assert_eq!(again(&cx, "").signed_in().unwrap(), 6);
    }

    /// A session cookie changed in any way is nobody, never someone else,
    /// and never a panic.
    #[test]
    fn mangled_sessions_are_nobody() {
        use crate::fuzz::{Rng, mutate};
        let mut cx = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        cx.sign_in(42);
        let (_, set) = cx
            .out_headers()
            .iter()
            .find(|(n, _)| n == "set-cookie")
            .unwrap();
        let good = set.split(';').next().unwrap().to_string();
        let mut rng = Rng::new(5);
        for _ in 0..20_000 {
            let mut b = good.clone().into_bytes();
            mutate(&mut rng, &mut b);
            let Ok(cookie) = String::from_utf8(b) else {
                continue;
            };
            if cookie.contains(['\r', '\n']) {
                continue;
            }
            let head = format!("GET /me HTTP/1.1\r\ncookie: {cookie}\r\n\r\n");
            let id = Cx::for_test(&head, &[]).signed_in().ok();
            assert!(id.is_none() || id == Some(42), "{cookie:?} is {id:?}");
        }
    }
}
