use wisp::prelude::*;

const NAV: [(&str, &str); 3] = [("/", "Home"), ("/about", "About"), ("/wisple", "Wisple")];

pub struct Link {
    pub href: &'static str,
    pub label: &'static str,
    pub current: bool,
}

pub struct Data {
    pub nav: [Link; 3],
}

/// The header's links, with the one for this part of the site marked
/// current: `/wisple/how-to-play` is part of Wisple.
pub fn load(cx: &mut Cx) -> Data {
    let section = cx.path().split('/').nth(1).unwrap_or("");
    Data { nav: NAV.map(|(href, label)| Link { href, label, current: &href[1..] == section }) }
}
