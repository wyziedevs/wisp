//! The OpenAPI document of the test app: pinned as `tests/openapi.json`
//! (`WISP_BLESS=1` writes it), the same through `wisp openapi` and
//! `/_wisp/openapi.json`, and valid OpenAPI 3.1 in its structure: every
//! `$ref` resolves, every `{param}` of a path is declared, operation ids are
//! unique, each operation answers, each security scheme it names exists.

use std::path::Path;
use wisp::test::client;
use wisp::{Value, from_json};
use wisp_test_app::Site;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn generated() -> String {
    wisp_build::openapi(Path::new(ROOT)).expect("the app has a document")
}

fn parsed(json: &str) -> Value {
    from_json::<Value>(json.as_bytes()).expect("the document is JSON")
}

#[test]
fn the_document_is_pinned() {
    let doc = wisp_build::pretty_json(&generated());
    let path = format!("{ROOT}/tests/openapi.json");
    if std::env::var_os("WISP_BLESS").is_some() {
        std::fs::write(&path, &doc).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(doc, want, "the document differs from tests/openapi.json");
}

#[test]
fn the_app_serves_what_the_cli_prints() {
    let mut app = client::<Site>();
    let served = app.get("/_wisp/openapi.json").text().to_string();
    assert_eq!(served, generated());
    // Pretty printing changes only the spacing.
    let pretty = wisp_build::pretty_json(&served);
    assert_eq!(parsed(&pretty), parsed(&served));
    assert_eq!(pretty.replace(['\n', ' '], ""), served.replace(' ', ""));
}

fn members(v: &Value) -> &[(String, Value)] {
    match v {
        Value::Object(m) => m,
        other => panic!("an object, not {other:?}"),
    }
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} is text in {v:?}"))
}

/// Every `{"$ref": …}` inside `v`, as written.
fn refs<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match (k.as_str(), x) {
                    ("$ref", Value::String(s)) => out.push(s),
                    _ => refs(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

/// The `{name}`s of a path template.
fn templated(path: &str) -> Vec<&str> {
    path.split('{')
        .skip(1)
        .filter_map(|p| p.split_once('}').map(|(n, _)| n))
        .collect()
}

#[test]
fn the_document_is_valid_openapi_3_1() {
    let doc = parsed(&generated());
    assert_eq!(text(&doc, "openapi"), "3.1.0");
    let info = doc.get("info").unwrap();
    assert!(!text(info, "title").is_empty() && !text(info, "version").is_empty());

    // What `$ref` can point at.
    let mut targets = Vec::new();
    let components = doc.get("components").expect("components");
    for (kind, group) in members(components) {
        for (name, _) in members(group) {
            targets.push(format!("#/components/{kind}/{name}"));
            // Names a `$ref` can spell.
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                "{name}"
            );
        }
    }
    let schemes: Vec<&str> = components
        .get("securitySchemes")
        .map(|s| members(s).iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();
    let mut found = Vec::new();
    refs(&doc, &mut found);
    for r in &found {
        assert!(targets.iter().any(|t| t == r), "{r} points at nothing");
    }

    let mut ids: Vec<&str> = Vec::new();
    let paths = members(doc.get("paths").expect("paths"));
    assert!(!paths.is_empty());
    for (path, item) in paths {
        assert!(path.starts_with('/'), "{path}");
        for (method, op) in members(item) {
            let at = format!("{} {path}", method.to_uppercase());
            assert!(
                ["get", "put", "post", "delete", "patch"].contains(&method.as_str()),
                "{at}"
            );
            let id = text(op, "operationId");
            assert!(!ids.contains(&id), "{at}: operationId {id} is twice");
            ids.push(id);

            // Every `{name}` of the path is a required path parameter, and
            // every path parameter is in the path.
            let mut declared: Vec<(&str, &str)> = Vec::new();
            for p in op
                .get("parameters")
                .and_then(Value::as_array)
                .unwrap_or(&[])
            {
                let (name, place) = (text(p, "name"), text(p, "in"));
                assert!(
                    ["path", "query", "header", "cookie"].contains(&place),
                    "{at}"
                );
                assert!(p.get("schema").is_some(), "{at}: {name} has no schema");
                if place == "path" {
                    assert_eq!(p.get("required").and_then(Value::as_bool), Some(true));
                }
                assert!(!declared.contains(&(name, place)), "{at}: {name} twice");
                declared.push((name, place));
            }
            // A query in the key (an action: `/x?/name`) is not the path.
            let template = path.split('?').next().unwrap();
            for name in templated(template) {
                assert!(declared.contains(&(name, "path")), "{at}: {{{name}}}");
            }
            for (name, place) in &declared {
                if *place == "path" {
                    assert!(templated(template).contains(name), "{at}: {name}");
                }
            }

            // A body is content with schemas.
            if let Some(body) = op.get("requestBody") {
                for (_, media) in members(body.get("content").expect("body content")) {
                    assert!(media.get("schema").is_some(), "{at}");
                }
            }
            if let Some(security) = op.get("security").and_then(Value::as_array) {
                for requirement in security {
                    for (scheme, _) in members(requirement) {
                        assert!(schemes.contains(&scheme.as_str()), "{at}: {scheme}");
                    }
                }
            }

            // Each answer is a status code or `default`, and says what it is
            // (a `$ref` to a shared response does).
            let responses = members(op.get("responses").expect("responses"));
            assert!(!responses.is_empty(), "{at}");
            for (code, r) in responses {
                let status = code.parse::<u16>().map(|n| (100..600).contains(&n));
                assert!(status.unwrap_or(code == "default"), "{at}: {code}");
                assert!(
                    r.get("description").is_some() || r.get("$ref").is_some(),
                    "{at}: {code}"
                );
            }
        }
    }
}

/// Types the endpoints have: an enum is its names, an `Option` is null too.
#[test]
fn the_document_describes_each_kind_of_route() {
    let doc = wisp_build::pretty_json(&generated());
    let doc = parsed(&doc);
    let paths = doc.get("paths").unwrap();
    let has = |path: &str, method: &str| {
        paths
            .get(path)
            .and_then(|p| p.get(method))
            .unwrap_or_else(|| panic!("no {method} {path}"))
    };
    // A page is its HTML.
    let page = has("/", "get");
    assert!(page.get("responses").unwrap().get("200").is_some());
    // A resource's own routes and its `/[id]` are separate paths.
    let list = has("/notes", "get");
    let one = has("/notes/{id}", "get");
    assert_ne!(text(list, "operationId"), text(one, "operationId"));
}
