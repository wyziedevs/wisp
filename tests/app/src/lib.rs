//! The app as a library, so tests can answer requests in process
//! (`wisp::test::client::<wisp_test_app::Site>()`). `main.rs` serves it.

wisp::app!();

pub type Site = App;
