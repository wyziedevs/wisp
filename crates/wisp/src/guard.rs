//! Who may do what: `cx.need(&USERS, |u| u.admin)?` for a role, and
//! `cx.cors("*")?` for pages on other sites that call the app.

use crate::{Cx, Error, Result, Row, Table};

impl Cx {
    /// The member of `table` signed in, if `allowed` of it: `let admin =
    /// cx.need(&USERS, |u| u.admin)?;`. Signed out is [`Cx::user`]'s
    /// error (a 303 to sign in, a 401 for a JSON client); signed in but
    /// not allowed is a 403, the same whatever the reason.
    pub fn need<T: Clone>(
        &self,
        table: &Table<T>,
        allowed: impl FnOnce(&T) -> bool,
    ) -> Result<Row<T>> {
        let user = self.user(table)?;
        if allowed(&user.value) {
            return Ok(user);
        }
        Err(Error::new(403, "You may not do that"))
    }

    /// Lets pages on other sites call the app from the browser (CORS):
    /// `origins` is `*` for any site, or the sites allowed, separated by
    /// spaces or commas (`"https://app.example.com https://example.com"`),
    /// which may also send their cookies. Call it from `before` in
    /// `src/hooks.rs`:
    ///
    /// ```ignore
    /// fn before(cx: &mut Cx) -> Result {
    ///     cx.cors("*")?;
    ///     ...
    /// }
    /// ```
    ///
    /// The browser's preflight (the OPTIONS it sends before a request that
    /// is not a plain form post) is answered with a 204 and the headers,
    /// carried as the `Err` that `?` returns; any other request goes on. A
    /// request from a site not allowed gets no CORS headers, so its
    /// browser does not hand it the answer.
    pub fn cors(&mut self, origins: &str) -> Result {
        match self.cors_preflight(origins) {
            Some(status) => Err(Error::raw(status, "".into())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cx: &Cx) -> Vec<String> {
        let all = cx.out_headers();
        all.iter().map(|(n, v)| format!("{n}: {v}")).collect()
    }

    #[test]
    fn need_a_role() {
        #[derive(Clone, Debug)]
        struct User {
            admin: bool,
        }
        let users: Table<User> = Table::new();
        let (ada, bob) = (
            users.add(User { admin: true }),
            users.add(User { admin: false }),
        );
        let mut cx = Cx::for_test("POST / HTTP/1.1\r\n\r\n", &[]);
        assert_eq!(cx.need(&users, |u| u.admin).unwrap_err().status(), 303);
        let json = Cx::for_test("GET / HTTP/1.1\r\naccept: application/json\r\n\r\n", &[]);
        assert_eq!(json.need(&users, |u| u.admin).unwrap_err().status(), 401);
        cx.sign_in(bob);
        let e = cx.need(&users, |u| u.admin).unwrap_err();
        assert_eq!((e.status(), e.code()), (403, "forbidden"));
        cx.sign_in(ada);
        assert_eq!(cx.need(&users, |u| u.admin).unwrap().id, ada);
        users.remove(ada);
        assert_eq!(cx.need(&users, |_| true).unwrap_err().status(), 303, "gone");
    }

    #[test]
    fn cors_goes_on_or_carries_the_preflight() {
        let mut plain = Cx::for_test("GET / HTTP/1.1\r\nOrigin: https://a.example\r\n\r\n", &[]);
        plain.cors("https://a.example").unwrap();
        assert_eq!(
            headers(&plain),
            [
                "vary: origin",
                "access-control-allow-origin: https://a.example",
                "access-control-allow-credentials: true"
            ]
        );
        let mut other = Cx::for_test(
            "GET / HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n",
            &[],
        );
        other.cors("https://a.example").unwrap();
        assert_eq!(headers(&other), ["vary: origin"], "no allow header");
        let mut pre = Cx::for_test(
            "OPTIONS /x HTTP/1.1\r\nOrigin: https://a.example\r\nAccess-Control-Request-Method: PUT\r\n\r\n",
            &[],
        );
        let e = pre.cors("https://a.example").unwrap_err();
        assert_eq!(e.status(), 204);
        let sent = headers(&pre);
        assert!(sent.contains(&"access-control-allow-methods: PUT".to_string()));
        assert!(sent.contains(&"access-control-allow-origin: https://a.example".to_string()));
        // A preflight from a site not allowed is answered, with no allowance.
        let mut evil = Cx::for_test(
            "OPTIONS /x HTTP/1.1\r\nOrigin: https://evil.example\r\nAccess-Control-Request-Method: PUT\r\n\r\n",
            &[],
        );
        evil.cors("https://a.example").unwrap();
        assert!(!headers(&evil).iter().any(|h| h.contains("allow-origin")));
    }
}
