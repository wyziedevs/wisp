/// Photos are bigger than the 1 MB every other route takes.
const BODY_LIMIT: usize = 4 * wisp::MB;

struct Data {
    saved: Option<Saved>,
    problem: Option<&'static str>,
    /// What was typed, which a failed post keeps.
    title: Option<String>,
}

/// What the action received, handed to `load` for the page.
struct Saved {
    name: String,
    size: usize,
    kind: String,
}

struct Problem(&'static str);

fn load(cx: &mut Cx, title: Option<String>) -> Data {
    Data { saved: cx.take(), problem: cx.take().map(|Problem(p)| p), title }
}

#[action]
fn default(cx: &mut Cx) {
    let Some(photo) = cx.form().file("photo") else {
        return cx.fail(422, Problem("Choose a photo"));
    };
    let saved = Saved { name: photo.name.into_owned(), size: photo.bytes.len(), kind: photo.content_type.to_string() };
    cx.set(saved);
}

/// Answers with a file instead of the page.
#[action]
fn export(title: String) -> Response {
    Response::download("export.csv", format!("title\n{title}\n"))
}
