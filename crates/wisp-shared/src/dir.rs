//! Which languages are written right to left, for `<html dir>`: the build
//! sets it in the shell's tag per locale, `wisp::dir` says it to a page.

/// Whether `locale` (`ar`, `he-IL`, `fa_IR`) is a right-to-left language.
pub fn is_rtl(locale: &str) -> bool {
    let lang = locale.split(['-', '_']).next().unwrap_or("");
    [
        "ar", "he", "fa", "ur", "ps", "sd", "ug", "yi", "dv", "ckb", "ku",
    ]
    .iter()
    .any(|l| lang.eq_ignore_ascii_case(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_to_left() {
        assert!(is_rtl("ar") && is_rtl("he-IL") && is_rtl("FA_ir"));
        assert!(!is_rtl("en") && !is_rtl("fr-CA") && !is_rtl("") && !is_rtl("arm"));
    }
}
