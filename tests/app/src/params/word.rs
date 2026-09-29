/// `[name=word]`: lowercase letters only. Wisp decodes the segment first.
fn matches(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())
}
