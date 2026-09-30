//! Wisp as a `tower` service, and tower services inside Wisp (the `tower`
//! feature).
//!
//! - In axum: `Router::new().route("/api/x", get(x)).fallback_service(wisp::tower::service::<App>().await?)`.
//! - Behind tower middleware: `ServiceBuilder::new().layer(...).service(wisp)`, served by hyper or axum.
//! - On AWS Lambda: `lambda_http::run(wisp)`.
//! - An axum `Router` inside Wisp: `wisp::tower::call(&mut router, cx).await` from the `before`
//!   hook or a catch-all `+server.rs`.
//!
//! Requests go through the same parser and limits as the built-in server's.
//! The client's address comes from a `SocketAddr` in the request's
//! extensions, if the host puts one there. axum's `ConnectInfo<SocketAddr>`
//! is not read, since naming it would take an axum dependency: put the
//! address in with a layer, `req.extensions_mut().insert(addr)`, or set
//! `WISP_CLIENT_IP_HEADER` behind a proxy.
//!
//! A [`Response::websocket`] is answered with a 501: upgrades need Wisp's
//! own server (or a WebSocket route of the host's own, such as axum's).

use crate::http::{Reply, answer, body_limit, reason};
use crate::{App, Cx, Response};
pub use bytes::{self, Bytes};
pub use http_body::{self, Frame, SizeHint};
use std::borrow::Cow;
use std::convert::Infallible;
use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
pub use {http, tower_service};

/// The app as a tower service. Cheap to clone.
pub struct Service<A>(PhantomData<fn() -> A>);

impl<A> Clone for Service<A> {
    fn clone(&self) -> Self {
        Service(PhantomData)
    }
}

/// Runs [`crate::prepare`] and returns the app as a service.
pub async fn service<A: App>() -> std::io::Result<Service<A>> {
    crate::prepare::<A>().await?;
    Ok(Service(PhantomData))
}

impl<A: App, B> tower_service::Service<http::Request<B>> for Service<A>
where
    B: http_body::Body + Send + 'static,
    B::Data: Send,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        Box::pin(async move { Ok(response(from_http::<A, B>(req).await)) })
    }
}

async fn from_http<A: App, B: http_body::Body>(req: http::Request<B>) -> Reply {
    let (parts, body) = req.into_parts();
    let target = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let body = match collect(body, body_limit::<A>(parts.uri.path())).await {
        Ok(b) => b,
        Err(status) => return Reply::plain(status),
    };
    let peer = parts
        .extensions
        .get::<SocketAddr>()
        .copied()
        .unwrap_or((Ipv4Addr::UNSPECIFIED, 0).into());
    // HTTP/2 names the host in the URI only.
    let host = parts
        .uri
        .authority()
        .filter(|_| !parts.headers.contains_key(http::header::HOST))
        .map(|a| ("host", a.as_str().as_bytes()));
    let headers = parts
        .headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_bytes()))
        .chain(host);
    match Cx::from_request::<A>(parts.method.as_str(), target, headers, &body, peer) {
        Ok(cx) => answer::<A>(cx).await,
        Err(status) => Reply::plain(status),
    }
}

fn response(reply: Reply) -> http::Response<Body> {
    let body = match reply.body {
        crate::Body::Bytes(b) => Body::full(Bytes::from(b)),
        crate::Body::Static(b) => Body::full(Bytes::from_static(b)),
        crate::Body::Stream(rx) => Body(Inner::Stream(rx)),
        crate::Body::Page | crate::Body::Made(_) | crate::Body::WebSocket(_) => {
            unreachable!("answer renders pages, unpacks made ones and refuses upgrades")
        }
    };
    let mut res = http::Response::new(body);
    *res.status_mut() =
        http::StatusCode::from_u16(reply.status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in reply.headers {
        // `from_static` panics on what it cannot send; an app's content type
        // is any `&'static str`, so only a plain one takes the no-copy path.
        let value = match value {
            Cow::Borrowed(v) if v.bytes().all(|b| b == b'\t' || (b' '..=b'~').contains(&b)) => {
                http::HeaderValue::from_static(v)
            }
            v => match http::HeaderValue::try_from(v.into_owned()) {
                Ok(v) => v,
                Err(_) => continue,
            },
        };
        if let Ok(name) = http::HeaderName::from_bytes(name.as_bytes()) {
            res.headers_mut().append(name, value);
        }
    }
    res
}

/// A request's body, all of it, or the status to refuse it with.
async fn collect<B: http_body::Body>(body: B, limit: usize) -> Result<Vec<u8>, u16> {
    use bytes::Buf;
    let mut body = std::pin::pin!(body);
    let mut all = Vec::new();
    while let Some(frame) = poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
        let Ok(frame) = frame else { return Err(400) };
        let Ok(mut data) = frame.into_data() else {
            continue;
        };
        if all.len() + data.remaining() > limit {
            return Err(413);
        }
        while data.has_remaining() {
            let n = data.chunk().len();
            all.extend_from_slice(data.chunk());
            data.advance(n);
        }
    }
    Ok(all)
}

/// Sends the request in `cx` to a tower service (an axum `Router`) and
/// returns its response, for a hook or endpoint to answer with. A body of
/// known size is read whole; any other (a live feed) is streamed as it comes.
pub async fn call<S, B>(svc: &mut S, cx: &Cx) -> crate::Result<Response>
where
    S: tower_service::Service<http::Request<Body>, Response = http::Response<B>>,
    S::Error: std::error::Error + Send + Sync + 'static,
    B: http_body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Send,
{
    let mut req = http::Request::new(Body::full(Bytes::copy_from_slice(cx.body())));
    *req.method_mut() = http::Method::from_bytes(cx.method.as_str().as_bytes())?;
    *req.uri_mut() = match cx.query_string() {
        "" => cx.path().parse()?,
        q => format!("{}?{q}", cx.path()).parse()?,
    };
    for (name, value) in cx.headers() {
        if let (Ok(n), Ok(v)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_str(value),
        ) {
            req.headers_mut().append(n, v);
        }
    }
    req.extensions_mut().insert(cx.peer());
    poll_fn(|c| svc.poll_ready(c)).await?;
    let (parts, body) = svc.call(req).await?.into_parts();
    let content_type = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    let mut res = if body.size_hint().exact().is_some() {
        let body = collect(body, usize::MAX)
            .await
            .map_err(|s| crate::Error::new(502, reason(s)))?;
        Response::new(content_type.to_string(), body)
    } else {
        let (res, tx) = Response::channel(content_type.to_string());
        tokio::spawn(forward(body, tx));
        res
    };
    res.status = parts.status.as_u16();
    for (name, value) in &parts.headers {
        let skip = [
            "content-type",
            "content-length",
            "transfer-encoding",
            "connection",
            "date",
        ]
        .contains(&name.as_str());
        if let (false, Ok(v)) = (skip, value.to_str()) {
            res.headers
                .push((Cow::Owned(name.as_str().to_string()), v.to_string()));
        }
    }
    Ok(res)
}

/// A body's chunks, sent on until it ends, fails, or nobody is listening.
async fn forward<B: http_body::Body>(body: B, tx: crate::Sender) {
    use bytes::Buf;
    let mut body = std::pin::pin!(body);
    while let Some(Ok(frame)) = poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
        let Ok(mut data) = frame.into_data() else {
            continue;
        };
        let mut chunk = Vec::with_capacity(data.remaining());
        while data.has_remaining() {
            let n = data.chunk().len();
            chunk.extend_from_slice(data.chunk());
            data.advance(n);
        }
        if tx.send(chunk).await.is_err() {
            return;
        }
    }
}

/// A response's body, or a request's for [`call`].
pub struct Body(Inner);

enum Inner {
    Full(Option<Bytes>),
    Stream(mpsc::Receiver<Vec<u8>>),
}

impl Body {
    pub fn full(bytes: impl Into<Bytes>) -> Body {
        Body(Inner::Full(Some(bytes.into())))
    }
}

impl http_body::Body for Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match &mut self.get_mut().0 {
            Inner::Full(b) => Poll::Ready(b.take().map(|b| Ok(Frame::data(b)))),
            Inner::Stream(rx) => rx
                .poll_recv(cx)
                .map(|c| c.map(|c| Ok(Frame::data(Bytes::from(c))))),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.0, Inner::Full(None))
    }

    fn size_hint(&self) -> SizeHint {
        match &self.0 {
            Inner::Full(b) => SizeHint::with_exact(b.as_ref().map_or(0, |b| b.len() as u64)),
            Inner::Stream(_) => SizeHint::default(),
        }
    }
}
