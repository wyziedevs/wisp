struct Data {
    slug: String,
}

fn load(slug: String) -> Data {
    Data { slug }
}

/// The pages `wisp build --static` writes for this route.
fn entries() -> Vec<&'static str> {
    vec!["hello", "second-post"]
}
