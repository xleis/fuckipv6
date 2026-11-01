//! Browser adapter for the canonical i18n catalog.
//!
//! The catalog JSON is embedded into the runtime script at build time. This
//! keeps one translation source for Rust and JavaScript while avoiding a
//! second network request before the first UI paint.

const TEMPLATE: &str = include_str!("../assets/i18n-runtime.js");
const CATALOG_MARKER: &str = "__FUCKIPV6_I18N_CATALOG__";

pub fn browser_runtime_script() -> String {
    debug_assert!(TEMPLATE.contains(CATALOG_MARKER));
    TEMPLATE.replacen(CATALOG_MARKER, super::i18n::catalog_json(), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_runtime_embeds_the_canonical_catalog_once() {
        let script = browser_runtime_script();
        assert!(!script.contains(CATALOG_MARKER));
        assert!(script.contains(r#""schema_version": 1"#));
        assert!(script.contains(r#""zh-CN""#));
        assert!(script.contains("root.FuckIPV6I18n"));
    }
}
