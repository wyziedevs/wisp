//! What Wisp's runtime (`wisp`), its compiler (`wisp-build`) and the
//! browser must agree on, in one crate both depend on: where a value lands
//! in a page and how it is escaped there (`contexts`), the marks and
//! headers of a live page (`protocol`), and the browser runtime itself;
//! and what more than one of them needs: SHA-256 (`sha256`), base64
//! (`base64`), JSON (`json`), `.env` files (`dotenv`), plural rules (`plural`), text direction (`dir`), the seeded generator of the property tests (`rng`). `std` only, so the compiler
//! stays small to build.

pub mod base64;
pub mod contexts;
pub mod dir;
pub mod dotenv;
pub mod fonts;
pub mod gzip;
pub mod json;
pub mod manifest;
pub mod og;
pub mod pattern;
pub mod plural;
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
/// What a component built as a custom element (`{@element "x-card"}`)
/// runs (`/_app/c/el.js`), which its module (`/_app/c/el/x-card.js`)
/// imports: `wisp-build` writes it out.
pub const ELEMENT_JS: &str = include_str!("client/element.js");

/// A cron schedule as one path segment, for the address a host's trigger
/// requests (`/_wisp/cron/<slug>`): its fields joined by `_`, and `/` as `~`,
/// so `*/5 * * * *` is `*~5_*_*_*_*`. `wisp build` writes it into each
/// host's config and the app finds its tasks by it.
pub fn cron_slug(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    fields.join("_").replace('/', "~")
}

#[cfg(test)]
mod cron_tests {
    use super::cron_slug;

    #[test]
    fn a_schedule_is_one_path_segment() {
        assert_eq!(cron_slug("0 3 * * *"), "0_3_*_*_*");
        assert_eq!(cron_slug(" */5  * * * 1-5 "), "*~5_*_*_*_1-5");
    }
}
