//! `Card::html(props)`: a component drawn to a `String`, for mail bodies
//! and anything else outside a request. Opt-in: an unused one costs a few
//! lines of dead code, and no page changes.

use crate::codegen::Comp;
use crate::template::PropDecl;

/// For the component's own module, after its `render`, where its props'
/// types resolve as they do there.
pub(crate) fn html_fn(props: &[PropDecl]) -> String {
    let args: String = props
        .iter()
        .map(|d| format!("{}: {}, ", d.name, d.ty))
        .collect();
    let pass: String = props.iter().map(|d| format!("{}, ", d.name)).collect();
    format!(
        "        #[doc(hidden)]\n        pub struct __Html;\n        #[allow(dead_code, clippy::type_complexity)]\n        impl __Html {{\n            pub fn html({args}) -> String {{\n                let mut o = ::wisp::Out::default();\n                render(&mut o, {pass}&|_| {{}});\n                o.body\n            }}\n        }}\n"
    )
}

/// Names the app's Rust sees without importing, Rust's and `wisp::prelude`'s:
/// a component of one of these names (`<Box>`, `<Table>`) must not make them
/// ambiguous where both globs are imported, so it has no `html`.
const PRELUDE: &str = "Box Option Result Vec String Some None Ok Err Default Clone Copy
    Send Sync Sized Drop Fn FnMut FnOnce From Into Always Ignore Never Config Cookie CookieOptions
    Cx Email Error FromJson Image Json KB MB Method OrStatus Response Rest Row SameSite Shared Table
    Upload Value RateLimit";

/// `pub mod __comps`: each component by name, `Card::html(..)`. A name the
/// app defines itself wins over the glob import `wisp::app!` makes of these.
pub(crate) fn components(comps: &[Comp]) -> String {
    let mut s = String::from("#[doc(hidden)]\n#[allow(unused_imports)]\npub mod __comps {\n");
    for c in comps
        .iter()
        .filter(|c| !PRELUDE.split_whitespace().any(|n| n == c.name))
    {
        s += &format!("    pub use super::{}::__Html as {};\n", c.module, c.name);
    }
    s + "}\n\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_struct_per_component() {
        let p = vec![PropDecl {
            name: "title".into(),
            ty: "&str".into(),
            default: None,
        }];
        let s = html_fn(&p);
        assert!(s.contains("pub fn html(title: &str, ) -> String"), "{s}");
        assert!(s.contains("render(&mut o, title, &|_| {});"), "{s}");
    }
}
