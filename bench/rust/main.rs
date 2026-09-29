//! Actix Web and Axum serving the same /fortunes, /plaintext and /json as
//! bench/app, rendering with Askama (templates compiled to Rust, like Wisp).
//!
//!   bench-rust actix|axum      PORT sets the port, THREADS the worker count

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
        _ => {
            eprintln!("usage: bench-rust actix|axum");
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
