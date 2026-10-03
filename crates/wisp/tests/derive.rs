//! `#[derive(FromJson)]` with its `#[validate]` rules, the JSON parser and
//! `Value`, as an app uses them from outside the crate.

use std::collections::BTreeMap;
use wisp::{FromJson, Json, Value, from_json, json, to_json};

#[derive(FromJson, Debug, PartialEq)]
struct Signup {
    #[validate(len = 1..=10)]
    name: String,
    #[validate(email)]
    email: String,
    #[validate(min = 18, max = 120)]
    age: u8,
    #[validate(min_len = 1, max_len = 3)]
    tags: Vec<String>,
    nick: Option<String>,
    admin: bool,
}

const GOOD: &str = r#"{"name":"Ada","email":"ada@example.com","age":36,"tags":["a"]}"#;

fn problems<T: FromJson>(body: &str) -> Vec<(String, String)> {
    match from_json::<T>(body.as_bytes()) {
        Ok(_) => Vec::new(),
        Err(e) => e.fields().to_vec(),
    }
}

fn says(body: &str) -> Vec<String> {
    problems::<Signup>(body)
        .into_iter()
        .map(|(f, p)| format!("{f}: {p}"))
        .collect()
}

#[wisp::model]
struct Pet {
    #[validate(len = 1..=5)]
    name: String,
    tags: Vec<String>,
}

#[test]
fn model_derives_json_from_json_and_clone() {
    let pet: Pet = from_json(br#"{"name":"Rex","tags":["a","b"]}"#).unwrap();
    assert_eq!(to_json(&pet.clone()), r#"{"name":"Rex","tags":["a","b"]}"#);
    let bad = problems::<Pet>(r#"{"name":"","tags":[]}"#);
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn a_struct_is_read_from_an_object() {
    let got: Signup = from_json(GOOD.as_bytes()).unwrap();
    assert_eq!(
        got,
        Signup {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            age: 36,
            tags: vec!["a".into()],
            nick: None,
            admin: false,
        }
    );
    // An `Option` may be null, a `bool` left out is false, unknown members are ignored.
    let more = r#"{"name":"Ada","email":"a@b.c","age":18,"tags":["x","y","z"],"nick":null,"admin":true,"extra":[1,{"deep":null}]}"#;
    let got: Signup = from_json(more.as_bytes()).unwrap();
    assert_eq!((got.nick, got.admin, got.tags.len()), (None, true, 3));
    let nick = GOOD.replace('}', r#","nick":"A""#) + "}";
    assert_eq!(
        from_json::<Signup>(nick.as_bytes())
            .unwrap()
            .nick
            .as_deref(),
        Some("A")
    );
}

#[test]
fn every_rule_says_what_is_wrong_by_field() {
    for (body, expected) in [
        (
            GOOD.replace("Ada", ""),
            vec!["name: must have at least 1 character"],
        ),
        (
            GOOD.replace("Ada", "Ada Lovelace!"),
            vec!["name: must have at most 10 characters"],
        ),
        (
            GOOD.replace("ada@example.com", "ada.example.com"),
            vec!["email: must be an email address"],
        ),
        (
            GOOD.replace("ada@example.com", "a b@c.d"),
            vec!["email: must be an email address"],
        ),
        (
            GOOD.replace("ada@example.com", "a@b."),
            vec!["email: must be an email address"],
        ),
        (GOOD.replace("36", "17"), vec!["age: must be at least 18"]),
        (GOOD.replace("36", "121"), vec!["age: must be at most 120"]),
        (
            GOOD.replace(r#"["a"]"#, "[]"),
            vec!["tags: must have at least 1 item"],
        ),
        (
            GOOD.replace(r#"["a"]"#, r#"["a","b","c","d"]"#),
            vec!["tags: must have at most 3 items"],
        ),
    ] {
        let got = says(&body);
        assert_eq!(got.len(), 1, "{body}: {got:?}");
        assert!(
            got[0].starts_with(expected[0].split(':').next().unwrap()),
            "{got:?}"
        );
        assert!(
            got[0].contains(expected[0].split(": ").nth(1).unwrap()),
            "{got:?} for {body}"
        );
    }
    // An address as browsers take one (WHATWG): the domain needs no dot.
    let dotless = GOOD.replace("ada@example.com", "a@b");
    assert!(says(&dotless).is_empty(), "{dotless}");
    // Every problem at once, not the first.
    let all = says(r#"{"name":"","email":"x","age":1,"tags":[]}"#);
    assert_eq!(all.len(), 4, "{all:?}");
    let fields: Vec<String> = problems::<Signup>(r#"{"name":"","email":"x","age":1,"tags":[]}"#)
        .into_iter()
        .map(|(f, _)| f)
        .collect();
    assert_eq!(fields, ["name", "email", "age", "tags"]);
}

#[test]
fn what_is_missing_or_of_the_wrong_type_is_named() {
    assert!(
        says("{}").iter().any(|p| p.starts_with("name: ")),
        "{:?}",
        says("{}")
    );
    let wrong = says(r#"{"name":5,"email":true,"age":"x","tags":{}}"#);
    assert_eq!(wrong.len(), 4, "{wrong:?}");
    // Whole numbers must fit: 300 is not a u8, 1.5 is not a whole number, -1 is not unsigned.
    for age in ["300", "1.5", "-1", "1e2", "\"36\"", "null"] {
        let body = GOOD.replace("36", age);
        let got = says(&body);
        if age == "1e2" {
            continue; // an exponent is a number; whether it is whole is up to the reader
        }
        assert!(got.iter().any(|p| p.starts_with("age: ")), "{age}: {got:?}");
    }
    // Not an object at all.
    for body in ["[]", "1", "\"x\"", "null", "true"] {
        assert!(!problems::<Signup>(body).is_empty(), "{body}");
    }
    // Nested problems say where.
    let nested = says(r#"{"name":"a","email":"a@b.c","age":20,"tags":["x",5]}"#);
    assert_eq!(nested.len(), 1);
    assert!(nested[0].starts_with("tags"), "{nested:?}");
    assert!(
        nested[0].contains('1'),
        "the position of the item: {nested:?}"
    );
}

#[test]
fn syntax_errors_are_400_and_say_where() {
    let err = from_json::<Signup>(b"{\"name\" 1}").unwrap_err();
    assert_eq!(err.status(), 400);
    assert_eq!(
        err.message(),
        "Invalid JSON: expected `:` at line 1, column 9"
    );
    let err = from_json::<Signup>(b"{\n  \"name\": ,\n}").unwrap_err();
    assert_eq!(err.status(), 400);
    assert!(
        err.message().ends_with("at line 2, column 11"),
        "{}",
        err.message()
    );
    assert_eq!(
        from_json::<Signup>(&[0xff, 0xfe]).unwrap_err().status(),
        400
    );
    assert_eq!(from_json::<Signup>(b"").unwrap_err().status(), 400);
    // Well-formed but not a Signup: 422, by field.
    assert_eq!(from_json::<Signup>(b"{}").unwrap_err().status(), 422);
}

#[derive(FromJson, Debug, PartialEq)]
struct Id(u64);

#[derive(FromJson, Debug, PartialEq)]
struct Pair(i32, String);

#[derive(FromJson, Debug, PartialEq)]
struct Unit;

#[derive(FromJson, Debug, PartialEq, Clone, Copy)]
enum Level {
    Low,
    #[allow(dead_code)]
    High = 5,
}

#[derive(FromJson, Debug, PartialEq)]
struct Everything {
    id: Id,
    pair: Pair,
    level: Level,
    map: BTreeMap<String, u8>,
    boxed: Box<Id>,
    any: Value,
    floats: Vec<f64>,
    signed: i64,
    big: u128,
    unit: Unit,
    #[validate(max_len = 2)]
    text: Option<String>,
}

#[test]
fn other_shapes_are_read() {
    let body = r#"{"id":7,"pair":[-3,"x"],"level":"High","map":{"a":1,"b":2},"boxed":9,
        "any":{"k":[1,2,{"z":null}]},"floats":[1.5,-2e3,0],"signed":-9223372036854775808,
        "big":340282366920938463463374607431768211455,"unit":null}"#;
    let got: Everything = from_json(body.as_bytes()).unwrap();
    assert_eq!(got.id, Id(7));
    assert_eq!(got.pair, Pair(-3, "x".into()));
    assert_eq!(got.level, Level::High);
    assert_eq!(got.map, BTreeMap::from([("a".into(), 1), ("b".into(), 2)]));
    assert_eq!(got.boxed, Box::new(Id(9)));
    assert_eq!(
        got.any
            .get("k")
            .and_then(Value::as_array)
            .map(<[Value]>::len),
        Some(3)
    );
    assert_eq!(got.floats, [1.5, -2000.0, 0.0]);
    assert_eq!((got.signed, got.big), (i64::MIN, u128::MAX));
    assert_eq!(got.text, None);

    let bad = |replace: (&str, &str)| problems::<Everything>(&body.replace(replace.0, replace.1));
    assert!(
        !bad((r#""level":"High""#, r#""level":"Mid""#)).is_empty(),
        "an unknown variant"
    );
    assert!(!bad((r#""level":"High""#, r#""level":1"#)).is_empty());
    assert!(
        !bad((r#""pair":[-3,"x"]"#, r#""pair":[-3]"#)).is_empty(),
        "a tuple of the wrong length"
    );
    assert!(!bad((r#""pair":[-3,"x"]"#, r#""pair":[-3,"x",1]"#)).is_empty());
    assert!(
        !bad((r#""map":{"a":1,"b":2}"#, r#""map":{"a":300}"#)).is_empty(),
        "a value of the map"
    );
    assert!(!bad((r#""map":{"a":1,"b":2}"#, r#""map":[]"#)).is_empty());
    assert!(!bad((r#""unit":null"#, r#""unit":0"#)).is_empty());
    assert!(!bad((r#""floats":[1.5,-2e3,0]"#, r#""floats":[1,"2"]"#)).is_empty());
    let long = bad((r#""unit":null"#, r#""unit":null,"text":"abc""#));
    assert_eq!(
        long.len(),
        1,
        "a rule on an Option checks a value that is there: {long:?}"
    );
    assert!(bad((r#""unit":null"#, r#""unit":null,"text":"ab""#)).is_empty());
    assert!(
        bad((r#""unit":null"#, r#""unit":null,"text":null"#)).is_empty(),
        "None passes its rules"
    );
}

#[test]
fn parse_accepts_json_and_only_json() {
    for good in [
        "0",
        "-0",
        "1",
        "-1",
        "1.5",
        "0.5",
        "1e10",
        "1E+2",
        "1.5e-3",
        "-0.0e0",
        "123456789012345678901234567890",
        "true",
        "false",
        "null",
        "\"\"",
        "\"a\"",
        "\"\\u00e9\"",
        "\"\\ud83d\\ude00\"",
        "\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"",
        "\"\u{e9}\u{1f600}\"",
        "[]",
        "{}",
        "[[]]",
        "[{}]",
        " [ 1 , 2 ] ",
        "\t\r\n{ \"a\" : 1 }\n",
        "{\"a\":1,\"a\":2}",
        "[1,[2,[3]]]",
    ] {
        assert!(
            json::parse(good).is_ok(),
            "{good:?}: {:?}",
            json::parse(good)
        );
    }
    for bad in [
        "",
        " ",
        "[",
        "]",
        "{",
        "}",
        "[1,]",
        "[,1]",
        "[1 2]",
        "{\"a\":1,}",
        "{,}",
        "{\"a\"}",
        "{\"a\" 1}",
        "{'a':1}",
        "{a:1}",
        "{1:1}",
        "[01]",
        "[1.]",
        "[.5]",
        "[+1]",
        "[1e]",
        "[1e+]",
        "[-]",
        "[--1]",
        "[0x1]",
        "[NaN]",
        "[Infinity]",
        "[-Infinity]",
        "tru",
        "nul",
        "fals",
        "truee",
        "True",
        "NULL",
        "[1]x",
        "1 2",
        "//c\n1",
        "/*c*/1",
        "#1",
        "\"\\x\"",
        "\"\\u12\"",
        "\"\\u12g4\"",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\ud800\\u0041\"",
        "\"a",
        "\"a\nb\"",
        "\"a\tb\"",
        "\"\u{1}\"",
        "'a'",
        "[1,2",
        "[\"a\":1]",
        "\u{feff}1",
        "\u{a0}1",
    ] {
        assert!(json::parse(bad).is_err(), "{bad:?} should not parse");
    }
    // Nesting is bounded, so a hostile body cannot use up the stack.
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(json::parse(&deep(100)).is_ok());
    assert!(json::parse(&deep(100_000)).is_err());
    let objects = "{\"a\":".repeat(100_000) + "1" + &"}".repeat(100_000);
    assert!(json::parse(&objects).is_err());
}

#[test]
fn errors_say_where() {
    for (text, message) in [
        ("[1,]", "expected a value at line 1, column 4"),
        ("{\"a\":1 \"b\"}", "expected `,` or `}` at line 1, column 8"),
        ("[1\n,\n2 3]", "expected `,` or `]` at line 3, column 3"),
        ("", "expected a value, found the end at line 1, column 1"),
        ("1 2", "unexpected text after the value at line 1, column 3"),
    ] {
        assert_eq!(json::parse(text).unwrap_err(), message, "{text:?}");
    }
}

#[test]
fn values_read_back() {
    let v = json::parse(
        r#"{"s":"x","n":-7,"f":1.5,"b":true,"z":null,"a":[1,"two"],"o":{"k":1},"big":1e400}"#,
    )
    .unwrap();
    assert_eq!(v.get("s").and_then(Value::as_str), Some("x"));
    assert_eq!(v.get("n").and_then(Value::as_i64), Some(-7));
    assert_eq!(v.get("n").and_then(Value::as_f64), Some(-7.0));
    assert_eq!(v.get("f").and_then(Value::as_f64), Some(1.5));
    assert_eq!(v.get("f").and_then(Value::as_i64), None, "1.5 is not whole");
    assert_eq!(v.get("b").and_then(Value::as_bool), Some(true));
    assert!(v.get("z").is_some_and(Value::is_null));
    assert!(!v.get("s").unwrap().is_null());
    assert_eq!(
        v.get("a").and_then(Value::as_array).map(<[Value]>::len),
        Some(2)
    );
    assert_eq!(
        v.get("o").and_then(|o| o.get("k")).and_then(Value::as_i64),
        Some(1)
    );
    assert!(v.get("missing").is_none());
    // The wrong kind is `None`, never a guess.
    assert_eq!(v.get("s").and_then(Value::as_i64), None);
    assert_eq!(v.get("n").and_then(Value::as_str), None);
    assert_eq!(v.get("s").and_then(Value::as_bool), None);
    assert_eq!(v.get("s").and_then(Value::as_array), None);
    assert_eq!(v.get("s").and_then(Value::as_f64), None);
    assert!(Value::Null.get("x").is_none());
    // The last of a repeated key, as `JSON.parse` has it.
    let dup = json::parse(r#"{"a":1,"a":2}"#).unwrap();
    assert_eq!(dup.get("a").and_then(Value::as_i64), Some(2));
    // A `Value` is written back as it was read.
    let text = r#"{"a":[1,2.5,"x\n",true,null],"b":{}}"#;
    assert_eq!(to_json(&json::parse(text).unwrap()), text);
}

#[test]
fn json_is_written_escaped() {
    assert_eq!(
        to_json("a\"b\\c\n\t\r\u{8}\u{c}\u{1}"),
        r#""a\"b\\c\n\t\r\u0008\u000c\u0001""#
    );
    // Nothing that could end a script or an attribute is left as it is.
    assert_eq!(
        to_json("</script><!--&'"),
        r#""\u003c/script\u003e\u003c!--\u0026'""#
    );
    assert_eq!(to_json("\u{2028}\u{2029}é😀"), "\"\\u2028\\u2029é😀\"");
    assert_eq!(to_json(&f64::NAN), "null");
    assert_eq!(to_json(&f64::INFINITY), "null");
    assert_eq!(to_json(&1.5f64), "1.5");
    assert_eq!(to_json(&-0.0f64), "-0");
    assert_eq!(
        to_json(&u128::MAX),
        "340282366920938463463374607431768211455"
    );
    assert_eq!(to_json(&i64::MIN), "-9223372036854775808");
    assert_eq!(to_json(&true), "true");
    assert_eq!(to_json(&None::<u8>), "null");
    assert_eq!(to_json(&Some("x")), "\"x\"");
    assert_eq!(to_json(&()), "null");
    assert_eq!(to_json(&vec![vec![1u8], vec![]]), "[[1],[]]");
    assert_eq!(
        to_json(&BTreeMap::from([("b", 1u8), ("a", 2)])),
        r#"{"a":2,"b":1}"#
    );
    assert_eq!(to_json(&('x', 1u8)), r#"["x",1]"#);
    assert_eq!(to_json(&Box::new(3u8)), "3");
    assert_eq!(to_json(&String::from("s")), "\"s\"");
}

#[derive(Json, FromJson, Debug, PartialEq)]
struct Round {
    a: Vec<Option<String>>,
    b: BTreeMap<String, Vec<u8>>,
    c: Level2,
}

#[derive(Json, FromJson, Debug, PartialEq)]
enum Level2 {
    One,
    Two,
}

#[test]
fn what_is_written_is_read_back() {
    let value = Round {
        a: vec![Some("x\"y".into()), None, Some("é</b>".into())],
        b: BTreeMap::from([("k".into(), vec![1, 2]), ("j".into(), vec![])]),
        c: Level2::Two,
    };
    let text = to_json(&value);
    assert_eq!(from_json::<Round>(text.as_bytes()).unwrap(), value);
    let _ = Level2::One;
}

#[test]
fn every_kind_of_value_is_written_as_json() {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::Arc;

    let mut n = 5u8;
    assert_eq!(to_json(&&mut n), "5");
    assert_eq!(to_json(&Rc::new("rc")), "\"rc\"");
    assert_eq!(to_json(&Arc::new(vec![1u8])), "[1]");
    assert_eq!(to_json(&Cow::Borrowed("cow")), "\"cow\"");
    assert_eq!(to_json(&Cow::<str>::Owned("own".into())), "\"own\"");
    assert_eq!(to_json(&'c'), "\"c\"");
    assert_eq!(to_json(&'"'), r#""\"""#);
    assert_eq!(to_json(&[1u8, 2, 3]), "[1,2,3]");
    assert_eq!(to_json(&[1u8, 2][..]), "[1,2]");
    assert_eq!(to_json(&(1u8, "two", 3.5f32)), r#"[1,"two",3.5]"#);
    assert_eq!(to_json(&(1u8, 2u8, 3u8, 4u8)), "[1,2,3,4]");
    assert_eq!(
        to_json(&i128::MIN),
        "-170141183460469231731687303715884105728"
    );
    assert_eq!(to_json(&f32::NEG_INFINITY), "null");
    assert_eq!(to_json(&1e21f64), "1000000000000000000000");
    let one = HashMap::from([("k".to_string(), vec![Some(1u8), None])]);
    assert_eq!(to_json(&one), r#"{"k":[1,null]}"#);
    let mut two: Vec<String> = to_json(&HashMap::from([("a", 1u8), ("b", 2)]))
        .trim_matches(['{', '}'])
        .split(',')
        .map(String::from)
        .collect();
    two.sort();
    assert_eq!(two, [r#""a":1"#, r#""b":2"#]);
}

#[test]
fn other_containers_are_read_from_json() {
    use std::collections::HashMap;
    let got: HashMap<String, Vec<Option<u8>>> = from_json(br#"{"a":[1,null],"b":[]}"#).unwrap();
    assert_eq!(got["a"], [Some(1), None]);
    assert!(got["b"].is_empty());
    assert!(from_json::<Vec<u8>>(b"[1,2,300]").is_err());
    assert_eq!(
        from_json::<Vec<Vec<u8>>>(b"[[1],[],[2,3]]").unwrap().len(),
        3
    );
    assert_eq!(from_json::<Option<u8>>(b"null").unwrap(), None);
    assert!(from_json::<bool>(b"true").unwrap());
    assert_eq!(from_json::<String>(br#""cafe""#).unwrap(), "cafe");
    assert_eq!(from_json::<f32>(b"1.5").unwrap(), 1.5);
    assert_eq!(from_json::<i8>(b"-128").unwrap(), -128);
    assert!(from_json::<i8>(b"-129").is_err());
    assert!(from_json::<u64>(b"18446744073709551616").is_err());
    assert_eq!(
        from_json::<Value>(b" [1, {\"a\": null}] ")
            .unwrap()
            .as_array()
            .map(<[Value]>::len),
        Some(2)
    );
    // A unit is `null`, and a string is not a number.
    assert!(from_json::<()>(b"null").is_ok());
    assert!(from_json::<u8>(b"\"1\"").is_err());
}

/// What a body reads as through a `Value` alone, the way `from_json` read
/// every body before its straight path (`json::Direct`).
fn through_value<T: FromJson>(body: &[u8]) -> Option<T> {
    let text = std::str::from_utf8(body).ok()?;
    if text.trim().is_empty() {
        return T::missing();
    }
    let v = json::parse(text).ok()?;
    let mut p = json::Problems::default();
    T::from_json(&v, &mut p).filter(|_| p.is_empty())
}

/// The straight path reads what the `Value` path reads, and refuses what
/// it refuses: over bodies broken every way, keys twice, escapes, nesting.
#[test]
fn the_straight_path_reads_what_a_value_does() {
    let seeds = [
        GOOD,
        r#"{"name":"Ada","email":"a@b.c","age":18,"tags":["x","y","z"],"nick":null,"admin":true,"extra":[1,{"deep":null}]}"#,
        r#" { "age" : 40 , "name" : "Bo" , "email" : "b@c.d" , "tags" : [ "q" ] , "name" : "Cy" } "#,
        r#"{"name":"A\"da","email":"a@b.c","age":20,"tags":["A"],"admin":false}"#,
        r#"{"name":"Ada","email":"a@b.c","age":2e1,"tags":[],"nick":"n"}"#,
    ];
    let mut rng = wisp_shared::rng::Rng::new(9);
    let special = b"{}[]\":,\\ 0123456789-.eEtrufalsn";
    for _ in 0..40_000 {
        let mut b = seeds[rng.below(seeds.len())].as_bytes().to_vec();
        for _ in 0..rng.below(4) {
            let at = rng.below(b.len() + 1);
            match rng.below(4) {
                0 if at < b.len() => b[at] = rng.pick(special),
                1 if at < b.len() => drop(b.remove(at)),
                2 => b.insert(at, rng.pick(special)),
                _ => b.truncate(at),
            }
        }
        let text = String::from_utf8_lossy(&b);
        let want = through_value::<Signup>(&b);
        let got = from_json::<Signup>(&b).ok();
        assert_eq!(got, want, "{text}");
        assert_eq!(
            from_json::<Vec<Option<i64>>>(&b).ok(),
            through_value(&b),
            "{text}"
        );
    }
}

/// Rows kept in memory, to read what a saved table wrote.
struct Mem(std::sync::Mutex<Vec<(String, u64, String)>>);

impl wisp::Store for &'static Mem {
    fn load(&self, table: &str) -> wisp::Result<Vec<(u64, String)>> {
        let rows = self.0.lock().unwrap();
        Ok(rows
            .iter()
            .filter(|r| r.0 == table)
            .map(|r| (r.1, r.2.clone()))
            .collect())
    }

    fn save(&self, table: &str, id: u64, json: Option<&str>) -> wisp::Result {
        let mut rows = self.0.lock().unwrap();
        rows.retain(|r| !(r.0 == table && r.1 == id));
        if let Some(j) = json {
            rows.push((table.into(), id, j.into()));
        }
        Ok(())
    }
}

static KEPT: Mem = Mem(std::sync::Mutex::new(Vec::new()));

#[wisp::model]
struct Member {
    #[unique]
    email: wisp::Email,
    password: wisp::Password,
    #[json(default)]
    age: u8,
    #[json(was = "nick", default = "anon".to_string())]
    name: String,
}

static MEMBERS: wisp::Table<Member> = wisp::Table::saved("members");

/// `#[json(default)]` and `#[json(was)]` bring old rows and short bodies to
/// the type, on the straight path and through a `Value` alike.
#[test]
fn old_rows_are_read_as_they_are_now() {
    for (body, name, age) in [
        (r#"{"email":"a@b.c","password":"pw"}"#, "anon", 0),
        (
            r#"{"email":"a@b.c","password":"pw","nick":"Al","age":3}"#,
            "Al",
            3,
        ),
        (
            r#"{"email":"a@b.c","password":"pw","nick":"Al","name":"Bo"}"#,
            "Bo",
            0,
        ),
        (
            r#"{"name":"Bo","nick":"Al","email":"a@b.c","password":"pw"}"#,
            "Bo",
            0,
        ),
    ] {
        let direct = from_json::<Member>(body.as_bytes()).unwrap();
        let by_value = through_value::<Member>(body.as_bytes()).unwrap();
        for m in [direct, by_value] {
            assert_eq!((m.name.as_str(), m.age), (name, age), "{body}");
        }
    }
    // What is not defaulted is still required.
    assert_eq!(problems::<Member>(r#"{"password":"pw"}"#).len(), 1);
}

/// `#[unique]` makes a saved table refuse a repeat; a `Password` is kept as
/// its hash, shown as nothing, and signs in.
#[test]
fn unique_and_password_fields() {
    wisp::store(&KEPT);
    let kept = || {
        KEPT.0
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.2.clone())
            .collect::<String>()
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let ann = |email: &str| {
        from_json::<Member>(format!(r#"{{"email":"{email}","password":"pw"}}"#).as_bytes()).unwrap()
    };
    let row = rt
        .block_on(wisp::signup(&MEMBERS, ann("ann@x.io")))
        .unwrap();
    let again = rt
        .block_on(wisp::signup(&MEMBERS, ann("ann@x.io")))
        .err()
        .unwrap();
    assert_eq!(again.status(), 422);
    assert_eq!(MEMBERS.len(), 1);
    assert_eq!(MEMBERS.by("ann@x.io").unwrap().id, row.id);
    assert!(MEMBERS.try_add(ann("ann@x.io")).is_err());

    assert!(row.value.password.hashed());
    assert!(
        !to_json(&row.value).contains("$pbkdf2")
            && to_json(&row.value).contains(r#""password":null"#)
    );
    let got = rt
        .block_on(wisp::login(&MEMBERS, "ann@x.io", "pw"))
        .unwrap();
    assert_eq!(got.id, row.id);
    assert!(
        rt.block_on(wisp::login(&MEMBERS, "ann@x.io", "nope"))
            .is_err()
    );
    assert!(kept().contains("$pbkdf2-sha256$") && !kept().contains(r#"":"pw""#));
}

#[wisp::model]
struct Login {
    #[unique]
    email: wisp::Email,
    password: wisp::Password,
    #[json(default)]
    note: String,
}

static LOGINS: wisp::Table<Login> = wisp::Table::saved("logins");

fn login_row(email: &str, password: &str) -> Login {
    from_json::<Login>(format!(r#"{{"email":"{email}","password":"{password}"}}"#).as_bytes())
        .unwrap()
}

/// Text typed that looks like a hash is a password: hashed, not kept as
/// the hash it resembles.
#[test]
fn a_typed_hash_is_a_password() {
    wisp::store(&KEPT);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let look = rt.block_on(wisp::password::hash("secret")).unwrap();
    let typed = login_row("look@x.io", &look);
    assert!(!typed.password.hashed());
    let id = LOGINS.add(typed);
    let kept = LOGINS.get(id).unwrap().value.password;
    assert!(kept.hashed() && kept.as_str() != look);
    let check = |p: &wisp::Password, s: &str| rt.block_on(wisp::Password::check(Some(p), s));
    assert!(check(&kept, &look).unwrap());
    assert!(!check(&kept, "secret").unwrap());
}

/// A plain `Password` added to a table is hashed once as it enters; later
/// writes and the admin's views do not hash it again.
#[test]
fn a_direct_add_is_hashed_once() {
    wisp::store(&KEPT);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let id = LOGINS.add(login_row("once@x.io", "hunter22"));
    let first = LOGINS.get(id).unwrap().value.password;
    assert!(first.hashed() && first.as_str().starts_with("$pbkdf2-sha256$"));
    let t = std::time::Instant::now();
    LOGINS.update(id, |l| l.note = "hi".into());
    LOGINS.update(id, |l| l.note = "again".into());
    let mut same = LOGINS.get(id).unwrap().value;
    assert!(
        t.elapsed() < std::time::Duration::from_millis(100),
        "rehashed"
    );
    assert!(same.password == first);
    // A whole row put back (admin, or `set`) keeps its hash.
    same.note = "set".into();
    LOGINS.set(id, same);
    assert!(LOGINS.get(id).unwrap().value.password == first);
    // A new password set in `update` is hashed, once.
    LOGINS.update(id, |l| {
        l.password = wisp::Password::Plain("new pass 1".into())
    });
    let second = LOGINS.get(id).unwrap().value.password;
    assert!(second.hashed() && second != first);
    assert!(
        rt.block_on(wisp::Password::check(Some(&second), "new pass 1"))
            .unwrap()
    );
}
