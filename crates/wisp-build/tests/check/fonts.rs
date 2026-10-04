//! `src/fonts.txt`: its CSS goes first in the app's styles, its preload
//! links in the shell's head, and a bad line is refused.

use crate::common::{Project, passes};
use std::fs;

const MAIN: (&str, &str) = ("src/main.rs", "wisp::main!();");
const PAGE: (&str, &str) = ("src/routes/+page.wisp", "<h1>hi</h1>");

#[test]
fn fonts_become_css_and_preloads() {
    let p = Project::new(&[
        MAIN,
        PAGE,
        (
            "src/fonts.txt",
            "Inter inter.woff2 100-900\nLora lora.woff2 400 serif\n",
        ),
    ]);
    wisp_build::write_styles(p.root()).unwrap();
    let css = fs::read_to_string(p.root().join(".wisp/scoped.css")).unwrap();
    assert!(css.starts_with("@font-face{font-family:\"Inter\""), "{css}");
    assert!(css.contains("font-weight:100 900;font-style:normal;font-display:swap"));
    assert!(css.contains(":root{--font-lora:\"Lora\",serif}"));
    let hot = wisp_build::hot(p.root()).unwrap();
    let shell = hot.templates[0].chunks.concat();
    assert!(shell.contains("<link rel=\"preload\" href=\"/fonts/inter.woff2\" as=\"font\" type=\"font/woff2\" crossorigin>"), "{shell}");
}

#[test]
fn no_fonts_no_css() {
    passes(&[MAIN, PAGE]);
    let p = Project::new(&[MAIN, PAGE]);
    assert_eq!(wisp_build::write_styles(p.root()).unwrap(), (false, false));
}

#[test]
fn a_bad_line_is_refused() {
    let p = Project::new(&[MAIN, PAGE, ("src/fonts.txt", "Inter inter.woff2 bold\n")]);
    let e = wisp_build::check(p.root()).unwrap_err();
    assert!(
        e.contains("src/fonts.txt:1") && e.contains("not a weight"),
        "{e}"
    );
}
