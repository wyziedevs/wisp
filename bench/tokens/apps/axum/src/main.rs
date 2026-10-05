// @feature setup
use askama::Template;
use axum::{
    Form, Json, Router,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::Deserialize;
use validator::{Validate, ValidationError};

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
    #[validate(custom(function = "name"))]
    name: String,
    #[validate(custom(function = "email"))]
    email: String,
}

fn problem(message: &'static str) -> ValidationError {
    ValidationError::new("invalid").with_message(message.into())
}

fn name(s: &str) -> Result<(), ValidationError> {
    match s.chars().count() {
        0 => Err(problem("must have at least 1 character")),
        51.. => Err(problem("must have at most 50 characters")),
        _ => Ok(()),
    }
}

fn email(s: &str) -> Result<(), ValidationError> {
    let user = |c: char| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c);
    let label = |l: &str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    match s.split_once('@') {
        Some((u, d)) if !u.is_empty() && u.chars().all(user) && d.split('.').all(label) => Ok(()),
        _ => Err(problem("must be an email address")),
    }
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
