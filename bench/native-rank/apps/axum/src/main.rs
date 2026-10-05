use axum::{
    extract::{Path, Query},
    http::{header, HeaderMap},
    response::Html,
    routing::get,
    Json, Router,
};
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

fn sid(h: &HeaderMap) -> String {
    h.get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| c.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == "sid").map(|(_, v)| v.to_string()))
        .unwrap_or_else(|| "none".into())
}

async fn params(Path(id): Path<String>, Query(q): Query<HashMap<String, String>>, h: HeaderMap) -> String {
    format!("id={id} q={} sid={}", q.get("q").map(String::as_str).unwrap_or(""), sid(&h))
}

async fn list() -> Html<String> {
    let mut s = String::with_capacity(32 * 1024);
    s.push_str("<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>");
    for i in 1..=1000 {
        let _ = write!(s, "<li>Item &lt;{i}&gt; &amp; co</li>");
    }
    s.push_str("</ul></body></html>");
    Html(s)
}

async fn json_big() -> Json<Vec<Row>> {
    Json((1..=200u32).map(|i| Row { id: i, name: format!("user-{i}"), active: i % 3 != 0, score: i * 37 % 101, tags: vec!["a".into(), format!("t{}", i % 7)] }).collect())
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/json", get(|| async { Json(Msg { message: "Hello, World!" }) }))
        .route("/params/{id}", get(params))
        .route("/list", get(list))
        .route("/json-big", get(json_big));
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let l = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    axum::serve(l, app).await.unwrap();
}
