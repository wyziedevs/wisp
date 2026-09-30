// @feature setup
use actix_web::{App, HttpResponse, HttpServer, get, post, web};
use serde::Deserialize;
use tera::{Context, Tera};

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
#[derive(Deserialize)]
struct ContactForm {
    name: String,
    email: String,
}

#[get("/contact")]
async fn contact(tera: web::Data<Tera>) -> HttpResponse {
    HttpResponse::Ok().content_type("text/html").body(page(&tera, "contact.html", &Context::new()))
}

#[post("/contact")]
async fn send(tera: web::Data<Tera>, web::Form(f): web::Form<ContactForm>) -> HttpResponse {
    let mut cx = Context::new();
    if f.name.is_empty() || f.name.len() > 50 {
        cx.insert("name_error", "Name must be 1 to 50 characters");
    }
    if !f.email.contains('@') {
        cx.insert("email_error", "Enter a valid email");
    }
    if cx.contains_key("name_error") || cx.contains_key("email_error") {
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
