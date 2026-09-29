struct Data {
    title: String,
    href: &'static str,
    note: Option<&'static str>,
    nothing: Option<String>,
    on: bool,
    n: u32,
}

fn load() -> Data {
    Data { title: "a<b".into(), href: "/x?a=1&b=2", note: Some("hi \"you\""), nothing: None, on: true, n: 7 }
}
