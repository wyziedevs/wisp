//! The Rust servers bench/ measures against Wisp, serving the same
//! /fortunes, /plaintext, /json and /page as bench/app, all rendering with
//! Askama (templates compiled to Rust, like Wisp) and serializing with serde:
//! Actix Web and Axum, the popular ones, and may-minihttp, xitca-web, ntex
//! and bare hyper, TechEmpower's top tier. Each also answers the-benchmarker's
//! `GET /`, `GET /user/:id` and `POST /user` as its entry there does (xitca-web
//! and ntex have none), and ohkami, from its top ten, answers only those.
//!
//!   bench-rust actix|axum|may|xitca|ntex|hyper|ohkami    PORT sets the port, THREADS the worker count

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
    (
        2,
        "A computer scientist is someone who fixes things that aren't broken.",
    ),
    (3, "After enough decimal places, nobody gives a damn."),
    (
        4,
        "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1",
    ),
    (
        5,
        "A computer program does what you tell it to do, not what you want it to do.",
    ),
    (
        6,
        "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen",
    ),
    (7, "Any program that runs right is obsolete."),
    (
        8,
        "A list is only as strong as its weakest link. — Donald Knuth",
    ),
    (9, "Feature: A bug with seniority."),
    (10, "Computers make very fast, very accurate mistakes."),
    (
        11,
        "<script>alert(\"This should not be displayed in a browser alert box.\");</script>",
    ),
    (12, "フレームワークのベンチマーク"),
];

fn fortunes() -> String {
    let mut fortunes = Vec::with_capacity(ROWS.len() + 1);
    fortunes.extend(ROWS.iter().map(|&(id, message)| Fortune { id, message }));
    fortunes.push(Fortune {
        id: 0,
        message: "Additional fortune added at request time.",
    });
    fortunes.sort_unstable_by(|a, b| a.message.cmp(b.message));
    Fortunes { fortunes }.render().expect("render")
}

/// `/page`: a layout, a table of 50 rows built per request with a name to
/// escape and a class chosen by a boolean, and a form. Templates in
/// `templates/`, the layout inherited as Askama's docs do.
struct Person {
    id: u32,
    name: &'static str,
    score: u32,
    active: bool,
}

#[derive(Template)]
#[template(path = "roster.html")]
struct Roster {
    people: Vec<Person>,
}

const NAMES: [&str; 5] = [
    "Ada <&\"",
    "Alan <&\"",
    "Grace <&\"",
    "Linus <&\"",
    "Edsger <&\"",
];

fn page() -> String {
    let people = (1..=50)
        .map(|id| Person {
            id,
            name: NAMES[id as usize % 5],
            score: id * 37 % 101,
            active: id % 3 != 0,
        })
        .collect();
    Roster { people }.render().expect("render")
}

/// TechEmpower's "json": serialized per request, with serde.
#[derive(serde::Serialize)]
struct Message {
    message: &'static str,
}

const MESSAGE: Message = Message {
    message: "Hello, World!",
};

/// The practice routes (bench/README.md), Actix and Axum only: the body of
/// `/echo`, a row of `/list`, and `/wait`'s answer.
#[derive(serde::Deserialize, serde::Serialize)]
struct Echo {
    name: String,
    email: String,
    age: i64,
    tags: Vec<String>,
}

impl Echo {
    /// The failing fields, in the order name, email, age, tags.
    fn problems(&self) -> Vec<&'static str> {
        let mut bad = Vec::new();
        if !(1..=50).contains(&self.name.chars().count()) {
            bad.push("name");
        }
        if !self.email.contains('@') {
            bad.push("email");
        }
        if !(0..=150).contains(&self.age) {
            bad.push("age");
        }
        if self.tags.len() > 10 {
            bad.push("tags");
        }
        bad
    }
}

#[derive(serde::Serialize)]
struct Errors {
    errors: Vec<&'static str>,
}

#[derive(serde::Serialize)]
struct User {
    id: u32,
    name: String,
    email: String,
    active: bool,
}

fn users() -> Vec<User> {
    (0..1000)
        .map(|i| User {
            id: i,
            name: format!("user {i}"),
            email: format!("user{i}@example.com"),
            active: i % 3 != 0,
        })
        .collect()
}

#[derive(serde::Serialize)]
struct Done {
    ok: bool,
}

const WAIT: std::time::Duration = std::time::Duration::from_millis(20);
const BODY_LIMIT: usize = 8 << 20;

fn main() -> std::io::Result<()> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let port: u16 = env("PORT").parse().unwrap_or(3000);
    let threads: usize = env("THREADS")
        .parse()
        .unwrap_or_else(|_| std::thread::available_parallelism().map_or(1, |n| n.get()));
    match std::env::args().nth(1).as_deref() {
        Some("actix") => actix(port, threads),
        Some("axum") => axum(port, threads),
        Some("may") => may(port, threads),
        Some("xitca") => xitca(port, threads),
        Some("ntex") => ntex(port, threads),
        Some("hyper") => hyper(port, threads),
        Some("ohkami") => ohkami(port, threads),
        _ => {
            eprintln!("usage: bench-rust actix|axum|may|xitca|ntex|hyper|ohkami");
            std::process::exit(2);
        }
    }
}

/// What `#[actix_web::main]` expands to, with the worker count set: one
/// single-threaded runtime per worker.
fn actix(port: u16, threads: usize) -> std::io::Result<()> {
    use actix_web::{App, HttpRequest, HttpResponse, HttpServer, error::JsonPayloadError, web};
    use actix_ws::Message;
    let server = HttpServer::new(|| {
        let json = web::JsonConfig::default().error_handler(|_: JsonPayloadError, _| {
            actix_web::error::InternalError::from_response(
                "",
                HttpResponse::UnprocessableEntity().json(Errors {
                    errors: vec!["body"],
                }),
            )
            .into()
        });
        App::new()
            .app_data(json)
            .app_data(web::PayloadConfig::new(BODY_LIMIT))
            .route(
                "/wait",
                web::get().to(|| async {
                    actix_web::rt::time::sleep(WAIT).await;
                    web::Json(Done { ok: true })
                }),
            )
            .route(
                "/echo",
                web::post().to(|echo: web::Json<Echo>| async move {
                    match echo.problems() {
                        errors if errors.is_empty() => HttpResponse::Ok().json(echo.into_inner()),
                        errors => HttpResponse::UnprocessableEntity().json(Errors { errors }),
                    }
                }),
            )
            .route(
                "/upload",
                web::post().to(|body: web::Bytes| async move { body.len().to_string() }),
            )
            .route("/list", web::get().to(|| async { web::Json(users()) }))
            .service(actix_files::Files::new("/static", "../static"))
            .route(
                "/ws",
                web::get().to(|req: HttpRequest, body: web::Payload| async move {
                    let (res, mut session, mut stream) = actix_ws::handle(&req, body)?;
                    actix_web::rt::spawn(async move {
                        while let Some(Ok(msg)) = stream.recv().await {
                            let sent = match msg {
                                Message::Text(text) => session.text(text).await,
                                Message::Binary(bytes) => session.binary(bytes).await,
                                Message::Ping(bytes) => session.pong(&bytes).await,
                                Message::Close(reason) => return drop(session.close(reason).await),
                                _ => Ok(()),
                            };
                            if sent.is_err() {
                                return;
                            }
                        }
                    });
                    Ok::<_, actix_web::Error>(res)
                }),
            )
            .route("/plaintext", web::get().to(|| async { "Hello, World!" }))
            .route(
                "/fortunes",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/html; charset=utf-8")
                        .body(fortunes())
                }),
            )
            .route("/json", web::get().to(|| async { web::Json(MESSAGE) }))
            .route(
                "/page",
                web::get().to(|| async {
                    HttpResponse::Ok()
                        .content_type("text/html; charset=utf-8")
                        .body(page())
                }),
            )
            .route("/", web::get().to(HttpResponse::Ok))
            .route("/user", web::post().to(HttpResponse::Ok))
            .route(
                "/user/{id}",
                web::get().to(|id: web::Path<String>| async move { id.into_inner() }),
            )
    });
    actix_web::rt::System::new().block_on(server.workers(threads).bind(("127.0.0.1", port))?.run())
}

/// What `#[tokio::main]` expands to, with the worker count set: one
/// work-stealing runtime shared by all threads.
fn axum(port: u16, threads: usize) -> std::io::Result<()> {
    use axum::{
        Json, Router,
        extract::{DefaultBodyLimit, Path, WebSocketUpgrade, rejection::JsonRejection},
        http::StatusCode,
        response::{Html, IntoResponse},
        routing::{get, post},
    };
    use tower_http::services::ServeDir;
    let app = Router::new()
        .route(
            "/wait",
            get(|| async {
                tokio::time::sleep(WAIT).await;
                Json(Done { ok: true })
            }),
        )
        .route(
            "/echo",
            post(|echo: Result<Json<Echo>, JsonRejection>| async move {
                match echo {
                    Ok(Json(echo)) => match echo.problems() {
                        errors if errors.is_empty() => Json(echo).into_response(),
                        errors => (StatusCode::UNPROCESSABLE_ENTITY, Json(Errors { errors }))
                            .into_response(),
                    },
                    Err(_) => (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(Errors {
                            errors: vec!["body"],
                        }),
                    )
                        .into_response(),
                }
            }),
        )
        .route(
            "/upload",
            post(|body: axum::body::Bytes| async move { body.len().to_string() })
                .layer(DefaultBodyLimit::max(BODY_LIMIT)),
        )
        .route("/list", get(|| async { Json(users()) }))
        .nest_service("/static", ServeDir::new("../static"))
        .route(
            "/ws",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(msg)) = socket.recv().await {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
        .route("/plaintext", get(|| async { "Hello, World!" }))
        .route("/fortunes", get(|| async { Html(fortunes()) }))
        .route("/json", get(|| async { axum::Json(MESSAGE) }))
        .route("/page", get(|| async { Html(page()) }))
        .route("/", get(|| async {}))
        .route("/user", post(|| async {}))
        .route(
            "/user/{id}",
            get(|Path(id): Path<String>| async move { id }),
        );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()?;
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
                "/plaintext" => res
                    .header("Content-Type: text/plain; charset=utf-8")
                    .body("Hello, World!"),
                "/fortunes" => res
                    .header("Content-Type: text/html; charset=utf-8")
                    .body_vec(fortunes().into_bytes()),
                "/json" => res
                    .header("Content-Type: application/json")
                    .body_vec(serde_json::to_vec(&MESSAGE)?),
                "/page" => res
                    .header("Content-Type: text/html; charset=utf-8")
                    .body_vec(page().into_bytes()),
                "/" => {
                    res.header("Content-Type: text/plain");
                }
                path if path.starts_with("/user") => {
                    if req.method() == "GET" {
                        let id = path.split('/').next_back().unwrap_or_default();
                        res.header("Content-Type: text/plain");
                        res.body_mut().extend_from_slice(id.as_bytes());
                    }
                }
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
    Factory
        .start(("127.0.0.1", port))?
        .join()
        .map_err(|_| std::io::Error::other("may-minihttp stopped"))
}

/// xitca-web: a thread-per-core server, `THREADS` workers.
fn xitca(port: u16, threads: usize) -> std::io::Result<()> {
    use xitca_web::{
        App, handler::handler_service, handler::html::Html, handler::json::Json,
        handler::params::Params, route::get, route::post,
    };
    App::new()
        .at("/plaintext", get(handler_service(async || "Hello, World!")))
        .at("/fortunes", get(handler_service(async || Html(fortunes()))))
        .at("/json", get(handler_service(async || Json(MESSAGE))))
        .at("/page", get(handler_service(async || Html(page()))))
        .at("/", get(handler_service(async || "")))
        .at("/user", post(handler_service(async || "")))
        .at(
            "/user/{id}",
            get(handler_service(async |Params(id): Params<String>| id)),
        )
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
            .route(
                "/fortunes",
                web::get().to(async || {
                    HttpResponse::Ok()
                        .content_type("text/html; charset=utf-8")
                        .body(fortunes())
                }),
            )
            .route(
                "/json",
                web::get().to(async || HttpResponse::Ok().json(&MESSAGE)),
            )
            .route(
                "/page",
                web::get().to(async || {
                    HttpResponse::Ok()
                        .content_type("text/html; charset=utf-8")
                        .body(page())
                }),
            )
            .route("/", web::get().to(async || HttpResponse::Ok()))
            .route("/user", web::post().to(async || HttpResponse::Ok()))
            .route(
                "/user/{id}",
                web::get().to(async |id: web::types::Path<String>| id.into_inner()),
            )
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

    async fn handle(
        req: Request<Incoming>,
    ) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let (kind, body): (&str, Bytes) = match req.uri().path() {
            "/plaintext" => (
                "text/plain; charset=utf-8",
                Bytes::from_static(b"Hello, World!"),
            ),
            "/fortunes" => ("text/html; charset=utf-8", fortunes().into()),
            "/json" => (
                "application/json",
                serde_json::to_vec(&MESSAGE).expect("json").into(),
            ),
            "/page" => ("text/html; charset=utf-8", page().into()),
            "/" | "/user" => return Ok(Response::new(Full::default())),
            path if path.starts_with("/user/") => {
                return Ok(Response::new(Full::new(Bytes::copy_from_slice(
                    path[6..].as_bytes(),
                ))));
            }
            _ => {
                let mut res = Response::new(Full::default());
                *res.status_mut() = StatusCode::NOT_FOUND;
                return Ok(res);
            }
        };
        let mut res = Response::new(Full::new(body));
        res.headers_mut()
            .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(kind));
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
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async move {
                #[cfg(unix)]
                let listener = listener.listen(1024)?;
                #[cfg(not(unix))]
                let listener = {
                    listener.set_nonblocking(true)?;
                    tokio::net::TcpListener::from_std(listener)?
                };
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(http1::Builder::new().serve_connection(
                        hyper_util::rt::TokioIo::new(stream),
                        service_fn(handle),
                    ));
                }
            })
        }));
    }
    for w in workers {
        w.join()
            .map_err(|_| std::io::Error::other("hyper worker panicked"))??;
    }
    Ok(())
}

/// ohkami on nio, a thread-per-core runtime with `THREADS` workers: its
/// the-benchmarker entry (rust/ohkami-nio), which answers only their routes.
fn ohkami(port: u16, threads: usize) -> std::io::Result<()> {
    use ohkami::prelude::*;
    let workers = u8::try_from(threads).unwrap_or(u8::MAX);
    nio::RuntimeBuilder::new()
        .worker_threads(workers)
        .build()?
        .block_on(async move {
            Ohkami::new((
                "/".GET(async || Response::OK()),
                "/user".POST(async || Response::OK()),
                "/user/:id".GET(async |Path(id): Path<String>| id),
            ))
            .howl(("127.0.0.1", port))
            .await
        });
    Ok(())
}
