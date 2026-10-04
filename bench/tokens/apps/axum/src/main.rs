// @feature setup
use askama::Template;
use axum::{
    Form, Json, Router,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use validator::Validate;

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
    name_error: Option<String>,
    email_error: Option<String>,
}

#[derive(Deserialize, Validate)]
struct ContactForm {
    #[validate(length(min = 1, max = 50, message = "must have 1 to 50 characters"))]
    name: String,
    #[validate(email(message = "must be an email address"))]
    email: String,
}

async fn contact() -> Html<String> {
    Html(Contact::default().render().unwrap())
}

async fn send(Form(f): Form<ContactForm>) -> Response {
    if let Err(e) = f.validate() {
        let errors = e.field_errors();
        let error = |k: &str| errors.get(k).and_then(|v| v[0].message.as_ref()).map(|m| m.to_string());
        let (name_error, email_error) = (error("name"), error("email"));
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
