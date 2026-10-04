//! Valid input the derives must accept: raw identifiers, paths, `Option` and
//! `Vec` fields, unusual types.

use wisp::{FromJson, Json, from_json, to_json};

#[derive(Json, FromJson, Debug, PartialEq)]
struct Raw {
    r#type: String,
    r#match: Option<u8>,
    list: std::vec::Vec<std::string::String>,
    f: Option<Vec<Option<u8>>>,
    r: std::collections::BTreeMap<String, Vec<u8>>,
}

#[test]
fn raw_identifiers_and_paths() {
    let body = r#"{"type":"a","match":3,"list":["x"],"f":[1,null],"r":{"k":[1]}}"#;
    let v: Raw = from_json(body.as_bytes()).unwrap();
    assert_eq!(v.r#type, "a");
    assert_eq!(to_json(&v), body);
}

#[derive(wisp::Cookie, Debug, PartialEq)]
struct Ck {
    r#type: u8,
    name: String,
}

#[derive(wisp::Cookie, Json, FromJson, Debug, PartialEq)]
enum Kind {
    A,
    r#Type,
}

#[test]
fn raw_cookie() {
    let c = Ck {
        r#type: 1,
        name: "n".into(),
    };
    assert_eq!(c.to_string().parse::<Ck>().unwrap(), c);
    assert_eq!(
        Kind::r#Type.to_string().parse::<Kind>().unwrap(),
        Kind::r#Type
    );
}

#[derive(wisp::Config)]
struct Conf {
    r#type: Option<String>,
}

#[derive(wisp::Config)]
#[allow(dead_code)]
struct NeedsType {
    r#type: String,
    other: std::option::Option<String>,
}

#[test]
fn config_names_a_raw_field_by_its_bare_variable() {
    let e = NeedsType::load().unwrap_err().message().to_string();
    assert!(e.contains("TYPE") && !e.contains("R#"), "{e}");
    assert!(!e.contains("OTHER"), "{e}");
}

#[test]
fn raw_config() {
    assert!(Conf::get().r#type.is_none());
}

#[wisp::action]
fn turbofish(flag: bool) {
    if flag {
        return drop(std::collections::HashMap::<u8, u8>::new());
    }
    let pairs = [(1u8, 2u8)];
    let _ = pairs.iter().map(|(a, b)| a + b).sum::<u8>();
}

fn unit<A, B>() {}

#[wisp::action]
fn turbofish_comma() {
    if unit::<u8, u8> as usize == 0 {
        return unit::<u8, u8>();
    }
}

#[wisp::action]
fn generic_bound<T: Fn(u8) -> u8 + Copy>(f: T) {
    let _ = f(1);
}

#[test]
fn actions_keep_their_returns() {
    assert!(turbofish(true).is_ok());
    assert!(turbofish(false).is_ok());
    assert!(generic_bound(|x| x).is_ok());
    assert!(turbofish_comma().is_ok());
}
