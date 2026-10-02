//! What Wisp's runtime (`wisp`), its compiler (`wisp-build`) and the
//! browser must agree on, in one crate both depend on: where a value lands
//! in a page and how it is escaped there (`contexts`), the marks and
//! headers of a live page (`protocol`), and the browser runtime itself;
//! and what more than one of them needs: SHA-256 (`sha256`), the seeded
//! generator of the property tests (`rng`). `std` only, so the compiler
//! stays small to build.

pub mod contexts;
pub mod protocol;
pub mod rng;
pub mod rules;
pub mod rust;
pub mod sha256;

/// The browser runtime as written: `wisp.js`, which every page links (form
/// actions, links that morph the page in place), and `live.js`, linked by a
/// page with browser code. `wisp`'s build minifies them for release builds,
/// and their hash is their `?v=` (`wisp_build::runtime_version`).
pub const WISP_JS: &str = include_str!("client/wisp.js");
pub const LIVE_JS: &str = include_str!("client/live.js");
/// The less used half of live.js (`/_app/c/extra.js`), which a generated
/// module imports when it uses it: `wisp-build` writes it out.
pub const EXTRA_JS: &str = include_str!("client/extra.js");
