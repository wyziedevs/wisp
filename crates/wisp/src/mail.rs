//! Email: `wisp::mail(to, subject, html).await?`.
//!
//! It goes out through an HTTP provider (so it needs [`crate::fetch`]'s
//! `tls` feature on a server, nothing on the edge), chosen by what the
//! environment holds, and from the address in `MAIL_FROM`:
//!
//! - `RESEND_API_KEY`: Resend.
//! - `POSTMARK_TOKEN`: Postmark.
//!
//! In dev with neither set, the mail is printed to the log (a magic link
//! to click). Outside dev with neither, it is a 500 that says so, not a
//! mail that silently went nowhere. SMTP and SES are not here: SMTP wants
//! STARTTLS, SES a request signature; both are an app's own `fetch` away.
//!
//! The recipient is checked as an `Email` is (one address, no names,
//! lists or line breaks), and the subject may not hold a line break, so a
//! visitor's input cannot add recipients or headers. The key goes only in
//! the request to the provider, over `https`; no error or log line holds
//! it, nor the body of the mail, outside dev's print.

use crate::json::to_json;
use crate::{Error, Request, Result};

/// Sends an HTML email. `to` is one address; `subject` one line.
pub async fn mail(to: &str, subject: &str, html: &str) -> Result {
    if let Some(problem) = crate::json::check::email(&to.to_string()) {
        return Err(Error::new(500, format!("mail: the recipient {problem}")));
    }
    if subject.contains(['\r', '\n']) {
        return Err(Error::new(500, "mail: the subject has a line break"));
    }
    let from = crate::env("MAIL_FROM").filter(|f| !f.is_empty());
    let provider = if let Some(key) = crate::env("RESEND_API_KEY").filter(|k| !k.is_empty()) {
        Some(resend(
            "https://api.resend.com/emails",
            &key,
            from.as_deref(),
            to,
            subject,
            html,
        ))
    } else {
        crate::env("POSTMARK_TOKEN")
            .filter(|k| !k.is_empty())
            .map(|key| {
                postmark(
                    "https://api.postmarkapp.com/email",
                    &key,
                    from.as_deref(),
                    to,
                    subject,
                    html,
                )
            })
    };
    match provider {
        Some(req) => deliver(req?).await,
        None if crate::settings().dev => {
            crate::http::log(format_args!(
                "wisp: mail (dev, not sent) to {to}: {subject}\n{html}"
            ));
            Ok(())
        }
        None => Err(Error::new(
            500,
            "mail is not set up: set RESEND_API_KEY or POSTMARK_TOKEN, and MAIL_FROM",
        )),
    }
}

fn no_from() -> Error {
    Error::new(
        500,
        "mail is not set up: set MAIL_FROM, the address mail comes from",
    )
}

fn resend(
    url: &str,
    key: &str,
    from: Option<&str>,
    to: &str,
    subject: &str,
    html: &str,
) -> Result<Request> {
    let mut req = json_post(
        url,
        &format!(
            "{{\"from\":{},\"to\":[{}],\"subject\":{},\"html\":{}}}",
            to_json(from.ok_or_else(no_from)?),
            to_json(to),
            to_json(subject),
            to_json(html)
        ),
    );
    req.header("authorization", &format!("Bearer {key}"));
    Ok(req)
}

fn postmark(
    url: &str,
    key: &str,
    from: Option<&str>,
    to: &str,
    subject: &str,
    html: &str,
) -> Result<Request> {
    let mut req = json_post(
        url,
        &format!(
            "{{\"From\":{},\"To\":{},\"Subject\":{},\"HtmlBody\":{}}}",
            to_json(from.ok_or_else(no_from)?),
            to_json(to),
            to_json(subject),
            to_json(html)
        ),
    );
    req.header("x-postmark-server-token", key);
    Ok(req)
}

fn json_post(url: &str, body: &str) -> Request {
    let mut req = Request::new("POST", url);
    req.header("content-type", "application/json");
    req.header("accept", "application/json");
    req.body = body.as_bytes().to_vec();
    req
}

/// Sends the request; anything but a 2xx is a 502 that names the status
/// only, for the provider's reply may echo what was sent.
async fn deliver(req: Request) -> Result {
    let reply = crate::fetch(req).await?;
    if (200..300).contains(&reply.status) {
        return Ok(());
    }
    Err(Error::new(
        502,
        format!("the mail provider answered {}", reply.status),
    ))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn run<T>(f: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// Answers one request with `status`, and returns what it was sent.
    async fn provider(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://127.0.0.1:{}/email",
            listener.local_addr().unwrap().port()
        );
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.ends_with(b"}") {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            let answer = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: 21\r\n\r\n{{\"echo\":\"s3cret-key\"}}"
            );
            s.write_all(answer.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, task)
    }

    #[test]
    fn resend_and_postmark_requests() {
        run(async {
            let (url, got) = provider(200).await;
            let req = resend(
                &url,
                "s3cret-key",
                Some("Me <me@x.com>"),
                "ann@x.com",
                "Hi \"you\"",
                "<p>Hello</p>",
            )
            .unwrap();
            deliver(req).await.unwrap();
            let sent = got.await.unwrap();
            assert!(sent.contains("authorization: Bearer s3cret-key\r\n"));
            assert!(sent.contains(r#"{"from":"Me \u003cme@x.com\u003e","to":["ann@x.com"],"subject":"Hi \"you\"","html":"\u003cp\u003eHello\u003c/p\u003e"}"#));

            let (url, got) = provider(200).await;
            let req = postmark(
                &url,
                "s3cret-key",
                Some("me@x.com"),
                "ann@x.com",
                "Hi",
                "<p>Hello</p>",
            )
            .unwrap();
            deliver(req).await.unwrap();
            let sent = got.await.unwrap();
            assert!(sent.contains("x-postmark-server-token: s3cret-key\r\n"));
            assert!(sent.contains(
                r#""To":"ann@x.com","Subject":"Hi","HtmlBody":"\u003cp\u003eHello\u003c/p\u003e""#
            ));
        });
    }

    #[test]
    fn a_refusal_names_the_status_and_no_secret() {
        run(async {
            let (url, _) = provider(422).await;
            let req = resend(
                &url,
                "s3cret-key",
                Some("me@x.com"),
                "ann@x.com",
                "Hi",
                "<p>Hello</p>",
            )
            .unwrap();
            let e = deliver(req).await.unwrap_err();
            assert_eq!(e.status(), 502);
            assert!(
                !e.message().contains("s3cret") && e.message().contains("422"),
                "{}",
                e.message()
            );
        });
    }

    #[test]
    fn recipients_and_subjects_cannot_add_more() {
        run(async {
            for to in [
                "a@x.com,b@y.com",
                "a@x.com\r\nbcc: b@y.com",
                "Ann <a@x.com>",
                "a@x.com b@y.com",
                "",
                "nobody",
            ] {
                let e = mail(to, "Hi", "<p>x</p>").await.unwrap_err();
                assert!(e.message().contains("recipient"), "{to:?}");
            }
            let e = mail("a@x.com", "Hi\r\nbcc: b@y.com", "x")
                .await
                .unwrap_err();
            assert!(e.message().contains("line break"));
            // A body is JSON-escaped, so it cannot break out of its string.
            let req = resend(
                "http://x/",
                "k",
                Some("me@x.com"),
                "a@x.com",
                "s",
                "\",\"to\":[\"evil@x.com\"]",
            )
            .unwrap();
            let body = String::from_utf8(req.body).unwrap();
            assert!(
                body.contains(r#"\",\"to\":[\"evil@x.com\"]"#)
                    && body.matches("\"to\":").count() == 1
            );
        });
    }

    #[test]
    fn a_from_is_needed() {
        assert!(
            resend("http://x/", "k", None, "a@x.com", "s", "h")
                .err()
                .unwrap()
                .message()
                .contains("MAIL_FROM")
        );
    }
}
