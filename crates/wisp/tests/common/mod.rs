//! A small app written by hand, so tests can drive `Cx`, `Response` and the
//! server through the public API alone: what a request does depends on its
//! path and query, and each route below is one thing an app does.

use wisp::prelude::*;
use wisp::rt::MAX_PARAMS;
use wisp::{App, Asset, ExportRoute, Message, Out};

pub struct Lab;

/// A value for `cx.set`.
pub struct Note(pub u32);

/// A value for `wisp::provide`.
pub struct Motto(pub &'static str);

static LIMIT: RateLimit = RateLimit::per_minute(2);

/// Where the app's files are: dev builds read `static/` from here.
pub const ROOT: &str = env!("CARGO_TARGET_TMPDIR");

/// The template `/_wisp/dev/swap` may replace, and its shape.
pub const TEMPLATE: (&str, u64) = ("src/t.wisp", 0x1234);

/// What `/asset.txt` is, from `static/` in dev builds and the table in release ones.
static ASSET: Asset = Asset {
    body: b"asset",
    ext: "txt",
    etag: "\"v1\"",
};

impl App for Lab {
    const ROOT: &'static str = ROOT;
    const CSS: Option<&'static str> = None;
    const PARAMS: &'static [&'static [&'static str]] = &[&[], &["id"]];
    const TEMPLATES: &'static [(&'static str, u64)] = &[TEMPLATE];

    fn route<'a>(_path: &'a str, segs: &[&'a str]) -> Option<(usize, [&'a str; MAX_PARAMS])> {
        let mut params = [""; MAX_PARAMS];
        match segs {
            ["nowhere"] => None,
            ["item", id] => {
                params[0] = id;
                Some((1, params))
            }
            _ => Some((0, params)),
        }
    }

    /// `/item/[id]` takes 16 bytes.
    fn body_limit(route: usize) -> Option<usize> {
        (route == 1).then_some(16)
    }

    fn shell() -> [&'static str; 3] {
        [
            "<!doctype html><html><head>",
            "</head><body>",
            "</body></html>",
        ]
    }

    fn asset(path: &str) -> Option<&'static Asset> {
        (path == "/asset.txt").then_some(&ASSET)
    }

    fn export_routes() -> Vec<ExportRoute> {
        let page = |pattern, entries| ExportRoute {
            pattern,
            page: true,
            actions: false,
            server: false,
            entries,
        };
        vec![
            page("/hello", None),
            page(
                "/item/[id]",
                Some(|| vec![vec!["a".into()], vec!["b.txt".into()]]),
            ),
            page("/nowhere", None),
            page("/thing/[name]", None),
            page(
                "/other/[name]",
                Some(|| vec![vec!["fine".into()], vec!["..".into()]]),
            ),
            page("/opt/[[name]]", None),
            page("/bad/[a]/[b]", Some(|| vec![vec!["only-one".into()]])),
            ExportRoute {
                actions: true,
                ..page("/status", None)
            },
            ExportRoute {
                server: true,
                page: false,
                ..page("/q", None)
            },
        ]
    }

    async fn init() -> Result<()> {
        wisp::provide(Motto("lab"));
        Ok(())
    }

    fn handle(
        _route: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
    ) -> impl Future<Output = Result<()>> + Send {
        lab(cx, out)
    }

    async fn error(
        _route: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> Result<()> {
        if cx.path().starts_with("/default-error") {
            wisp::rt::default_error(cx, out, status, message);
        } else {
            out.body.push_str(&format!("[{status}:{message}]"));
        }
        Ok(())
    }
}

/// Files `Response::file_in` reads.
pub const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

/// How many streams from `/forever` have stopped.
static ENDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Events until the client goes.
async fn tick(tx: &wisp::Sender) -> Result<(), wisp::Gone> {
    loop {
        tx.event("tick").await?;
        wisp::sleep(std::time::Duration::from_millis(1)).await;
    }
}

fn send(out: &mut Out, r: Response) -> Result<()> {
    wisp::rt::respond(out, r);
    Ok(())
}

fn text(out: &mut Out, s: impl Into<String>) -> Result<()> {
    send(out, Response::text(s))
}

/// The query's `name`, or "".
fn arg(cx: &Cx, name: &str) -> String {
    cx.query(name).unwrap_or_default().into_owned()
}

async fn lab(cx: &mut Cx, out: &mut Out) -> Result<()> {
    let path = cx.path().to_string();
    match path.as_str() {
        "/hello" => {
            out.head.push_str("<title>lab</title>");
            out.body.push_str("<h1>hello</h1>");
            Ok(())
        }
        "/q" => {
            let (n, s) = (cx.query_or("n", 5u32), arg(cx, "s"));
            text(out, format!("{}|{n}|{s}", cx.query_string()))
        }
        "/host" => text(out, format!("{:?}", cx.host())),
        "/set-cookie" => {
            let flags = arg(cx, "o");
            let has = |c: char| flags.contains(c);
            let options = CookieOptions {
                max_age: (!has('x')).then(|| std::time::Duration::from_secs(60)),
                script_readable: has('r'),
                same_site: if has('s') {
                    SameSite::Strict
                } else if has('n') {
                    SameSite::None
                } else {
                    SameSite::Lax
                },
                path: if has('p') { "/p" } else { "/" },
                domain: has('d').then_some("example.com"),
                signed: has('g'),
            };
            cx.set_cookie_with("a", arg(cx, "v"), options);
            text(out, "set")
        }
        "/get-cookie" => {
            let seen = format!(
                "{:?}|{}|{:?}|{}",
                cx.cookie("a"),
                cx.cookie_or("a", 0u32),
                cx.signed_cookie("a"),
                cx.signed_cookie_or("a", 9u32)
            );
            text(out, seen)
        }
        "/delete-cookie" => {
            cx.delete_cookie("a");
            let gone = cx.cookie("a");
            text(out, format!("{gone:?}"))
        }
        "/sign" => {
            cx.set_signed_cookie("user", arg(cx, "v"));
            text(out, "signed")
        }
        "/who" => {
            let user = cx.signed_cookie("user").unwrap_or("nobody").to_string();
            text(out, user)
        }
        "/flash" => {
            cx.flash(&arg(cx, "m"));
            redirect("/flashed")
        }
        "/flashed" => {
            let (first, second) = (cx.flashed(), cx.flashed());
            text(out, format!("{first:?}|{second:?}"))
        }
        "/bearer" => {
            // Cargo sets this for every test, so the key is known: "wisp".
            cx.need_bearer("CARGO_PKG_NAME")?;
            text(out, cx.bearer().unwrap_or(""))
        }
        "/unset-key" => {
            cx.need_bearer("WISP_SURELY_NOT_SET")?;
            text(out, "in")
        }
        "/basic" => text(out, format!("{:?}", cx.basic_auth())),
        "/writes" => text(out, cx.writes().to_string()),
        "/cors" => {
            let origins = arg(cx, "o");
            match cx.cors(&origins) {
                Some(preflight) => send(out, preflight),
                None => text(out, "body"),
            }
        }
        "/status" => {
            cx.set_status(cx.query_or("n", 200));
            out.body.push_str("status");
            Ok(())
        }
        "/state" => {
            cx.set(Note(7));
            cx.set(Note(8));
            let seen = cx.get::<Note>().map(|n| n.0);
            let taken = cx.take::<Note>().map(|n| n.0);
            let after = cx.take::<Note>().map(|n| n.0);
            text(
                out,
                format!("{seen:?}|{taken:?}|{after:?}|{}", wisp::state::<Motto>().0),
            )
        }
        "/fail" => {
            cx.fail(422, Note(3));
            let note = cx.get::<Note>().map_or(0, |n| n.0);
            out.body.push_str(&format!("failed {note}"));
            Ok(())
        }
        "/id" => text(out, cx.request_id().to_string()),
        "/peer" => text(out, format!("{}|{}", cx.peer(), cx.client_ip())),
        "/limit" => {
            LIMIT.check(cx.client_ip())?;
            text(out, "ok")
        }
        "/form" => {
            wisp::rt::check_origin(cx)?;
            text(out, "posted")
        }
        "/header" => {
            // Input that reaches a header: a bug of the app's, which must not split a response.
            cx.set_header("x-echo", arg(cx, "v"));
            text(out, "echoed")
        }
        "/error-header" => Err(Error::new(400, "no").with_header("x-echo", arg(cx, "v"))),
        "/redirect-to" => redirect(arg(cx, "to")),
        "/err" => match arg(cx, "k").as_str() {
            "teapot" => error(418, "short and stout"),
            "empty" => error(503, ""),
            "invalid" => Err(Error::invalid("a", "is bad").and("b", "is worse")),
            "retry" => Err(Error::new(429, "slow down").with_header("retry-after", "5")),
            "moved" => Err(Error::redirect(308, "/new")),
            "or404" => None::<u8>.or_404().map(|_| ()),
            "or400" => "x".parse::<u8>().or_status(400).map(|_| ()),
            "io" => Err(std::io::Error::other("disk on fire").into()),
            "panic" => panic!("boom"),
            _ => text(out, "no error"),
        },
        "/default-error" => error(404, "gone"),
        "/json" => match arg(cx, "k").as_str() {
            "created" => send(out, Response::created(&vec![1u8, 2])),
            _ => send(out, Response::json_of(&vec!["a\"b", "c"])),
        },
        "/resp" => match arg(cx, "k").as_str() {
            "html" => send(out, Response::html("<p>x</p>")),
            "download" => send(out, Response::download(&arg(cx, "n"), "a,b\n")),
            "redirect" => redirect("/there"),
            "empty" => send(out, Response::empty(204).with_header("x-a", "1")),
            "status" => send(out, Response::text("teapot").with_status(418)),
            _ => text(out, "plain"),
        },
        "/file" => send(out, Response::file_in(DATA, &arg(cx, "n")).await?),
        "/stream" => send(
            out,
            Response::stream("text/plain", |tx| async move {
                tx.send("a").await?;
                tx.send("b").await?;
                Ok(())
            }),
        ),
        "/events" => send(
            out,
            Response::events(|tx| async move {
                tx.event("one").await?;
                tx.event("two\nlines\r\nthree").await?;
                Ok(())
            }),
        ),
        "/ws" => send(
            out,
            Response::websocket(|ws| async move {
                while let Some(m) = ws.recv().await {
                    match m {
                        Message::Text(t) => ws.send(t).await?,
                        Message::Binary(b) => ws.send(b).await?,
                    }
                }
                Ok(())
            }),
        ),
        "/ws-fail" => send(
            out,
            Response::websocket(|ws| async move {
                ws.send("before").await?;
                error(500, "the handler failed")
            }),
        ),
        "/forever" => send(
            out,
            Response::events(|tx| async move {
                let done = tick(&tx).await;
                ENDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                done
            }),
        ),
        "/ended" => text(
            out,
            ENDED.load(std::sync::atomic::Ordering::Relaxed).to_string(),
        ),
        "/room" => send(
            out,
            Response::websocket(|ws| async move {
                wisp::channel("room").connect(&ws).await?;
                Ok(())
            }),
        ),
        "/live" => {
            let mut news = wisp::channel("live").subscribe();
            send(
                out,
                Response::events(|tx| async move {
                    while let Some(m) = news.recv().await {
                        tx.event(&m).await?;
                    }
                    Ok(())
                }),
            )
        }
        "/announce" => text(out, wisp::channel("live").send(arg(cx, "m")).to_string()),
        "/meta" => {
            let header = |name| cx.header(name).unwrap_or("-").to_string();
            let seen = format!(
                "{} {} {} {}",
                cx.method.as_str(),
                header("content-type"),
                header("authorization"),
                header("cookie")
            );
            text(out, seen)
        }
        "/sleep" => {
            wisp::sleep(std::time::Duration::from_millis(5)).await;
            text(out, "slept")
        }
        "/echo" => {
            let body = String::from_utf8_lossy(cx.body()).into_owned();
            text(out, format!("{}:{body}", cx.body().len()))
        }
        #[cfg(feature = "tower")]
        _ if path.starts_with("/proxy/") => {
            send(out, wisp::tower::call(&mut behind::Inner, cx).await?)
        }
        _ if path.starts_with("/item/") => {
            let echo = format!("item {} {}", cx.param("id"), cx.body().len());
            text(out, echo)
        }
        _ => error(404, "Not Found"),
    }
}

/// A tower service behind the app, which `/proxy` forwards a request to.
#[cfg(feature = "tower")]
pub mod behind {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use wisp::tower::http::{Request, Response};
    use wisp::tower::http_body::{Frame, SizeHint};
    use wisp::tower::{Body, Bytes};

    /// Answers by the end of the path: `/full` with a body of known size, `/stream` with
    /// one that is not, `/fail` not at all.
    pub struct Inner;

    impl wisp::tower::tower_service::Service<Request<Body>> for Inner {
        type Response = Response<Live>;
        type Error = std::io::Error;
        type Future = std::future::Ready<Result<Response<Live>, std::io::Error>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: Request<Body>) -> Self::Future {
            let path = req.uri().path().to_string();
            let seen = format!(
                "{} {} {:?}",
                req.method(),
                req.uri(),
                req.headers().get("x-in").map(|v| v.to_str().unwrap_or(""))
            );
            std::future::ready(if path.ends_with("/fail") {
                Err(std::io::Error::other("behind is down"))
            } else if path.ends_with("/stream") {
                Ok(reply(Live::Parts(vec![seen.into(), "|two".into()]), 201))
            } else {
                Ok(reply(Live::Whole(Some(seen.into())), 200))
            })
        }
    }

    fn reply(body: Live, status: u16) -> Response<Live> {
        let mut res = Response::new(body);
        *res.status_mut() = status.try_into().unwrap();
        let h = res.headers_mut();
        h.insert("content-type", "text/x-behind".parse().unwrap());
        h.insert("content-length", "999".parse().unwrap());
        h.insert("date", "never".parse().unwrap());
        h.insert("x-kept", "yes".parse().unwrap());
        h.append("set-cookie", "a=1".parse().unwrap());
        h.append("set-cookie", "b=2".parse().unwrap());
        res
    }

    /// A body that is whole, or in parts whose total is not known up front.
    pub enum Live {
        Whole(Option<Bytes>),
        Parts(Vec<Bytes>),
    }

    impl wisp::tower::http_body::Body for Live {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(match self.get_mut() {
                Live::Whole(b) => b.take().map(|b| Ok(Frame::data(b))),
                Live::Parts(p) if p.is_empty() => None,
                Live::Parts(p) => Some(Ok(Frame::data(p.remove(0)))),
            })
        }

        fn size_hint(&self) -> SizeHint {
            match self {
                Live::Whole(b) => SizeHint::with_exact(b.as_ref().map_or(0, |b| b.len() as u64)),
                Live::Parts(_) => SizeHint::default(),
            }
        }
    }
}
