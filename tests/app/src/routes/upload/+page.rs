use wisp::prelude::*;

/// Photos are bigger than the 1 MB every other route takes.
pub const BODY_LIMIT: usize = 4 * wisp::MB;

pub struct Data {
    pub saved: Option<Saved>,
    pub problem: Option<&'static str>,
    pub title: String,
}

/// What the action received, handed to `load` for the page.
#[derive(Clone)]
pub struct Saved {
    pub name: String,
    pub size: usize,
    pub kind: String,
}

pub struct Problem(&'static str);

pub fn load(cx: &mut Cx) -> Data {
    Data {
        saved: cx.get::<Saved>().cloned(),
        problem: cx.get::<Problem>().map(|p| p.0),
        // A failed post keeps what was typed.
        title: cx.form().get("title").unwrap_or_default().into_owned(),
    }
}

#[action]
pub fn default(cx: &mut Cx) {
    let saved = cx.form().file("photo").map(|f| Saved { name: f.name.into_owned(), size: f.bytes.len(), kind: f.content_type.to_string() });
    match saved {
        Some(saved) => cx.set(saved),
        None => {
            cx.set_status(422);
            cx.set(Problem("Choose a photo"));
        }
    }
}

/// Answers with a file instead of the page.
#[action]
pub fn export(cx: &mut Cx) -> Response {
    let title = cx.form().get("title").unwrap_or_default().into_owned();
    Response::new("text/csv", format!("title\n{title}\n")).with_header("content-disposition", "attachment; filename=\"export.csv\"")
}
