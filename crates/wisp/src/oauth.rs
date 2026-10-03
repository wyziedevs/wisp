//! Sign in with GitHub, Google or any OpenID Connect provider: the
//! authorization-code flow with PKCE, in two routes of the app's own.
//!
//! ```ignore
//! // src/routes/auth/github/+server.rs        sends the visitor to GitHub
//! fn get(cx) -> Result { wisp::oauth::github().start(cx) }
//!
//! // src/routes/auth/github/callback/+server.rs   GitHub sends them back
//! async fn get(cx) -> Result {
//!     let who = wisp::oauth::github().finish(cx).await?;      // who.id, who.email
//!     let user = USERS.find(|u| u.github == who.id).or_else(|| ...);
//!     cx.sign_in(user.id);
//!     redirect("/")
//! }
//! ```
//!
//! The keys are `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` (`GOOGLE_...`;
//! `{NAME}_...` for [`oidc`]), or [`Provider::keys`]. The provider is told
//! to come back to `ORIGIN` (or, in dev, the host asked) plus
//! `/auth/{name}/callback`: register that, or set it with
//! [`Provider::redirect`]. Set `ORIGIN` in production: without it the
//! `Host` header decides, and a provider refuses an address not registered.
//!
//! What keeps it safe, each with a test:
//! - `state`, a random value in a signed cookie that lives 10 minutes and
//!   is used up by the first callback, must come back in the query, which
//!   is compared in constant time: a callback the visitor did not start
//!   (login CSRF) is refused.
//! - PKCE (S256): the code alone is worthless to whoever sees it.
//! - The secret goes only in the POST to the provider's token endpoint,
//!   never in a URL; tokens and keys are in no error and no log. Endpoints
//!   must be `https` (but for loopback, for tests).
//! - Only an email the provider says is verified is returned: an unverified
//!   one lets anyone claim an address and take over its account.
//! - Every failure is the same 400 (a 403 if the visitor said no), a 502
//!   when the provider failed.

use crate::json::{Value, from_json};
use crate::sign::{base64, random};
use crate::{CookieOptions, Cx, Error, Request, Result, secure_eq};
use std::time::Duration;
use wisp_shared::sha256::sha256;

/// How long a sign-in may take, from the redirect to the callback.
const TRIP: Duration = Duration::from_secs(600);

/// Who the provider says the visitor is.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// The provider's id for them, which never changes (not their name).
    pub id: String,
    /// Their email, only if the provider vouches for it.
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct Provider {
    name: &'static str,
    auth: String,
    token: String,
    userinfo: String,
    scope: &'static str,
    keys: Option<(String, String)>,
    redirect: Option<String>,
    github: bool,
}

pub fn github() -> Provider {
    Provider {
        github: true,
        ..Provider::new(
            "github",
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            "https://api.github.com/user",
            "read:user user:email",
        )
    }
}

pub fn google() -> Provider {
    Provider::new(
        "google",
        "https://accounts.google.com/o/oauth2/v2/auth",
        "https://oauth2.googleapis.com/token",
        "https://openidconnect.googleapis.com/v1/userinfo",
        "openid email profile",
    )
}

/// A provider that speaks OpenID Connect, from its `issuer` address
/// (`https://login.example.com`): its endpoints are read from
/// `/.well-known/openid-configuration`, once. `name` is in the callback
/// path and the keys' names (`name = "okta"`: `OKTA_CLIENT_ID`).
pub async fn oidc(name: &'static str, issuer: &str) -> Result<Provider> {
    static FOUND: std::sync::Mutex<Vec<(String, Provider)>> = std::sync::Mutex::new(Vec::new());
    let issuer = issuer.trim_end_matches('/');
    let find = || {
        let found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        (found.iter().find(|(i, _)| i == issuer)).map(|(_, p)| Provider { name, ..p.clone() })
    };
    if let Some(p) = find() {
        return Ok(p);
    }
    let reply = crate::fetch(Request::new(
        "GET",
        &format!("{issuer}/.well-known/openid-configuration"),
    ))
    .await?;
    let found: Value = from_json(reply.bytes()).map_err(|_| unreachable_provider())?;
    let get = |k: &str| found.get(k).and_then(Value::as_str).map(str::to_string);
    // The document must be the issuer's own, or any site could claim to be.
    let (auth, token, userinfo) = match (
        get("issuer"),
        get("authorization_endpoint"),
        get("token_endpoint"),
        get("userinfo_endpoint"),
    ) {
        (Some(i), Some(a), Some(t), Some(u)) if i.trim_end_matches('/') == issuer => (a, t, u),
        _ => return Err(unreachable_provider()),
    };
    let p = Provider {
        keys: None,
        ..Provider::new(name, &auth, &token, &userinfo, "openid email profile")
    };
    // A second sign-in that raced this one found nothing either: one entry.
    let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    match found.iter_mut().find(|(i, _)| i == issuer) {
        Some(entry) => entry.1 = p.clone(),
        None => found.push((issuer.to_string(), p.clone())),
    }
    Ok(p)
}

fn unreachable_provider() -> Error {
    Error::new(502, "the sign-in provider did not answer as expected")
}

fn failed() -> Error {
    Error::new(400, "Sign-in failed. Please try again.").with_code("oauth_failed")
}

impl Provider {
    /// Any provider of the authorization-code flow.
    pub fn new(
        name: &'static str,
        auth: &str,
        token: &str,
        userinfo: &str,
        scope: &'static str,
    ) -> Provider {
        assert!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "a provider's name is a word, such as \"github\""
        );
        Provider {
            name,
            auth: auth.into(),
            token: token.into(),
            userinfo: userinfo.into(),
            scope,
            keys: None,
            redirect: None,
            github: false,
        }
    }

    /// The client id and secret, when not in `{NAME}_CLIENT_ID` and
    /// `{NAME}_CLIENT_SECRET`.
    pub fn keys(mut self, id: &str, secret: &str) -> Provider {
        self.keys = Some((id.into(), secret.into()));
        self
    }

    /// The address the provider sends the visitor back to, when it is not
    /// `ORIGIN` + `/auth/{name}/callback`.
    pub fn redirect(mut self, url: &str) -> Provider {
        self.redirect = Some(url.into());
        self
    }

    fn keys_or_env(&self) -> Result<(String, String)> {
        if let Some(k) = &self.keys {
            return Ok(k.clone());
        }
        let var = |what| {
            crate::env(&format!("{}_CLIENT_{what}", self.name.to_ascii_uppercase()))
                .filter(|v| !v.is_empty())
        };
        match (var("ID"), var("SECRET")) {
            (Some(id), Some(secret)) => Ok((id, secret)),
            _ => Err(Error::new(
                500,
                format!(
                    "sign-in with {0} is not set up: set {1}_CLIENT_ID and {1}_CLIENT_SECRET",
                    self.name,
                    self.name.to_ascii_uppercase()
                ),
            )),
        }
    }

    fn callback(&self, cx: &Cx) -> String {
        if let Some(r) = &self.redirect {
            return r.clone();
        }
        let origin = crate::settings().origin.clone().unwrap_or_else(|| {
            let scheme = if cx.is_https() { "https" } else { "http" };
            format!("{scheme}://{}", cx.host().unwrap_or("localhost"))
        });
        format!("{origin}/auth/{}/callback", self.name)
    }

    fn cookie(&self) -> String {
        format!("wisp_oauth_{}", self.name)
    }

    /// Sends the visitor to the provider: the `Err` that `?` returns is a
    /// 303 there, so `fn get(cx) -> Result { provider.start(cx) }` is the
    /// whole route.
    pub fn start(&self, cx: &mut Cx) -> Result {
        let (id, _) = self.keys_or_env()?;
        secure_endpoint(&self.auth)?;
        let (mut state, mut verifier) = (String::new(), String::new());
        base64(&mut state, &random::<16>(), true);
        base64(&mut verifier, &random::<32>(), true);
        let mut challenge = String::new();
        base64(&mut challenge, &sha256(&[verifier.as_bytes()]), true);
        let options = CookieOptions {
            max_age: Some(TRIP),
            signed: true,
            ..CookieOptions::default()
        };
        cx.set_cookie_with(&self.cookie(), format_args!("{state}.{verifier}"), options);
        let mut url = format!(
            "{}{}response_type=code",
            self.auth,
            if self.auth.contains('?') { '&' } else { '?' }
        );
        for (k, v) in [
            ("client_id", id.as_str()),
            ("redirect_uri", &self.callback(cx)),
            ("scope", self.scope),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ] {
            url.push('&');
            url.push_str(k);
            url.push('=');
            let _ = crate::cx::encode(&mut url, v, crate::cx::unreserved);
        }
        Err(Error::redirect(303, url))
    }

    /// Finishes the sign-in on the callback: checks it is the one this
    /// visitor started, trades the code for the visitor's profile, and
    /// returns it. Sign them in with [`Cx::sign_in`] after finding or
    /// making their row.
    pub async fn finish(&self, cx: &mut Cx) -> Result<Profile> {
        let name = self.cookie();
        let started = cx.signed_cookie(&name).map(str::to_string);
        // Used up whatever comes next: a callback cannot be played twice.
        // Kept on the error pages too, which drop a handler's headers.
        cx.delete_cookie(&name);
        cx.keep_headers();
        let (state, verifier) = started
            .as_deref()
            .and_then(|s| s.split_once('.'))
            .ok_or_else(failed)?;
        let sent = cx.query("state");
        if !secure_eq(sent.as_deref().unwrap_or(""), state) {
            return Err(failed());
        }
        if cx.query("error").is_some() {
            return Err(Error::new(403, "Sign-in was cancelled").with_code("oauth_denied"));
        }
        let code = cx
            .query("code")
            .filter(|c| !c.is_empty() && c.len() <= 2048)
            .ok_or_else(failed)?;
        let (id, secret) = self.keys_or_env()?;
        secure_endpoint(&self.token)?;
        secure_endpoint(&self.userinfo)?;

        let mut form = String::new();
        for (k, v) in [
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &self.callback(cx)),
            ("client_id", &id),
            ("client_secret", &secret),
            ("code_verifier", verifier),
        ] {
            if !form.is_empty() {
                form.push('&');
            }
            form.push_str(k);
            form.push('=');
            let _ = crate::cx::encode(&mut form, v, crate::cx::unreserved);
        }
        let mut req = Request::new("POST", &self.token);
        req.header("content-type", "application/x-www-form-urlencoded");
        req.header("accept", "application/json");
        req.body = form.into_bytes();
        let reply = crate::fetch(req).await?;
        let token: Value = from_json(reply.bytes()).map_err(|_| failed())?;
        if reply.status != 200 {
            return Err(failed());
        }
        let access = token
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(failed)?;

        let user = self.get(&self.userinfo, access).await?;
        let text = |k: &str| user.get(k).and_then(Value::as_str).map(str::to_string);
        if self.github {
            let id = match user.get("id") {
                Some(Value::Number(n)) => n.clone(),
                _ => return Err(failed()),
            };
            let emails = self
                .get(&format!("{}/emails", self.userinfo), access)
                .await?;
            let email = match &emails {
                Value::Array(all) => all.iter().find_map(|e| {
                    let (primary, verified) =
                        (e.get("primary")?.as_bool()?, e.get("verified")?.as_bool()?);
                    (primary && verified).then(|| e.get("email")?.as_str().map(str::to_string))?
                }),
                _ => None,
            };
            return Ok(Profile {
                id,
                email,
                name: text("name").or_else(|| text("login")),
            });
        }
        let id = text("sub").filter(|s| !s.is_empty()).ok_or_else(failed)?;
        // Some providers send the flag as a string.
        let verified = match user.get("email_verified") {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s == "true",
            _ => false,
        };
        Ok(Profile {
            id,
            email: text("email").filter(|_| verified),
            name: text("name"),
        })
    }

    async fn get(&self, url: &str, access: &str) -> Result<Value> {
        let mut req = Request::new("GET", url);
        req.header("authorization", &format!("Bearer {access}"));
        req.header("accept", "application/json");
        let reply = crate::fetch(req).await?;
        if reply.status != 200 {
            return Err(unreachable_provider());
        }
        from_json(reply.bytes()).map_err(|_| unreachable_provider())
    }
}

/// The client secret and the visitor's token travel to these: `https`, or
/// a loopback address for a provider on this machine (a test's).
fn secure_endpoint(url: &str) -> Result {
    let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|l| {
            url.strip_prefix(l)
                .is_some_and(|r| r.is_empty() || r.starts_with([':', '/']))
        });
    if url.starts_with("https://") || local {
        return Ok(());
    }
    Err(Error::new(
        500,
        "a sign-in provider's address must be https://",
    ))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A provider on this machine that answers each request in turn with
    /// the next of `answers`, and keeps what it was sent.
    async fn provider(
        answers: Vec<(u16, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in answers {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got);
                    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let want = head
                        .to_ascii_lowercase()
                        .split("content-length: ")
                        .nth(1)
                        .and_then(|r| r.split("\r\n").next()?.parse::<usize>().ok())
                        .unwrap_or(0);
                    if n == 0 || rest.len() >= want {
                        break;
                    }
                }
                let answer = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(answer.as_bytes()).await.unwrap();
                seen.push(String::from_utf8_lossy(&got).to_string());
            }
            seen
        });
        (base, task)
    }

    fn run<T>(f: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn test_provider(base: &str) -> Provider {
        Provider::new(
            "fake",
            "https://idp.example/auth",
            &format!("{base}/token"),
            &format!("{base}/me"),
            "openid email",
        )
        .keys("the-id", "the-secret")
        .redirect("https://app.example/auth/fake/callback")
    }

    /// The query of the redirect `start` made, and the cookie it set.
    fn started(p: &Provider) -> (String, String, String) {
        let mut cx = Cx::for_test("GET /auth/fake HTTP/1.1\r\n\r\n", &[]);
        let e = p.start(&mut cx).unwrap_err();
        assert_eq!(e.status(), 303);
        let url = e.header.as_ref().unwrap().1.clone();
        let (_, set) = cx
            .out_headers()
            .iter()
            .find(|(n, _)| n == "set-cookie")
            .unwrap();
        (
            url,
            set.split(';').next().unwrap().to_string(),
            set.to_string(),
        )
    }

    fn param(url: &str, k: &str) -> String {
        url.split_once('?')
            .unwrap()
            .1
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(str::to_string))
            .unwrap()
    }

    fn callback(cookie: &str, query: &str) -> Cx {
        Cx::for_test(
            &format!("GET /auth/fake/callback?{query} HTTP/1.1\r\ncookie: {cookie}\r\n\r\n"),
            &[],
        )
    }

    #[test]
    fn start_redirects_with_state_and_pkce() {
        let (url, cookie, set) = started(&test_provider("http://127.0.0.1:1"));
        assert!(url.starts_with("https://idp.example/auth?response_type=code&client_id=the-id&redirect_uri=https%3A%2F%2Fapp.example%2Fauth%2Ffake%2Fcallback&scope=openid%20email&state="));
        assert!(url.ends_with("&code_challenge_method=S256") && !url.contains("the-secret"));
        let state = param(&url, "state");
        assert!(cookie.starts_with("wisp_oauth_fake=") && cookie.contains(&format!("={state}.")));
        assert!(
            set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("Max-Age=600"),
            "{set}"
        );
        let verifier = cookie.split_once('=').unwrap().1.split('.').nth(1).unwrap();
        let mut challenge = String::new();
        base64(&mut challenge, &sha256(&[verifier.as_bytes()]), true);
        assert_eq!(param(&url, "code_challenge"), challenge);
        assert_ne!(
            started(&test_provider("http://127.0.0.1:1")).1,
            cookie,
            "fresh each time"
        );
    }

    #[test]
    fn a_whole_sign_in() {
        run(async {
            let (base, seen) = provider(vec![
                (200, r#"{"access_token":"tok-123","token_type":"bearer"}"#),
                (
                    200,
                    r#"{"sub":"u-9","email":"ada@example.com","email_verified":true,"name":"Ada"}"#,
                ),
            ])
            .await;
            let p = test_provider(&base);
            let (url, cookie, _) = started(&p);
            let mut cx = callback(
                &cookie,
                &format!("code=the-code&state={}", param(&url, "state")),
            );
            let who = p.finish(&mut cx).await.unwrap();
            assert_eq!(
                who,
                Profile {
                    id: "u-9".into(),
                    email: Some("ada@example.com".into()),
                    name: Some("Ada".into())
                }
            );
            assert!(cx.cookie("wisp_oauth_fake").is_none(), "used up");
            let seen = seen.await.unwrap();
            let verifier = cookie.split('.').nth(1).unwrap();
            assert!(
                seen[0].starts_with("POST /token HTTP/1.1"),
                "the secret is in the body, not the URL"
            );
            for want in [
                "grant_type=authorization_code",
                "code=the-code",
                "client_secret=the-secret",
                &format!("code_verifier={verifier}"),
            ] {
                assert!(seen[0].contains(want), "{want}");
            }
            assert!(seen[1].contains("authorization: Bearer tok-123\r\n"));
        });
    }

    #[test]
    fn unverified_emails_are_not_returned() {
        run(async {
            for me in [
                r#"{"sub":"1","email":"a@x.com","email_verified":false}"#,
                r#"{"sub":"1","email":"a@x.com"}"#,
                r#"{"sub":"1","email":"a@x.com","email_verified":"false"}"#,
            ] {
                let (base, _) = provider(vec![(200, r#"{"access_token":"t"}"#), (200, me)]).await;
                let p = test_provider(&base);
                let (url, cookie, _) = started(&p);
                let mut cx = callback(&cookie, &format!("code=c&state={}", param(&url, "state")));
                assert_eq!(p.finish(&mut cx).await.unwrap().email, None, "{me}");
            }
        });
    }

    #[test]
    fn github_takes_the_verified_primary_email() {
        run(async {
            let (base, _) = provider(vec![
                (200, r#"{"access_token":"t"}"#),
                (200, r#"{"id":583231,"login":"octo","name":null,"email":"public@x.com"}"#),
                (200, r#"[{"email":"a@x.com","primary":false,"verified":true},{"email":"b@x.com","primary":true,"verified":false},{"email":"c@x.com","primary":true,"verified":true}]"#),
            ])
            .await;
            let p = Provider {
                github: true,
                ..test_provider(&base)
            };
            let (url, cookie, _) = started(&p);
            let mut cx = callback(&cookie, &format!("code=c&state={}", param(&url, "state")));
            let who = p.finish(&mut cx).await.unwrap();
            assert_eq!(
                (who.id.as_str(), who.email.as_deref(), who.name.as_deref()),
                ("583231", Some("c@x.com"), Some("octo"))
            );
        });
    }

    #[test]
    fn a_callback_not_started_here_is_refused() {
        run(async {
            // No provider is called: a refused callback never gets that far.
            let p = test_provider("http://127.0.0.1:1");
            let (url, cookie, _) = started(&p);
            let good = param(&url, "state");
            let refuse = |mut cx: Cx, why: &'static str| {
                let p = p.clone();
                async move {
                    let e = p.finish(&mut cx).await.unwrap_err();
                    assert_eq!((e.status(), e.code()), (400, "oauth_failed"), "{why}");
                    assert!(cx.cookie("wisp_oauth_fake").is_none(), "used up: {why}");
                }
            };
            refuse(callback(&cookie, "code=c&state=forged"), "wrong state").await;
            refuse(callback(&cookie, "code=c"), "no state").await;
            refuse(callback(&cookie, &format!("state={good}")), "no code").await;
            refuse(
                callback(&cookie, &format!("code=&state={good}")),
                "empty code",
            )
            .await;
            refuse(
                callback(&cookie, &format!("code={}&state={good}", "x".repeat(3000))),
                "huge code",
            )
            .await;
            refuse(
                Cx::for_test(
                    &format!("GET /cb?code=c&state={good} HTTP/1.1\r\n\r\n"),
                    &[],
                ),
                "no cookie: login CSRF",
            )
            .await;
            // A state cookie the visitor made up, or changed, is not signed.
            refuse(
                callback("wisp_oauth_fake=forged.v", "code=c&state=forged"),
                "unsigned cookie",
            )
            .await;
            let tampered = cookie.replacen('.', "x.", 1);
            refuse(
                callback(&tampered, &format!("code=c&state={good}")),
                "tampered cookie",
            )
            .await;
            // The visitor said no.
            let mut cx = callback(&cookie, &format!("error=access_denied&state={good}"));
            assert_eq!(p.finish(&mut cx).await.unwrap_err().status(), 403);
        });
    }

    #[test]
    fn a_provider_that_fails_gives_no_profile_and_leaks_nothing() {
        run(async {
            for answers in [
                vec![(400, r#"{"error":"bad_verification_code"}"#)],
                vec![(200, r#"{"error":"nope"}"#)],
                vec![(200, "not json")],
                vec![
                    (200, r#"{"access_token":"t"}"#),
                    (401, r#"{"echo":"the-secret"}"#),
                ],
                vec![
                    (200, r#"{"access_token":"t"}"#),
                    (200, r#"{"email":"a@x.com","email_verified":true}"#),
                ],
            ] {
                let (base, _) = provider(answers.clone()).await;
                let p = test_provider(&base);
                let (url, cookie, _) = started(&p);
                let mut cx = callback(&cookie, &format!("code=c&state={}", param(&url, "state")));
                let e = p.finish(&mut cx).await.unwrap_err();
                assert!(matches!(e.status(), 400 | 502), "{answers:?}");
                assert!(
                    !e.message().contains("the-secret")
                        && !e.message().contains("bad_verification"),
                    "{}",
                    e.message()
                );
            }
        });
    }

    #[test]
    fn endpoints_must_be_https() {
        for url in [
            "https://x.example/t",
            "http://127.0.0.1:80/t",
            "http://localhost:3000/t",
            "http://[::1]:1/t",
            "http://127.0.0.1",
        ] {
            assert!(secure_endpoint(url).is_ok(), "{url}");
        }
        for url in [
            "http://idp.example/t",
            "http://127.0.0.1.evil.example/t",
            "http://localhost.evil.example/",
            "ftp://x",
            "",
        ] {
            assert!(secure_endpoint(url).is_err(), "{url}");
        }
        let p = Provider::new(
            "fake",
            "http://idp.example/auth",
            "https://t",
            "https://u",
            "x",
        )
        .keys("i", "s");
        let mut cx = Cx::for_test("GET /a HTTP/1.1\r\n\r\n", &[]);
        assert_eq!(p.start(&mut cx).unwrap_err().status(), 500);
    }

    #[test]
    fn keys_come_from_the_environment_or_say_so() {
        let p = Provider::new(
            "wisp_unset_probe",
            "https://a",
            "https://t",
            "https://u",
            "x",
        );
        let e = p.keys_or_env().unwrap_err();
        assert!(e.message().contains("WISP_UNSET_PROBE_CLIENT_ID") && e.status() == 500);
    }
}
