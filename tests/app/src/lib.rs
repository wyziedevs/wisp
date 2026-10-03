//! The app as a library, so tests can answer requests in process
//! (`wisp::test::client::<wisp_test_app::Site>()`). `main.rs` serves it.

wisp::app!();

pub type Site = App;

/// A component drawn to a string, outside any request: `Card::html(props)`.
#[cfg(test)]
mod html_tests {
    use super::*;

    #[test]
    fn a_component_renders_to_a_string() {
        let html = Card::html("Hi <you>", 3, true);
        assert!(html.contains("<h2>Hi &lt;you&gt; ★</h2>"), "{html}");
        assert!(html.contains("3 items"), "{html}");
        assert!(Badge::html("new").contains("new"));
    }
}
