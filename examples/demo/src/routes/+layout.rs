const NAV: [(&str, &str); 3] = [("/", "Home"), ("/about", "About"), ("/wisple", "Wisple")];

struct Link {
    href: &'static str,
    label: &'static str,
    current: bool,
}

struct Data {
    nav: [Link; 3],
}

/// The header's links, with the one for this part of the site marked
/// current: `/wisple/how-to-play` is part of Wisple.
fn load(cx: &mut Cx) -> Data {
    let section = cx.path().split('/').nth(1).unwrap_or("");
    Data { nav: NAV.map(|(href, label)| Link { href, label, current: &href[1..] == section }) }
}
