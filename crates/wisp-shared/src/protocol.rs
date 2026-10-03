//! The live-page wire protocol, in one place: the marks the server writes
//! into a page for the browser runtime, the JSON that lists a page's
//! instances, the bits of an `on:` directive's modifiers and the headers
//! `wisp.js` sends and reads.
//!
//! The runtime writes the instance list by it (`wisp`'s `live.rs`) and
//! `wisp-build` writes the marks by it. The browser half (`client/live.js`,
//! `client/wisp.js`) spells the same names as literals, so the minifier
//! keeps them as short as they are; the tests below hold each literal to
//! its constant, and `tests/app/tests/golden.rs` runs both scripts under
//! Node against a page the test app renders.
//!
//! A page with browser code, as written:
//!
//! ```text
//! <button data-w="0.0">2</button>            instance 0, binding group 0
//! <li data-w="0.1" data-wl="{&quot;item&quot;:&quot;a&quot;}">a</li>
//!                                             a Rust loop's values its
//!                                             directives read, as JSON
//! <template data-w="0.2">…</template><!--[-->…<!--]-->
//!                                             a client block, then each
//!                                             copy the server painted
//! <script type="application/json" id="wisp-live">
//!   {"m":{"t3":"/_app/c/t3.js?v=…"},"i":[[0,"t3",-1,{"start":2}],[1,"t5",0,{},"v"]],"r":"/p/[id]","p":{"id":"1"}}
//! </script>
//! ```
//!
//! `m` maps module ids to their URLs. `i` lists the instances in the order
//! they rendered, each a record `[I, module, parent, blob, how?]`: its
//! number, its module's id, the instance it renders inside (-1 for none),
//! its server values and, for an island, how it starts (`ISLAND_*`). The
//! optional part last, both scripts read a record by place, with nothing
//! to work out. `r` and `p` are the route id and parameters of a page with
//! a `+page.js`. `t` holds the messages the page's scripts show, by key,
//! in the request's locale.

/// Opens the instance list, up to the first module of `m`: the scripts
/// find it by its id.
pub const LIVE_OPEN: &str = "<script type=\"application/json\" id=\"wisp-live\">{\"m\":{";
/// Between the modules and the records.
pub const LIVE_RECORDS: &str = "},\"i\":[";
/// Before the route id of a page with a `+page.js`.
pub const LIVE_ROUTE: &str = ",\"r\":";
/// Between the route id and its parameters.
pub const LIVE_PARAMS: &str = ",\"p\":{";
/// Before the messages the page's scripts show (`t('key')`): an object
/// of them by key.
pub const LIVE_TEXTS: &str = ",\"t\":{";
/// Closes the instance list.
pub const LIVE_CLOSE: &str = "}</script>";

/// On a directive element: `I.G`, its instance and binding group (in a
/// client component's or block's own markup, `G` alone).
pub const GROUP_ATTR: &str = "data-w";
/// On a directive element in a Rust loop: the loop values it reads, as JSON.
pub const LOOP_ATTR: &str = "data-wl";
/// The `<template>` a client component's children go in.
pub const SLOT_ATTR: &str = "data-wslot";
/// Before and after each copy of a client block the server painted, which
/// the browser's first draw takes over.
pub const COPY_START: &str = "<!--[-->";
pub const COPY_END: &str = "<!--]-->";

/// An island's start (a record's `how`): when it is seen, when the browser
/// is idle, at the first pointer, focus or key on it, or never (its code
/// and values are not sent). `m(query)`, when the media query matches, is
/// the fifth.
pub const ISLAND_VISIBLE: &str = "v";
pub const ISLAND_IDLE: &str = "i";
pub const ISLAND_INTERACTION: &str = "x";
pub const ISLAND_MEDIA: &str = "m";
pub const ISLAND_NONE: &str = "n";

/// An `on:` directive's modifiers as live.js takes them: bit `1 << k` for
/// `ON_FLAGS[k]`.
pub const ON_FLAGS: [&str; 13] = [
    "prevent", "stop", "once", "self", "capture", "passive", "window", "document", "outside",
    "ctrl", "shift", "alt", "meta",
];
/// The modifiers that say where it listens: `capture` to `outside`.
pub const ON_PLACED: u32 = 0b1_1111_0000;
/// An event handled by one listener at the root.
pub const ON_ROOT: u32 = 8192;

/// Sent by wisp.js with every request it makes.
pub const HEADER_JS: &str = "x-wisp";
/// A redirect's target, for wisp.js to follow itself.
pub const HEADER_LOCATION: &str = "x-wisp-location";
/// Browser code failed to start: wisp.js asks for the route's error page.
pub const HEADER_ERROR: &str = "x-wisp-error";

/// `/_app/<file>`, where Wisp serves its own files, as a literal: for a
/// `concat!` that builds a tag once, at compile time.
#[macro_export]
macro_rules! app_path {
    ($file:literal) => {
        concat!("/_app/", $file)
    };
}

/// Where Wisp serves its own files.
pub const APP_PREFIX: &str = app_path!("");
/// The browser modules: templates', `src/lib`'s (`lib/`) and extra.js.
pub const MODULES: &str = app_path!("c/");
/// A release build's npm modules, from `.wisp/npm`: each path names its
/// package's version, so it never changes.
pub const NPM_MODULES: &str = app_path!("c/npm/");
/// Templates' images: a release build's by content hash (immutable), a dev
/// build's `src/lib` ones under `lib/`.
pub const IMAGES: &str = app_path!("img/");
/// The browser runtime: wisp.js, live.js and live.js's less used half.
pub const WISP_JS_PATH: &str = app_path!("wisp.js");
pub const LIVE_JS_PATH: &str = app_path!("live.js");
pub const EXTRA_JS_PATH: &str = app_path!("c/extra.js");
/// `#[remote]` functions: each is served at this and its hash, and
/// browser code calls them through the module at `REMOTE_JS_PATH`.
pub const REMOTE: &str = app_path!("r/");
pub const REMOTE_JS_PATH: &str = app_path!("c/remote.js");
/// Components built as custom elements: `el/x-card.js`, and what they run.
pub const ELEMENTS: &str = app_path!("c/el/");
pub const ELEMENT_JS_PATH: &str = app_path!("c/el.js");
/// An app's service worker and web app manifest, at the root so the
/// worker's scope is the whole site.
pub const SERVICE_WORKER_PATH: &str = "/service-worker.js";
pub const MANIFEST_PATH: &str = "/manifest.webmanifest";
/// The app's CSS (`src/app.css`, or what Tailwind built of it).
pub const APP_CSS_PATH: &str = app_path!("app.css");
/// A dev build's scoped `<style>`s, from the project root: served after
/// the app's CSS at `APP_CSS_PATH` (a release build embeds both).
pub const SCOPED_CSS: &str = ".wisp/scoped.css";

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name as the browser runtime spells it, at the places it reads it.
    #[test]
    fn the_browser_runtime_spells_the_same_names() {
        let (live, wisp) = (crate::LIVE_JS, crate::WISP_JS);
        let q = |s: &str| format!("'{s}'");
        let in_live = [
            format!("getAttribute({})", q(GROUP_ATTR)),
            format!("querySelectorAll('[{GROUP_ATTR}]')"),
            format!("getAttribute({})", q(LOOP_ATTR)),
            format!("hasAttribute({})", q(SLOT_ATTR)),
            format!("getElementById({})", q(LIVE_ID)),
            format!("n.data == {}", q(&COPY_START[4..5])),
            format!("n.data == {}", q(&COPY_END[4..5])),
            format!("b & {ON_ROOT}"),
            format!("{ON_ROOT} for an event the root handles"),
            format!("b & {ON_PLACED_JS}"),
            "i.map(([I, id, P, blob, how]) =>".into(),
            format!("{} = {{}}", &LIVE_TEXTS[2..3]),
        ];
        let in_wisp = [
            format!("getElementById({})", q(LIVE_ID)),
            format!("[{GROUP_ATTR}^="),
            format!("closest?.('[{GROUP_ATTR}]')"),
            format!("{{ {}: '1' }}", q(HEADER_JS)),
            format!("headers.get({})", q(HEADER_LOCATION)),
            format!("{{ {}: '1' }}", q(HEADER_ERROR)),
            format!("how == {}", q(ISLAND_INTERACTION)),
            format!("how == {}", q(ISLAND_VISIBLE)),
            format!("how[0] == '{ISLAND_MEDIA}'"),
            format!("startsWith({})", q(APP_PREFIX)),
            "for (const [I, , P, , how] of".into(),
        ];
        for want in &in_live {
            assert!(live.contains(want.as_str()), "live.js reads `{want}`");
        }
        for want in &in_wisp {
            assert!(wisp.contains(want.as_str()), "wisp.js reads `{want}`");
        }
        // The bits, as live.js's comment lists them and its code tests them.
        for (k, flag) in ON_FLAGS.iter().enumerate() {
            let bit = 1u32 << k;
            assert!(
                live.contains(&format!("{flag} {bit}")),
                "live.js: {flag} is {bit}"
            );
        }
        assert_eq!(ON_PLACED, (1 << 4 | 1 << 5 | 1 << 6 | 1 << 7 | 1 << 8));
        assert!(ON_ROOT > 1 << (ON_FLAGS.len() - 1));
        // The instance list's pieces.
        assert!(LIVE_OPEN.contains(&format!("id=\"{LIVE_ID}\"")));
        assert_eq!((&COPY_START[..4], &COPY_END[5..]), ("<!--", "-->"));
    }

    /// `document` and `outside`, which both listen on the document.
    const ON_PLACED_JS: u32 = 1 << 7 | 1 << 8;

    /// The id of the instance list, which the scripts find it by.
    const LIVE_ID: &str = "wisp-live";
}
