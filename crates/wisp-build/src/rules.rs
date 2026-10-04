//! `#[validate(…)]` (`wisp_shared::rules`) as the build uses it: the
//! attributes the browser checks a form's field by, from the field's type
//! and rules, and the fields of a page's actions.

use crate::rust_scan::{Items, TypeItem};
use crate::ty::{self, Scalar};
pub use wisp_shared::rules::{Key, Native, Rule, Validate, parse, plain, rule};

/// An upload's type: `Image` or `Upload` (any file, kept as a blob), or an
/// `Option` of one.
pub fn is_upload(ty: &str) -> bool {
    matches!(upload_kind(ty), "Image" | "Upload")
}

/// An `Upload` (or `Option<Upload>`): kept as a blob once the action's
/// inputs all pass.
pub fn is_blob(ty: &str) -> bool {
    upload_kind(ty) == "Upload"
}

fn upload_kind(ty: &str) -> &str {
    ty::last_segment(ty::option_inner(ty).unwrap_or(ty))
}

/// The `#[validate(rules)]` of parameter `name` of type `ty`, or what is
/// wrong with it: `max_size` is an upload's alone.
pub fn validate(rules: &str, name: &str, ty: &str) -> Result<Validate, String> {
    let v = parse(rules)?;
    if v.max_size.is_some() && !is_upload(ty) {
        return Err(format!(
            "`max_size` is for an upload, and `{name}` is a `{ty}`: make it an `Image` or `Upload` (or an `Option` of one)"
        ));
    }
    Ok(v)
}

/// For a field of type `ty` with `rules`; `whole` for a struct's field
/// (`fn default(post: Post)`), whose blank is missing.
pub fn native(ty: &str, rules: &[Rule], whole: bool) -> Native {
    let (optional, t) = match ty::option_inner(ty) {
        Some(inner) => (true, inner),
        None => (false, ty),
    };
    let last = ty::last_segment(ty::unref(t));
    let text = ty::is_text(t);
    let number = matches!(
        ty::scalar(t),
        Scalar::Unsigned | Scalar::Signed | Scalar::Float
    );
    let mut n = Native::default();
    if !text && !number && last != "Email" && !is_upload(t) {
        return n;
    }
    n.email = last == "Email";
    n.upload = is_upload(t);
    n.blob = is_blob(t);
    for r in rules {
        r.native(&mut n, text, number);
    }
    let blank_refused = whole || !text || n.email || n.min_len.is_some_and(|l| l > 0);
    n.required = !optional && blank_refused;
    n
}

/// A field of a form that posts to `action`, which reads it as `name`, of
/// type `ty` as written.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    pub action: String,
    pub name: String,
    pub ty: String,
    pub native: Native,
    /// A field of a struct parameter (`post: Post`), not a parameter itself.
    pub whole: bool,
}

/// A text field a form asks for in a `<textarea>`: one named for long text.
pub fn is_long(name: &str, ty: &str) -> bool {
    matches!(
        name,
        "body" | "bio" | "message" | "comment" | "description" | "notes" | "content"
    ) && ty::is_maybe_text(ty)
}

/// The `type` an `<input>` for a field takes (`<form fields>` writes it):
/// `""` for plain text.
pub fn input_type(name: &str, ty: &str) -> &'static str {
    let t = ty::option_inner(ty).unwrap_or(ty);
    if is_upload(t) {
        "file"
    } else if name == "password" || name.ends_with("_password") || ty::last_segment(t) == "Password"
    {
        "password"
    } else if ty::last_segment(t) == "Email" {
        "email"
    } else if ty::last_segment(t) == "bool" {
        "checkbox"
    } else if matches!(
        ty::scalar(t),
        Scalar::Unsigned | Scalar::Signed | Scalar::Float
    ) {
        "number"
    } else {
        ""
    }
}

/// The fields of the actions in `items`: each parameter, and each field of a struct that one reads whole (`fn
/// default(post: Post)`), which the file defines or the app's `shared`
/// modules do. None that is a route parameter in `params`: the route, not
/// the form, gives that.
pub fn fields(items: &Items, params: &[&str], shared: &[TypeItem]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut push = |action: &str, name: String, ty: &str, native: Native, whole: bool| {
        if !params.contains(&name.as_str()) {
            out.push(Field {
                action: action.to_string(),
                name,
                ty: ty.to_string(),
                native,
                whole,
            });
        }
    };
    for f in items.fns.iter().filter(|f| f.action) {
        for (p, t) in &f.params {
            let whole = items.types.iter().chain(shared).find(|s| {
                s.name == ty::last_segment(t)
                    && s.derives.iter().any(|d| d == "FromJson" || d == "Rest")
            });
            if let Some(s) = whole {
                for (name, ft) in &s.fields {
                    let name = name.strip_prefix("r#").unwrap_or(name);
                    if s.set_by_wisp(name) {
                        continue;
                    }
                    let rules = (s.rules.iter().filter(|(n, _)| n == name))
                        .flat_map(|(_, r)| parse(r).unwrap_or_default().rules)
                        .collect::<Vec<_>>();
                    push(
                        &f.name,
                        name.to_string(),
                        ft,
                        native(ft, &rules, true),
                        true,
                    );
                }
                continue;
            }
            // `body: T` is the whole JSON body, not a field.
            if p == "body" && !ty::is_maybe_text(t) {
                continue;
            }
            let rules = (f.checks.iter().filter(|(c, _)| c == p))
                .flat_map(|(_, r)| parse(r).unwrap_or_default().rules)
                .collect::<Vec<_>>();
            push(&f.name, p.clone(), t, native(t, &rules, false), false);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_check_what_the_server_refuses() {
        // (field: `type | rules`, a struct's when it starts `struct `; tag
        // and its type; the attributes it gets)
        for (field, tag, want) in [
            // Blank text is taken unless a rule refuses it.
            ("String |", "input", ""),
            (
                "String | min_len = 8",
                "input password",
                " required minlength=\"8\"",
            ),
            (
                "String | max_len = 5",
                "input",
                " pattern=\"[\\s\\S]{0,5}\"",
            ),
            (
                "String | len = 1..=100",
                "input",
                " required minlength=\"1\" pattern=\"[\\s\\S]{0,100}\"",
            ),
            ("String | len = 1..=100", "textarea", " required"),
            ("Option<String> | len = 1..", "input", " minlength=\"1\""),
            ("String | email", "input", " type=\"email\" required"),
            ("String | email", "input text", " required"),
            ("Option<Email> |", "input", " type=\"email\""),
            ("Email |", "input", " type=\"email\" required"),
            // A struct's blank field is missing.
            ("struct String |", "input", " required"),
            ("struct Option<String> |", "input", ""),
            (
                "u32 | min = 1, max = 10",
                "input number",
                " required min=\"1\" max=\"10\"",
            ),
            ("f64 | min = 0.5", "input number", " required"),
            ("u32 | min = 1", "input", " required"),
            ("u32 | min = LOW", "input number", " required"),
            (
                "Image | max_size = 1 * MB",
                "input file",
                " required accept=\"image/*\"",
            ),
            ("Option<Image> |", "input file", " accept=\"image/*\""),
            ("String | min_len = 1", "select", " required"),
            ("bool |", "input checkbox", ""),
            ("Vec<String> | len = 1..", "select", ""),
            ("char |", "input", ""),
        ] {
            let (whole, field) = match field.strip_prefix("struct ") {
                Some(f) => (true, f),
                None => (false, field),
            };
            let (ty, rules) = field.split_once('|').unwrap();
            let (tag, kind) = tag.split_once(' ').unwrap_or((tag, ""));
            let n = native(ty.trim(), &parse(rules).unwrap().rules, whole);
            assert_eq!(n.attrs(tag, kind, &|_| false), want, "{field} {tag}");
        }
        // What the tag says stays: no second `type`, `required` or `min`.
        let n = native("u32", &parse("min = 1").unwrap().rules, false);
        assert_eq!(n.attrs("input", "number", &|a| a == "value"), " required");
        let n = native("String", &parse("email").unwrap().rules, false);
        assert_eq!(n.attrs("input", "", &|a| a == "type"), " required");
        assert_eq!(n.attrs("select", "", &|a| a == "multiple"), "");
    }

    #[test]
    fn inputs_by_type() {
        for (name, ty, want) in [
            ("email", "Email", "email"),
            ("email", "Option<Email>", "email"),
            ("password", "String", "password"),
            ("new_password", "String", "password"),
            ("secret", "wisp::Password", "password"),
            ("avatar", "Image", "file"),
            ("avatar", "Option<Image>", "file"),
            ("agree", "bool", "checkbox"),
            ("age", "u8", "number"),
            ("price", "f64", "number"),
            ("name", "String", ""),
            ("tags", "Vec<String>", ""),
        ] {
            assert_eq!(input_type(name, ty), want, "{name}: {ty}");
        }
    }

    #[test]
    fn fields_of_actions() {
        let items = crate::rust_scan::scan(
            "#[derive(FromJson)] struct Post { #[validate(len = 1..=9)] title: String, note: Option<String>, done: bool }\n\
             #[action] fn add(#[validate(min_len = 2)] text: String, slug: String, n: Option<u8>) {}\n\
             #[action] fn default(post: Post) {}\n\
             #[action] fn save(mut note: models::Note) {}\n\
             fn helper(x: Email) {}",
        )
        .unwrap();
        // A type of the app's own modules too (`src/models.rs`).
        let shared = crate::rust_scan::scan(
            "#[derive(Rest)] pub struct Note { #[validate(min = 1)] stars: u8, created_at: String }",
        )
        .unwrap()
        .types;
        let got: Vec<(String, String, bool)> = fields(&items, &["slug"], &shared)
            .into_iter()
            .map(|f| (f.action, f.name, f.native.required))
            .collect();
        let want = [
            ("add", "text", true),
            ("add", "n", false),
            ("default", "title", true),
            ("default", "note", false),
            ("default", "done", false),
            ("save", "stars", true),
        ];
        let want: Vec<(String, String, bool)> = want
            .iter()
            .map(|&(a, n, r)| (a.into(), n.into(), r))
            .collect();
        assert_eq!(got, want);
    }
}
