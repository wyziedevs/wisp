//! `#[derive(Json)]` as an app uses it, from outside the crate.

use wisp::Json;

fn json<T: Json>(value: &T) -> String {
    let mut out = String::new();
    value.json(&mut out);
    out
}

#[derive(Json)]
struct Key {
    /// Doc comments and visibility are skipped.
    pub(crate) letter: char,
    r#type: Mark,
    tags: Vec<&'static str>,
    next: Option<Box<Key>>,
}

#[derive(Json, Clone, Copy)]
enum Mark {
    Hit,
    #[allow(dead_code)]
    Miss = 2,
}

#[derive(Json)]
struct Pair(i32, String);

#[derive(Json)]
struct Id(u64);

#[derive(Json)]
struct Nothing;

#[derive(Json)]
struct Empty {}

#[test]
fn structs_are_objects() {
    let k = Key {
        letter: '<',
        r#type: Mark::Hit,
        tags: vec!["a"],
        next: Some(Box::new(Key {
            letter: 'b',
            r#type: Mark::Hit,
            tags: vec![],
            next: None,
        })),
    };
    assert_eq!(
        json(&k),
        r#"{"letter":"\u003c","type":"Hit","tags":["a"],"next":{"letter":"b","type":"Hit","tags":[],"next":null}}"#
    );
    assert_eq!(json(&Empty {}), "{}");
}

#[test]
fn tuple_structs_are_arrays_and_newtypes_their_value() {
    assert_eq!(json(&Pair(-3, "x".into())), r#"[-3,"x"]"#);
    assert_eq!(json(&Id(7)), "7");
    assert_eq!(json(&Nothing), "null");
}

#[test]
fn enums_are_their_variant_names() {
    assert_eq!(json(&[Mark::Hit, Mark::Miss]), r#"["Hit","Miss"]"#);
}
