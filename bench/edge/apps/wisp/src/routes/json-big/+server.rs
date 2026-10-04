#[model]
struct Row {
    id: u32,
    name: String,
    active: bool,
    score: u32,
    tags: Vec<String>,
}
fn get() -> Vec<Row> {
    (1..=200u32)
        .map(|i| Row { id: i, name: format!("user-{i}"), active: i % 3 != 0, score: i * 37 % 101, tags: vec!["a".into(), format!("t{}", i % 7)] })
        .collect()
}
