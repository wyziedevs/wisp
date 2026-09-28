//! `#[action]` marks a `+page.rs` function as a form action.
//!
//! The attribute returns its input unchanged. `wisp-build` finds it in the
//! source and generates the route for it; nothing else in a page module is
//! reachable over HTTP. No `syn`, no `quote`: this crate compiles instantly.

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn action(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return "compile_error!(\"#[action] takes no arguments\");".parse().unwrap();
    }
    item
}
