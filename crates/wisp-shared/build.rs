//! The base path (`WISP_BASE`, such as `/app`), compiled into every URL Wisp
//! writes (see `protocol::BASE`): a build of the same sources for another
//! base is a rebuild, which cargo does when the variable changes.

fn main() {
    println!("cargo::rerun-if-env-changed=WISP_BASE");
    let base = std::env::var("WISP_BASE").unwrap_or_default();
    let base = base.trim().trim_end_matches('/');
    // `/app`: one slash first, no `?`, `#`, quote, space or control byte.
    let ok = base.is_empty()
        || (base.starts_with('/')
            && !base.starts_with("//")
            && !base.contains("//")
            && base.bytes().all(|b| b.is_ascii_graphic() && !b"?#\"'<>\\`%".contains(&b)));
    if !ok {
        println!("cargo::error=WISP_BASE is `{base}`: write it as `/app`, a path with a slash first, none last and no `?`, `#`, quote or space");
        return;
    }
    println!("cargo::rustc-env=WISP_BASE={base}");
}
