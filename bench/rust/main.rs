//! The Rust servers bench/ measures against Wisp, serving the same
//! /fortunes, /plaintext and /json as bench/app, all rendering with Askama
//! (templates compiled to Rust, like Wisp) and serializing with serde:
//! Actix Web and Axum, the popular ones, and may-minihttp, xitca-web, ntex
//! and bare hyper, TechEmpower's top tier.
//!
//!   bench-rust actix|axum|may|xitca|ntex|hyper    PORT sets the port, THREADS the worker count

use askama::Template;

struct Fortune {
    id: u32,
    message: &'static str,
}

#[derive(Template)]
#[template(
    ext = "html",
    source = r#"<!DOCTYPE html>
<html>
<head><title>Fortunes</title></head>
<body><table>
<tr><th>id</th><th>message</th></tr>
{% for f in fortunes %}
<tr><td>{{ f.id }}</td><td>{{ f.message }}</td></tr>
{% endfor %}
</table></body>
</html>
"#
)]
struct Fortunes {
    fortunes: Vec<Fortune>,
}

const ROWS: [(u32, &str); 12] = [
    (1, "fortune: No such file or directory"),
    (2, "A computer scientist is someone who fixes things that aren't broken."),
    (3, "After enough decimal places, nobody gives a damn."),
    (4, "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"),
    (5, "A computer program does what you tell it to do, not what you want it to do."),
    (6, "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"),
    (7, "Any program that runs right is obsolete."),
    (8, "A list is only as strong as its weakest link. — Donald Knuth"),
    (9, "Feature: A bug with seniority."),
    (10, "Computers make very fast, very accurate mistakes."),
    (11, "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"),
    (12, "フレームワークのベンチマーク"),
];

fn fortunes() -> String {
    let mut fortunes = Vec::with_capacity(ROWS.len() + 1);
    fortunes.extend(ROWS.iter().map(|&(id, message)| Fortune { id, message }));
    fortunes.push(Fortune { id: 0, message: "Additional fortune added at request time." });
    fortunes.sort_unstable_by(|a, b| a.message.cmp(b.message));
    Fortunes { fortunes }.render().expect("render")
}

/// TechEmpower's "json": serialized per request, with serde.
#[derive(serde::Serialize)]
struct Message {
    message: &'static str,
}

const MESSAGE: Message = Message { message: "Hello, World!" };

fn main() -> std::io::Result<()> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let port: u16 = env("PORT").parse().unwrap_or(3000);
    let threads: usize = env("THREADS").parse().unwrap_or_else(|_| std::thread::available_parallelism().map_or(1, |n| n.get()));
    match std::env::args().nth(1).as_deref() {
        Some("actix") => actix(port, threads),
        Some("axum") => axum(port, threads),
        Some("may") => may(port, threads),
        Some("xitca") => xitca(port, threads),
        Some("ntex") => ntex(port, threads),
        Some("hyper") => hyper(port, threads),
        _ => {
            eprintln!("usage: bench-rust actix|axum|may|xitca|ntex|hyper");
            std::process::exit(2);
        }
    }
}

/// What `#[actix_web::main]` expands to, with the worker count set: one
/// single-threaded runtime per worker.
fn actix(port: u16, threads: usize) -> std::io::Result<()> {
    use actix_web::{App, HttpResponse, HttpServer, web};
    let server = HttpServer::new(|| {
        App::new()
            .route("/plaintext", web::get().to(|| async { "Hello, World!" }))
            .route("/fortunes", web::get().to(|| async { HttpResponse::Ok().content_type("text/html; charset=utf-8").body(fortunes()) }))
            .route("/json", web::get().to(|| async { web::Json(MESSAGE) }))
    });
    actix_web::rt::System::new().block_on(server.workers(threads).bind(("127.0.0.1", port))?.run())
}

/// What `#[tokio::main]` expands to, with the worker count set: one
/// work-stealing runtime shared by all threads.
fn axum(port: u16, threads: usize) -> std::io::Result<()> {
    use axum::{Router, response::Html, routing::get};
    let app = Router::new().route("/plaintext", get(|| async { "Hello, World!" })).route("/fortunes", get(|| async { Html(fortunes()) })).route("/json", get(|| async { axum::Json(MESSAGE) }));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        axum::serve(listener, app).await
    })
}

/// may-minihttp: stackful coroutines on `THREADS` scheduler threads and a
/// hand-rolled HTTP/1 codec, matched on the path as its TechEmpower entry does.
fn may(port: u16, threads: usize) -> std::io::Result<()> {
    use may_minihttp::{HttpService, HttpServiceFactory, Request, Response};

    struct Service;
    impl HttpService for Service {
        fn call(&mut self, req: Request, res: &mut Response) -> std::io::Result<()> {
            match req.path() {
                "/plaintext" => res.header("Content-Type: text/plain; charset=utf-8").body("Hello, World!"),
                "/fortunes" => res.header("Content-Type: text/html; charset=utf-8").body_vec(fortunes().into_bytes()),
                "/json" => res.header("Content-Type: application/json").body_vec(serde_json::to_vec(&MESSAGE)?),
                _ => {
                    res.status_code(404, "Not Found");
                }
            }
            Ok(())
        }
    }
    struct Factory;
    impl HttpServiceFactory for Factory {
        type Service = Service;
        fn new_service(&self, _: usize) -> Service {
            Service
        }
    }
    may::config().set_workers(threads);
    Factory.start(("127.0.0.1", port))?.join().map_err(|_| std::io::Error::other("may-minihttp stopped"))
}

/// xitca-web: a thread-per-core server, `THREADS` workers.
fn xitca(port: u16, threads: usize) -> std::io::Result<()> {
    use xitca_web::{App, handler::handler_service, handler::html::Html, handler::json::Json, route::get};
    App::new()
        .at("/plaintext", get(handler_service(async || "Hello, World!")))
        .at("/fortunes", get(handler_service(async || Html(fortunes()))))
        .at("/json", get(handler_service(async || Json(MESSAGE))))
        .serve()
        .worker_threads(threads)
        .bind(("127.0.0.1", port))?
        .run()
        .wait()
}

/// ntex on its own runtime (neon: epoll on Linux), `THREADS` workers.
#[ntex::main]
async fn ntex(port: u16, threads: usize) -> std::io::Result<()> {
    use ntex::web::{self, App, HttpResponse};
    web::HttpServer::new(async || {
        App::new()
            .route("/plaintext", web::get().to(async || "Hello, World!"))
            .route("/fortunes", web::get().to(async || HttpResponse::Ok().content_type("text/html; charset=utf-8").body(fortunes())))
            .route("/json", web::get().to(async || HttpResponse::Ok().json(&MESSAGE)))
    })
    .workers(threads)
    .bind(("127.0.0.1", port))?
    .run()
    .await
}

/// Bare hyper, no framework: a single-threaded tokio runtime per thread,
/// each accepting on a socket of its own (SO_REUSEPORT) where the OS has
/// it, as hyper's TechEmpower entry does, and a match on the path.
fn hyper(port: u16, threads: usize) -> std::io::Result<()> {
    use http_body_util::Full;
    use hyper::body::{Bytes, Incoming};
    use hyper::{Request, Response, StatusCode, header, server::conn::http1, service::service_fn};

    async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let (kind, body): (&str, Bytes) = match req.uri().path() {
            "/plaintext" => ("text/plain; charset=utf-8", Bytes::from_static(b"Hello, World!")),
            "/fortunes" => ("text/html; charset=utf-8", fortunes().into()),
            "/json" => ("application/json", serde_json::to_vec(&MESSAGE).expect("json").into()),
            _ => {
                let mut res = Response::new(Full::default());
                *res.status_mut() = StatusCode::NOT_FOUND;
                return Ok(res);
            }
        };
        let mut res = Response::new(Full::new(body));
        res.headers_mut().insert(header::CONTENT_TYPE, header::HeaderValue::from_static(kind));
        Ok(res)
    }

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    #[cfg(not(unix))]
    let shared = std::net::TcpListener::bind(addr)?;
    let mut workers = Vec::with_capacity(threads);
    for _ in 0..threads {
        #[cfg(unix)]
        let listener = {
            let s = tokio::net::TcpSocket::new_v4()?;
            s.set_reuseport(true)?;
            s.bind(addr)?;
            s
        };
        #[cfg(not(unix))]
        let listener = shared.try_clone()?;
        workers.push(std::thread::spawn(move || -> std::io::Result<()> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            rt.block_on(async move {
                #[cfg(unix)]
                let listener = listener.listen(1024)?;
                #[cfg(not(unix))]
                let listener = {
                    listener.set_nonblocking(true)?;
                    tokio::net::TcpListener::from_std(listener)?
                };
                loop {
                    let Ok((stream, _)) = listener.accept().await else { continue };
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(stream), service_fn(handle)));
                }
            })
        }));
    }
    for w in workers {
        w.join().map_err(|_| std::io::Error::other("hyper worker panicked"))??;
    }
    Ok(())
}
