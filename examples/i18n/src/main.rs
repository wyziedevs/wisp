// Translations: src/locales/{en,fr,ar}.json, pages under src/routes/[[lang=locale]]
// (`/`, `/fr`, `/ar/about`), `{t("key")}` in markup, prefix and fallback in
// Cargo.toml's `i18n`. `wisp dev`, then open http://127.0.0.1:3000/fr.
wisp::main!();

#[cfg(test)]
mod tests;
