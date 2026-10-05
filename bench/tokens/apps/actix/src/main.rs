// @feature setup
use actix_web::{App, HttpResponse, HttpServer, get, post, web};
use serde::Deserialize;
use tera::{Context, Tera};
use validator::{Validate, ValidationError};

mod db;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let tera = web::Data::new(Tera::new("templates/**/*").unwrap());
    HttpServer::new(move || {
        App::new()
            .app_data(tera.clone())
            .service(list)
            .service(contact)
            .service(send)
            .service(api)
            .service(search)
    })
    .bind(("0.0.0.0", 3000))?
    .run()
    .await
}

fn page(tera: &Tera, name: &str, cx: &Context) -> String {
    tera.render(name, cx).unwrap()
}

// @feature list
#[get("/")]
async fn list(tera: web::Data<Tera>) -> HttpResponse {
    let mut cx = Context::new();
    cx.insert("items", &db::items().await);
    HttpResponse::Ok().content_type("text/html").body(page(&tera, "list.html", &cx))
}

// @feature form
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

#[get("/contact")]
async fn contact(tera: web::Data<Tera>) -> HttpResponse {
    HttpResponse::Ok().content_type("text/html").body(page(&tera, "contact.html", &Context::new()))
}

#[post("/contact")]
async fn send(tera: web::Data<Tera>, web::Form(f): web::Form<ContactForm>) -> HttpResponse {
    if let Err(e) = f.validate() {
        let mut cx = Context::new();
        for (field, errors) in e.field_errors() {
            cx.insert(format!("{field}_error"), &errors[0].message);
        }
        cx.insert("name", &f.name);
        cx.insert("email", &f.email);
        let body = page(&tera, "contact.html", &cx);
        return HttpResponse::UnprocessableEntity().content_type("text/html").body(body);
    }
    println!("{} <{}>", f.name, f.email);
    HttpResponse::SeeOther().insert_header(("location", "/")).finish()
}

// @feature api
#[get("/api/items")]
async fn api() -> web::Json<Vec<db::Item>> {
    web::Json(db::items().await)
}

// @feature search
#[get("/search")]
async fn search(tera: web::Data<Tera>) -> HttpResponse {
    let mut cx = Context::new();
    cx.insert("items", &db::items().await);
    HttpResponse::Ok().content_type("text/html").body(page(&tera, "search.html", &cx))
}
