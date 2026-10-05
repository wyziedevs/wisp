//! A web app manifest made by `init` ([`app_manifest`]). The build makes
//! the rest: `src/manifest.json`, the service worker ([`crate::App::PWA`]).

use crate::Error;
use std::sync::OnceLock;

static ICONS: OnceLock<&'static str> = OnceLock::new();
static MANIFEST: OnceLock<String> = OnceLock::new();

/// In `init`, instead of `src/manifest.json`: the web app manifest, made at
/// startup (from the environment, say), served at `/manifest.webmanifest`
/// and linked from every page.
/// `wisp::app_manifest(r#"{"name": "Notes", "theme_color": "#7c3aed"}"#)?;`
/// What it leaves out is filled in as for the file (`start_url`, `display`,
/// `static/`'s icons). `"offline": true` goes in the file, which the build
/// reads.
pub fn app_manifest(json: &str) -> crate::Result {
    let icons = ICONS.get().copied().unwrap_or("[]");
    let fail = |e: String| Error::new(500, format!("wisp::app_manifest: {e}"));
    match wisp_shared::manifest::complete(json, icons, crate::protocol::BASE) {
        Ok((_, true)) => Err(fail(
            "`\"offline\": true` goes in src/manifest.json, which the build reads".into(),
        )),
        Ok((m, false)) => MANIFEST.set(m).map_err(|_| fail("called twice".into())),
        Err(e) => Err(fail(e)),
    }
}

/// The icons the build found, for [`app_manifest`]: before `init`.
pub(crate) fn icons(icons: &'static str) {
    let _ = ICONS.set(icons);
}

/// What [`app_manifest`] gave, if it was called.
pub(crate) fn manifest() -> Option<&'static str> {
    MANIFEST.get().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_from_init() {
        let msg = |r: crate::Result| r.unwrap_err().detail();
        assert!(msg(app_manifest("{\"offline\": true}")).contains("goes in src/manifest.json"));
        assert!(msg(app_manifest("[]")).contains("a JSON object"));
        assert_eq!(manifest(), None);
        icons("[{\"src\":\"/icon.svg\"}]");
        app_manifest("{\"name\": \"Notes\"}").unwrap();
        assert_eq!(
            manifest(),
            Some(
                "{\"name\":\"Notes\",\"short_name\":\"Notes\",\"start_url\":\"/\",\"display\":\"standalone\",\"icons\":[{\"src\":\"/icon.svg\"}]}"
            )
        );
        assert!(msg(app_manifest("{}")).contains("called twice"));
    }
}
