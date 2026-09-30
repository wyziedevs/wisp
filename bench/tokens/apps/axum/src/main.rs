// @feature setup
use askama::Template;
use axum::{
    Form, Json, Router,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;

mod db;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(list))
        .route("/contact", get(contact).post(send))
        .route("/api/items", get(api))
        .route("/search", get(search));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// @feature list
#[derive(Template)]
#[template(path = "list.html")]
struct List {
    items: Vec<db::Item>,
}

async fn list() -> Html<String> {
    Html(List { items: db::items().await }.render().unwrap())
}

// @feature form
#[derive(Template, Default)]
#[template(path = "contact.html")]
struct Contact {
    name: String,
    email: String,
    name_error: Option<&'static str>,
    email_error: Option<&'static str>,
}

#[derive(Deserialize)]
struct ContactForm {
    name: String,
    email: String,
}

async fn contact() -> Html<String> {
    Html(Contact::default().render().unwrap())
}

async fn send(Form(f): Form<ContactForm>) -> Response {
    let name_error = (f.name.is_empty() || f.name.len() > 50).then_some("Name must be 1 to 50 characters");
    let email_error = (!f.email.contains('@')).then_some("Enter a valid email");
    if name_error.is_some() || email_error.is_some() {
        let page = Contact { name: f.name, email: f.email, name_error, email_error };
        return (StatusCode::UNPROCESSABLE_ENTITY, Html(page.render().unwrap())).into_response();
    }
    println!("{} <{}>", f.name, f.email);
    Redirect::to("/").into_response()
}

// @feature api
async fn api() -> Json<Vec<db::Item>> {
    Json(db::items().await)
}

// @feature search
#[derive(Template)]
#[template(path = "search.html")]
struct Search {
    items: Vec<db::Item>,
}

async fn search() -> Html<String> {
    Html(Search { items: db::items().await }.render().unwrap())
}
