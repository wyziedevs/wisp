use actix_web::{get, web, App, HttpRequest, HttpResponse, HttpServer};
use serde::Serialize;
use std::{collections::HashMap, fmt::Write};

#[derive(Serialize)]
struct Msg {
    message: &'static str,
}

#[derive(Serialize)]
struct Row {
    id: u32,
    name: String,
    active: bool,
    score: u32,
    tags: Vec<String>,
}

#[get("/")]
async fn index() -> HttpResponse {
    HttpResponse::Ok().content_type("text/plain; charset=utf-8").body("Hello, World!")
}

#[get("/json")]
async fn json() -> web::Json<Msg> {
    web::Json(Msg { message: "Hello, World!" })
}

#[get("/params/{id}")]
async fn params(id: web::Path<String>, q: web::Query<HashMap<String, String>>, req: HttpRequest) -> HttpResponse {
    let sid = req.cookie("sid").map(|c| c.value().to_string()).unwrap_or_else(|| "none".into());
    HttpResponse::Ok().content_type("text/plain; charset=utf-8").body(format!("id={id} q={} sid={sid}", q.get("q").map(String::as_str).unwrap_or("")))
}

#[get("/list")]
async fn list() -> HttpResponse {
    let mut s = String::with_capacity(32 * 1024);
    s.push_str("<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>");
    for i in 1..=1000 {
        let _ = write!(s, "<li>Item &lt;{i}&gt; &amp; co</li>");
    }
    s.push_str("</ul></body></html>");
    HttpResponse::Ok().content_type("text/html; charset=utf-8").body(s)
}

#[get("/json-big")]
async fn json_big() -> web::Json<Vec<Row>> {
    web::Json((1..=200u32).map(|i| Row { id: i, name: format!("user-{i}"), active: i % 3 != 0, score: i * 37 % 101, tags: vec!["a".into(), format!("t{}", i % 7)] }).collect())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    HttpServer::new(|| App::new().service(index).service(json).service(params).service(list).service(json_big))
        .bind(("0.0.0.0", port))?
        .run()
        .await
}
