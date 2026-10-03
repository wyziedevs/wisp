//! The web app manifest as Wisp serves it (`/manifest.webmanifest`), from
//! `src/manifest.json` at build or `wisp::app_manifest` in `init`.

use crate::json::{self, Json};

/// `text` (a JSON object) with what it leaves out filled in: `start_url`
/// `/`, `display` `standalone`, `name` and `short_name` from each other,
/// and `icons` (`static/`'s, as JSON) when it has none. Wisp's own
/// `offline` member is taken out: the second value is whether it was true.
pub fn complete(text: &str, icons: &str) -> Result<(String, bool), String> {
    let Json::Obj(mut members) = json::parse(text)? else {
        return Err("a web app manifest is a JSON object: {\"name\": \"Notes\"}".into());
    };
    let offline = members
        .iter()
        .any(|(k, v)| k == "offline" && *v == Json::Bool(true));
    members.retain(|(k, _)| k != "offline");
    let has = |members: &[(String, Json)], k: &str| members.iter().any(|(m, _)| m == k);
    let name = (members.iter())
        .find(|(k, _)| k == "name" || k == "short_name")
        .and_then(|(_, v)| v.as_str())
        .map(String::from);
    for (k, v) in [("name", name.clone()), ("short_name", name)] {
        if let (Some(v), false) = (v, has(&members, k)) {
            members.push((k.into(), Json::Str(v)));
        }
    }
    for (k, v) in [("start_url", "/"), ("display", "standalone")] {
        if !has(&members, k) {
            members.push((k.into(), Json::Str(v.into())));
        }
    }
    if !has(&members, "icons") && icons != "[]" {
        members.push(("icons".into(), json::parse(icons)?));
    }
    Ok((Json::Obj(members).to_string(), offline))
}

#[cfg(test)]
mod tests {
    use super::complete;

    #[test]
    fn fills_in_what_is_left_out() {
        let icons = r#"[{"src":"/icon-192.png","sizes":"192x192","type":"image/png"}]"#;
        let (m, offline) = complete(r#"{"name": "Notes", "offline": true}"#, icons).unwrap();
        assert!(offline);
        assert_eq!(
            m,
            format!(
                r#"{{"name":"Notes","short_name":"Notes","start_url":"/","display":"standalone","icons":{icons}}}"#
            )
        );
        // What it says stays; `offline` false is still not served.
        let (m, offline) = complete(
            r#"{"short_name": "N", "display": "browser", "icons": [], "offline": false}"#,
            icons,
        )
        .unwrap();
        assert!(!offline);
        assert_eq!(
            m,
            r#"{"short_name":"N","display":"browser","icons":[],"name":"N","start_url":"/"}"#
        );
        assert!(complete("[1]", "[]").unwrap_err().contains("a JSON object"));
        assert!(complete("{", "[]").is_err());
    }
}
