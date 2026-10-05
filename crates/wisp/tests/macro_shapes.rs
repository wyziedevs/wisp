//! Shapes the derives and `#[model]` must compile warning-free: `#[cfg]` and
//! doc attributes, visibility, nested options, nested models, raw names.
#![deny(warnings)]

use wisp::{FromJson, Json, from_json, to_json};

/// A doc on the type.
#[derive(Json, FromJson, Debug, PartialEq)]
pub(crate) struct Shapes {
    /// A doc on a field.
    pub(crate) twice: Option<Option<u8>>,
    pub list: Vec<Option<u8>>,
    #[cfg(not(test))]
    gone: u8,
    #[cfg(test)]
    kept: u8,
    inner: Inner,
    map: std::collections::BTreeMap<String, Vec<u8>>,
    r#ref: bool,
}

#[wisp::model]
#[derive(Debug, PartialEq)]
/// A nested model.
struct Inner {
    /// Its name.
    name: String,
    #[cfg(not(test))]
    hidden: u8,
}

#[derive(Json, FromJson, Debug, PartialEq)]
enum Kind {
    /// Doc on a variant.
    A = 1,
    #[cfg(not(test))]
    Gone,
    r#B,
}

#[derive(Json, FromJson, Debug, PartialEq)]
struct Pair(pub u8, #[cfg(test)] String);

#[derive(Json, FromJson, Debug, PartialEq)]
struct Nothing;

#[test]
fn shapes_round_trip() {
    let s = Shapes {
        twice: Some(None),
        list: vec![Some(1), None],
        kept: 2,
        inner: Inner { name: "x".into() },
        map: Default::default(),
        r#ref: true,
    };
    let text = to_json(&s);
    assert!(!text.contains("gone"), "{text}");
    let back: Shapes = from_json(text.as_bytes()).unwrap();
    assert_eq!(back.list, s.list);
    assert_eq!(back.inner, s.inner);
    assert_eq!(to_json(&Kind::r#B), "\"B\"");
    assert_eq!(from_json::<Kind>(b"\"A\"").ok(), Some(Kind::A));
    let p = Pair(1, "a".into());
    assert_eq!(from_json::<Pair>(to_json(&p).as_bytes()).ok(), Some(p));
    assert_eq!(
        from_json::<Nothing>(to_json(&Nothing).as_bytes()).ok(),
        Some(Nothing)
    );
}

/// A doc, a lifetime and a `where` clause with an arrow in it.
#[wisp::action]
pub(crate) fn where_arrow<'a, T>(name: &'a str, f: T)
where
    T: Fn(&'a str) -> usize,
{
    if f(name) == 0 {
        return;
    }
}

#[wisp::action]
fn closure_return_type(flag: bool) {
    let add = |x: u8| -> u8 {
        if x > 1 {
            return x;
        }
        x + 1
    };
    if flag {
        return drop(vec![add(1)]);
    }
}

#[test]
fn actions_with_where_clauses_and_typed_closures() {
    assert!(where_arrow("", |s: &str| s.len()).is_ok());
    assert!(where_arrow("a", |s: &str| s.len()).is_ok());
    assert!(closure_return_type(true).is_ok());
}
