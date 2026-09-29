//! A module of the app's own: no `mod shop;` anywhere, and route files
//! reach it as `shop`.

/// What a thing costs, in cents.
pub fn price(name: &str) -> u32 {
    match name {
        "tea" => 300,
        _ => 0,
    }
}
