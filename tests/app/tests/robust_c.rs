//! Signed cookies across a change of `WISP_SECRET`: with the old secret in
//! `WISP_SECRET_OLD`, cookies it signed still hold; without, they do not.
//! And an old secret too short to be safe stops the server, as a short
//! `WISP_SECRET` does.

mod common;

use common::{SECRET, start, status};

const NEW: &str = "fedcba9876543210fedcba9876543210";

/// The `user=…` cookie the login form sets.
fn sign_in(secret: &str) -> String {
    let s = start(&[("WISP_SECRET", secret)]);
    let r = s.request(
        "POST",
        "/login",
        "content-type: application/x-www-form-urlencoded\r\n",
        b"name=ada",
    );
    let line = r
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: user="))
        .unwrap_or_else(|| panic!("no cookie in {r}"));
    format!("user={}", line.split(';').next().unwrap())
}

/// The status of `/admin`, which only a signed-in visitor sees.
fn admin(env: &[(&str, &str)], cookie: &str) -> u16 {
    let s = start(env);
    status(&s.request("GET", "/admin", &format!("cookie: {cookie}\r\n"), b""))
}

#[test]
fn an_old_secret_keeps_cookies_it_signed() {
    let cookie = sign_in(SECRET);
    assert_eq!(admin(&[("WISP_SECRET", SECRET)], &cookie), 200);
    assert_eq!(
        admin(
            &[("WISP_SECRET", NEW), ("WISP_SECRET_OLD", SECRET)],
            &cookie
        ),
        200
    );
    assert_eq!(admin(&[("WISP_SECRET", NEW)], &cookie), 303, "signed out");
    // A cookie the new secret signs holds with it alone, as always.
    let fresh = sign_in(NEW);
    assert_eq!(
        admin(&[("WISP_SECRET", NEW), ("WISP_SECRET_OLD", SECRET)], &fresh),
        200
    );
}

#[test]
fn a_short_old_secret_stops_the_server() {
    let out = common::command(&[("WISP_SECRET_OLD", "short")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("WISP_SECRET_OLD is 5 characters"), "{said}");
}
