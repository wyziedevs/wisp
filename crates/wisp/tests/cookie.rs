//! `#[derive(Cookie)]` as an app uses it, from outside the crate.

use wisp::Cookie;

#[derive(Cookie, Debug, PartialEq)]
struct Prefs {
    /// Doc comments and visibility are skipped.
    pub(crate) name: String,
    size: u8,
    theme: Theme,
}

#[derive(Cookie, Debug, PartialEq)]
enum Theme {
    Light,
    #[allow(dead_code)]
    Dark = 2,
}

#[derive(Cookie, Debug, PartialEq)]
struct Pair(i32, String);

#[test]
fn round_trips() {
    let p = Prefs {
        name: "a|b; c%".into(),
        size: 7,
        theme: Theme::Light,
    };
    assert_eq!(p.to_string(), "a%7Cb%3B%20c%25|7|Light");
    assert_eq!(p.to_string().parse(), Ok(p));
    assert_eq!("-3|x".parse(), Ok(Pair(-3, "x".into())));
    assert_eq!("Dark".parse(), Ok(Theme::Dark));
}

#[derive(Cookie, Debug, PartialEq)]
struct Nothing;

#[test]
fn rejects_what_is_not_one() {
    for junk in ["", "a|7", "a|7|Light|extra", "a|300|Light", "a|7|Blue"] {
        assert!(junk.parse::<Prefs>().is_err(), "{junk}");
    }
}

#[test]
fn a_type_without_fields_is_the_empty_string() {
    assert_eq!(Nothing.to_string(), "");
    assert_eq!("".parse(), Ok(Nothing));
    assert!("x".parse::<Nothing>().is_err());
}
