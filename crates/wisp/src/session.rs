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
//! The table is read when the server starts; until it has a row, a session
//! is read with nothing looked up. With the app's own store
//! (`wisp::store`), which several instances can share, each reads it again
//! every 30 s, so a sign-out on one holds on all within that. The log
//! files are each instance's own.

use crate::cx::CookieOptions;
use crate::{Cx, Error, Result, Row, Table, store, unix_now};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const COOKIE: &str = "session";
const TABLE: &str = "wisp_sign_outs";

/// How many times each id has signed out everywhere, by id, saved.
static SIGN_OUTS: Table<u64> = Table::saved(TABLE);

/// Whether [`SIGN_OUTS`] has any row: until it does, reading a session
/// looks nothing up.
static ANY: AtomicBool = AtomicBool::new(false);

/// How often a shared store's sign-outs are read again.
#[cfg(not(target_arch = "wasm32"))]
const REREAD: Duration = Duration::from_secs(30);

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
/// id (from 1). A store that fails is an `Err` (a 500 through `?`), and
/// nothing is ended: say so rather than that it was.
pub fn sign_out_everywhere(id: u64) -> Result {
    if id == 0 {
        return Err(Error::new(
            500,
            "sign_out_everywhere takes a row id, from 1",
        ));
    }
    let mut rows = SIGN_OUTS.write();
    let count = rows.map.get(&id).map_or(1, |n| n + 1);
    SIGN_OUTS.save(&mut rows, id, Some(&count.to_string()))?;
    rows.map.insert(id, count);
    drop(rows);
    ANY.store(true, Ordering::Release);
    Ok(())
}

/// Reads the sign-outs when the server starts, once `init` has set the
/// store, and again every 30 s from a store of the app's own, off the
/// requests' threads.
pub(crate) fn ready() {
    reread();
    #[cfg(not(target_arch = "wasm32"))]
    if store::custom().is_some() {
        let _ = std::thread::Builder::new()
            .name("wisp-sign-outs".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(REREAD);
                    reread();
                }
            });
    }
}

/// Reads the sign-outs from the store; a count there above the one here
/// (another instance's sign-out) wins. No log file means none: no folder
/// is made for an app that never signs anyone out. A store that cannot be
/// read leaves sessions as they were, says why, and is tried again.
fn reread() {
    if !store::holds(TABLE) {
        return;
    }
    let read = std::panic::catch_unwind(|| {
        SIGN_OUTS.refresh(|now, stored| stored > now)?;
        Ok::<_, Error>(!SIGN_OUTS.is_empty())
    });
    match read {
        Ok(Ok(any)) => {
            ANY.fetch_or(any, Ordering::Release);
        }
        Ok(Err(e)) => log(&e.detail()),
        Err(_) => log("see the panic above"),
    }
}

fn log(why: &str) {
    crate::http::log(format_args!(
        "wisp: could not read who signed out everywhere ({why}); sessions hold as before until it can"
    ));
}

/// How many times `id` has signed out everywhere: what its sessions must
/// carry. No lookup while no one ever has.
fn sign_outs(id: u64) -> u64 {
    if !ANY.load(Ordering::Acquire) {
        return 0;
    }
    SIGN_OUTS.with(id, |n| *n).unwrap_or(0)
}

/// A member row: who it is (`email` or `name`) and its password hash.
/// `#[model]` implements it for a struct with a `Password` field (or a
/// `hash: String`) and an `email` or `name` field; any other type can by
/// hand.
pub trait Account {
    /// The field members sign in by: errors name it.
    const WHO: &'static str;
    fn who(&self) -> &str;
    fn hash(&self) -> &str;
    fn set_hash(&mut self, hash: String);
    /// Whether `hash` already is one: a [`Password`](crate::Password) made
    /// by `Password::new`. [`signup`] hashes what is not.
    fn hashed(&self) -> bool {
        false
    }
}

/// The member of `users` whose `who` and `password` these are, or a 422 on
/// `who`'s field, the same whichever is wrong. Takes as long for no such
/// member (`password::check`), so names stay secret.
pub async fn login<T: Account + Clone>(
    users: &Table<T>,
    who: &str,
    password: &str,
) -> Result<Row<T>> {
    let found = users.find(|u| u.who() == who);
    let right = crate::password::check(password, found.as_ref().map(|u| u.hash())).await?;
    found
        .filter(|_| right)
        .ok_or_else(|| Error::invalid(T::WHO, format!("Wrong {} or password", T::WHO)))
}

/// Keeps `row` in `users`, its `hash` field holding the password, which
/// becomes its hash first; a 422 on `who`'s field when a member has that
/// already.
pub async fn signup<T: Account + Clone>(users: &Table<T>, mut row: T) -> Result<Row<T>> {
    if !row.hashed() {
        let hash = crate::password::hash(row.hash()).await?;
        row.set_hash(hash);
    }
    let who = row.who().to_string();
    users
        .add_unless(|u| u.who() == who, row)
        .ok_or_else(|| Error::invalid(T::WHO, "Already signed up"))
}

/// Names the app's users table, in `init`: `wisp::users(&db::USERS)`. The
/// build reads it, so `cx.user()` takes no argument.
pub fn users<T>(_: &Table<T>) {}

impl Cx {
    /// [`login`], then signs the visitor in as the member.
    pub async fn login<T: Account + Clone>(
        &mut self,
        users: &Table<T>,
        who: &str,
        password: &str,
    ) -> Result<Row<T>> {
        let user = login(users, who, password).await?;
        self.sign_in(user.id);
        Ok(user)
    }

    /// [`signup`], then signs the visitor in as the new member.
    pub async fn signup<T: Account + Clone>(&mut self, users: &Table<T>, row: T) -> Result<Row<T>> {
        let user = signup(users, row).await?;
        self.sign_in(user.id);
        Ok(user)
    }

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
        if crate::http::wants_json(self) {
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

        sign_out_everywhere(77).unwrap();
        assert!(again(&first, "").signed_in().is_err(), "a stolen copy too");
        assert_eq!(again(&other, "").signed_in().unwrap(), 78, "others stay");

        // Signing in again, in the same second even, starts one that holds.
        let mut next = Cx::for_test("POST /login HTTP/1.1\r\n\r\n", &[]);
        next.sign_in(77);
        let kept = again(&next, "");
        assert_eq!(kept.signed_in().unwrap(), 77);
        sign_out_everywhere(77).unwrap();
        assert!(again(&next, "").signed_in().is_err());
        assert!(sign_out_everywhere(0).is_err());
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
