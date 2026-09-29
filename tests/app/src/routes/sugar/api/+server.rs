/// A value, not a `Response`: sent as JSON.
fn get() -> Vec<(&'static str, Option<i32>)> {
    vec![("a<b", Some(1)), ("c", None)]
}
