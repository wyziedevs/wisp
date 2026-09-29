pub struct Data {
    pub title: String,
    pub href: &'static str,
    pub note: Option<&'static str>,
    pub nothing: Option<String>,
    pub on: bool,
    pub n: u32,
}

pub fn load() -> Data {
    Data { title: "a<b".into(), href: "/x?a=1&b=2", note: Some("hi \"you\""), nothing: None, on: true, n: 7 }
}
