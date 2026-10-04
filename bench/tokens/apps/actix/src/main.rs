// @feature setup
use actix_web::{App, HttpResponse, HttpServer, get, post, web};
use serde::Deserialize;
use tera::{Context, Tera};
use validator::Validate;

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
    #[validate(length(min = 1, max = 50, message = "must have 1 to 50 characters"))]
    name: String,
    #[validate(email(message = "must be an email address"))]
    email: String,
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
