# Auth and integrations

What goes beyond `cx.login`, `cx.user` and sessions. Everything here is
opt-in: an app that calls none of it carries none of it, and no request
pays for it.

## Roles and CORS

```rust
let admin = cx.need(&USERS, |u| u.admin)?;   // Row<User>
```

Signed out is `cx.user`'s error (303 to sign in, 401 for a JSON client);
signed in but not allowed is a 403.

```rust
fn before(cx: &mut Cx) -> Result {
    cx.cors("*")?;           // or "https://app.example.com https://x.com"
    Ok(())
}
```

A preflight is answered with a 204 and the headers, carried as the `Err`
that `?` returns. A site not listed gets no `access-control-allow-origin`.

## Signed tokens (reset, verify, magic link)

```rust
let t = wisp::token("reset", &user.id, Duration::from_secs(3600));
let id: u64 = wisp::untoken("reset", &t)?;   // 400 for any failure, the same one
```

HMAC-SHA256 under `WISP_SECRET` (and `WISP_SECRET_OLD`) over purpose,
payload and expiry. The payload is readable, not forgeable; keep secrets
out of it. A token works until it expires: for a reset, put something in
it that changes when it is used (the password hash's first bytes) and
compare. A purpose of `""` is none.

## Two-factor codes

```rust
let secret = wisp::totp::secret();                       // keep with the user
let uri = wisp::totp::uri("My App", &user.email, &secret); // QR code or text
wisp::totp::check(&user.totp, &code)                     // ±1 step (30 s)
wisp::totp::check_step(..)                               // the step, to refuse a replay
```

RFC 6238, SHA-1, six digits. Six digits can be guessed: put a `RateLimit`
on the check, by user.

## Outbound HTTP

```rust
let mut req = wisp::Request::new("POST", "https://api.example.com/rows");
req.header("authorization", &format!("Bearer {key}"));
req.body = json.into_bytes();
let reply = wisp::fetch(req).await?;      // reply.status, reply.text()
```

`http://` works as is. `https://` needs the `tls` feature
(`wisp = { version = "..", features = ["tls"] }`: rustls and the web's root
certificates, no OpenSSL). One request per connection, no redirects, 30 s,
16 MB. Errors name the host only. In the edge build it is the host's
`fetch`. A URL a visitor chose is for the app to check before calling.

## Email

```rust
wisp::mail("ann@example.com", "Your link", &html).await?;
```

Set `MAIL_FROM` and `RESEND_API_KEY` or `POSTMARK_TOKEN` (needs `tls`).
In dev with neither, the mail is printed to the log. Outside dev with
neither it is a 500. The recipient is one address; a subject with a line
break is refused. SMTP and SES are not built in: call their HTTP APIs with
`fetch`.

## Sign in with GitHub, Google or OpenID Connect

```rust
// src/routes/auth/github/+server.rs
fn get(cx: &mut Cx) -> Result { wisp::oauth::github().start(cx) }

// src/routes/auth/github/callback/+server.rs
async fn get(cx: &mut Cx) -> Result {
    let who = wisp::oauth::github().finish(cx).await?;   // id, email (verified only), name
    let id = match USERS.find(|u| u.github == who.id) { Some(u) => u.id, None => /* sign up */ };
    cx.sign_in(id);
    redirect("/")
}
```

Keys: `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` (`GOOGLE_...`, and
`{NAME}_...` for `oidc(name, issuer).await?`), or `.keys(id, secret)`.
Register `{ORIGIN}/auth/github/callback` with the provider; set `ORIGIN`
in production, or use `.redirect(url)`.

Kept safe by: a random `state` in a signed 10-minute cookie, used up by
the first callback and compared in constant time; PKCE (S256); the secret
only in the POST body to an `https` endpoint; no key or token in any error
or log; only a provider-verified email returned (match accounts on `id`,
not email, unless you want a sign-in to join an existing account); every
failure the same 400.

## Not here

Distributed `RateLimit` waits for the relay (S2). Automatic
`/_wisp/oauth/...` routes need `http.rs`; the two small routes above do the
same. SMTP, SES.
