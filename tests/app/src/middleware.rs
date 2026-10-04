//! Named route middleware: `const MIDDLEWARE: &[&str] = &["gate"];` in a
//! page, an endpoint or a layout runs these first.

pub fn gate(cx: &mut Cx) -> Result {
    match cx.query("key").as_deref() {
        Some("open") => Ok(()),
        _ => error(403, "Closed"),
    }
}

pub fn stamp(cx: &mut Cx) -> Result {
    cx.set_header("x-stamped", "yes");
    Ok(())
}
