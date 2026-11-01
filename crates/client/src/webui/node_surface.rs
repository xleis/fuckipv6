//! Manifest-driven browser surface for the shared node HTTP contract.
//!
//! The browser is deliberately a renderer and input adapter only. It fetches
//! `NodeUiManifest`, projects `NodeSnapshot`, and submits the versioned
//! `NodeCommandHttpRequest` DTO; it does not own runtime or network state.

pub(super) const NODE_MANIFEST_HTML: &str = include_str!("node_ui.html");
pub(super) const NODE_UI_JS: &str = include_str!("node_ui.js");

#[cfg(test)]
mod tests {
    use super::{NODE_MANIFEST_HTML, NODE_UI_JS};
    use crate::node::{node_ui_manifest, NodePrimaryStatus, NodeUiManifest};
    use fuckipv6_common::i18n::{message_keys, t, Locale};
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn marker_json(source: &str, begin: &str, end: &str) -> Value {
        let start = source
            .find(begin)
            .unwrap_or_else(|| panic!("missing marker {begin}"))
            + begin.len();
        let finish = source[start..]
            .find(end)
            .unwrap_or_else(|| panic!("missing marker {end}"))
            + start;
        serde_json::from_str(source[start..finish].trim())
            .unwrap_or_else(|error| panic!("invalid JSON between {begin} and {end}: {error}"))
    }

    fn embedded_manifest() -> Value {
        marker_json(
            NODE_UI_JS,
            "/* NODE_UI_MANIFEST_CONTRACT_BEGIN */",
            "/* NODE_UI_MANIFEST_CONTRACT_END */",
        )
    }

    fn source_between<'a>(source: &'a str, begin: &str, end: &str) -> &'a str {
        let start = source
            .find(begin)
            .unwrap_or_else(|| panic!("missing source marker {begin}"))
            + begin.len();
        let finish = source[start..]
            .find(end)
            .unwrap_or_else(|| panic!("missing source marker {end}"))
            + start;
        &source[start..finish]
    }

    fn required_label_keys(manifest: &NodeUiManifest) -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        for page in &manifest.pages {
            keys.insert(page.nav_label_key.clone());
            keys.insert(page.title_key.clone());
            for action in &page.actions {
                keys.insert(action.label_key.clone());
                for field in &action.fields {
                    keys.insert(field.label_key.clone());
                }
                if let Some(confirmation) = &action.confirmation {
                    keys.insert(confirmation.title_key.clone());
                    keys.insert(confirmation.consequence_key.clone());
                }
            }
        }
        keys
    }

    #[test]
    fn embedded_manifest_matches_the_canonical_manifest_exactly() {
        let canonical =
            serde_json::to_value(node_ui_manifest()).expect("serialize canonical manifest");
        assert_eq!(embedded_manifest(), canonical);
        assert_eq!(
            canonical["pages"].as_array().expect("manifest pages").len(),
            11
        );
    }

    #[test]
    fn every_manifest_page_action_field_and_confirmation_has_a_localized_label() {
        let manifest = node_ui_manifest();
        for locale in [Locale::ZhCn, Locale::En] {
            let labels = message_keys(locale);
            for key in required_label_keys(&manifest) {
                assert!(
                    labels.contains(key.as_str()),
                    "missing {locale:?} label key {key}"
                );
                assert!(
                    !t(locale, &key).trim().is_empty(),
                    "empty {locale:?} label for {key}"
                );
            }
        }
    }

    #[test]
    fn browser_error_text_covers_transport_and_problem_codes_in_every_locale() {
        for code in [
            "schema_version_mismatch",
            "invalid_request",
            "backend_rejected",
            "backend_unavailable",
            "contract_violation",
            "not_found",
            "conflict",
            "unavailable",
            "permission_denied",
            "cancelled",
            "unsupported",
            "internal",
        ] {
            let key = format!("error.{code}");
            for locale in [Locale::ZhCn, Locale::En] {
                assert!(
                    message_keys(locale).contains(key.as_str()),
                    "missing {locale:?} key {key}"
                );
                assert!(
                    !t(locale, &key).trim().is_empty(),
                    "empty {locale:?} error text for {key}"
                );
            }
        }
    }

    #[test]
    fn every_manifest_page_action_and_field_has_an_explicit_browser_binding() {
        let manifest = node_ui_manifest();
        let projection = source_between(NODE_UI_JS, "function projectPage", "return Object.freeze");
        let commands = source_between(
            NODE_UI_JS,
            "function commandForAction",
            "function commandEnvelope",
        );

        for page in &manifest.pages {
            assert!(
                projection.contains(&format!("case \"{}\":", page.id)),
                "missing browser projection for page {}",
                page.id
            );
            for action in &page.actions {
                assert!(
                    commands.contains(&format!("case \"{}\":", action.id)),
                    "missing command binding for action {}",
                    action.id
                );
                for field in &action.fields {
                    assert!(
                        commands.contains(&format!("values.{}", field.id))
                            || commands.contains(&format!("\"{}\"", field.id)),
                        "command binding for {} does not consume manifest field {}",
                        action.id,
                        field.id
                    );
                }
            }
        }
    }

    #[test]
    fn browser_surface_uses_the_single_primary_status() {
        assert!(NODE_MANIFEST_HTML.contains(r#"id="primary-status""#));
        assert!(NODE_MANIFEST_HTML.contains(r#"id="primary-detail""#));
        assert!(NODE_MANIFEST_HTML.contains("ui.primaryStatusFor(state.snapshot, state.locale)"));
        assert!(!NODE_MANIFEST_HTML.contains(r#"$("lifecycle")"#));
        assert!(NODE_UI_JS.contains("function primaryStatusFor(snapshot, locale)"));
        assert!(NODE_UI_JS.contains("unknown control health"));
        for status in NodePrimaryStatus::ALL {
            for locale in [Locale::ZhCn, Locale::En] {
                assert!(
                    message_keys(locale).contains(status.label_key()),
                    "missing {locale:?} primary status label: {}",
                    status.label_key()
                );
                assert!(
                    message_keys(locale).contains(status.detail_key()),
                    "missing {locale:?} primary status detail: {}",
                    status.detail_key()
                );
            }
        }
    }

    #[test]
    fn browser_surface_is_manifest_driven_and_fails_closed() {
        assert!(NODE_MANIFEST_HTML.contains("state.manifest.pages.map"));
        assert!(NODE_MANIFEST_HTML.contains("action.fields.map"));
        assert!(NODE_MANIFEST_HTML.contains("ui.executionBlockReason"));
        assert!(NODE_MANIFEST_HTML.contains("ui.validateSnapshotResponse"));
        assert!(NODE_MANIFEST_HTML.contains("ui.commandEnvelope"));
        assert!(NODE_MANIFEST_HTML.contains("function failClosed"));
        assert!(NODE_MANIFEST_HTML.contains("state.failedClosed"));
        assert!(!NODE_MANIFEST_HTML.contains("localStorage"));
        assert!(!NODE_MANIFEST_HTML.contains("Authorization"));
        assert!(!NODE_MANIFEST_HTML.contains("Bearer "));
        assert!(!NODE_MANIFEST_HTML.contains("token="));
        assert!(NODE_MANIFEST_HTML.contains("/api/v1/auth/passkey/authentication/start"));
        assert!(NODE_MANIFEST_HTML.contains("/api/v1/auth/passkey/authentication/finish"));
        assert!(NODE_MANIFEST_HTML.contains("/api/v1/auth/session"));
        assert!(NODE_MANIFEST_HTML.contains("/api/v1/auth/passkey/logout"));
        assert!(NODE_MANIFEST_HTML.contains(r#"credentials: "same-origin""#));
        assert!(NODE_MANIFEST_HTML.contains("/api/node/events?schema_version=1"));
        assert!(NODE_MANIFEST_HTML.contains("function expireSession(message)"));
        assert!(NODE_MANIFEST_HTML.contains(r#"id="language-preference""#));
        assert!(NODE_MANIFEST_HTML.contains("/api/v1/settings/language"));
        assert!(NODE_MANIFEST_HTML.contains("function loadLanguagePreference"));
        assert!(NODE_MANIFEST_HTML.contains("function changeLanguagePreference"));
        assert!(!NODE_MANIFEST_HTML.contains(r#"data-page="overview""#));
        assert!(NODE_MANIFEST_HTML.contains(r#"$("data-content").innerHTML"#));
        assert!(NODE_MANIFEST_HTML.contains(r#"$("actions-content").innerHTML"#));
        assert!(NODE_MANIFEST_HTML.contains("ui.primaryNavigation"));
        assert!(NODE_MANIFEST_HTML.contains("ui.setContentBusy"));
        assert!(NODE_MANIFEST_HTML.contains("ui.observeLayout"));
        assert!(NODE_MANIFEST_HTML.contains("ui.initLocalTabs"));
        assert!(!NODE_MANIFEST_HTML.contains(r#"$("content").innerHTML"#));
        assert!(NODE_UI_JS.contains("NODE_UI_MANIFEST_CONTRACT_BEGIN"));
        assert!(NODE_UI_JS.contains("function executionBlockReason"));
        assert!(NODE_UI_JS.contains("contract_violation"));
        assert!(NODE_UI_JS.contains("function projectPage"));
        assert!(NODE_UI_JS
            .contains(r#"field.validation === "port" || field.validation === "distinct_ports""#));
    }
}
