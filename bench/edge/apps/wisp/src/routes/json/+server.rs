#[model]
struct Msg {
    ok: bool,
    name: String,
    n: u32,
}
fn get() -> Msg {
    Msg { ok: true, name: "wisp".into(), n: 42 }
}
