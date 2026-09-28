use wisp::prelude::*;

pub struct Link {
    pub href: &'static str,
    pub label: &'static str,
    pub current: bool,
}

pub struct Data {
    pub nav: [Link; 3],
}

const NAV: [(&str, &str); 3] = [("/", "Home"), ("/about", "About"), ("/wisple", "Wisple")];

/// The header's links, with the one this page lives under marked current.
pub async fn load(cx: &mut Cx) -> Result<Data> {
    let path = cx.path();
    let nav = NAV.map(|(href, label)| Link {
        href,
        label,
        current: match href {
            "/" => path == "/",
            _ => path.strip_prefix(href).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
        },
    });
    Ok(Data { nav })
}
