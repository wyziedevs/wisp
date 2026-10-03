//! Server functions: browser code calls each as `await add(2, 3)`
//! (`tests/remote.rs`).

#[derive(Json, FromJson)]
pub struct Pair {
    pub a: i64,
    pub b: i64,
}

#[remote]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[remote]
fn swap(pair: Pair) -> Pair {
    Pair {
        a: pair.b,
        b: pair.a,
    }
}

#[remote(get)]
fn greet(name: String, title: Option<String>) -> String {
    match title {
        Some(t) => format!("Hello, {t} {name}"),
        None => format!("Hello, {name}"),
    }
}

/// Reads what `before` in hooks.rs found: the hooks ran first.
#[remote]
fn whoami() -> Option<String> {
    cx.get::<crate::hooks::User>().map(|u| u.0.clone())
}

#[remote]
fn shout(#[validate(len = 1..=5)] word: &str) -> String {
    word.to_uppercase()
}

#[remote]
fn missing(id: u64) -> Result<String> {
    error(404, format!("No thing {id}"))
}

#[remote]
fn leave() {
    redirect("/")
}
