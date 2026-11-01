//! Embedded React browser surface for the shared node HTTP contract.
//!
//! The browser is a renderer and input adapter only. It fetches the canonical
//! `NodeUiManifest` and `NodeSnapshot`, and submits versioned
//! `NodeCommandHttpRequest` DTOs through the same adapter used by GPUI and
//! programmatic clients.

pub(super) const NODE_APP_HTML: &str = include_str!("dist/index.html");
pub(super) const NODE_APP_JS: &str = include_str!("dist/assets/app.js");
pub(super) const NODE_APP_CSS: &str = include_str!("dist/assets/app.css");

#[cfg(test)]
mod tests {
    use super::{NODE_APP_CSS, NODE_APP_HTML, NODE_APP_JS};
    use crate::node::{node_ui_manifest, NodePrimaryStatus, NodeUiManifest};
    use fuckipv6_common::i18n::{message_keys, t, Locale};
    use serde_json::Value;
    use std::collections::BTreeSet;

    const NODE_MANIFEST_FIXTURE: &str =
        include_str!("../../../../contracts/typescript/fixtures/node-manifest.json");

    fn fixture_manifest() -> Value {
        serde_json::from_str(NODE_MANIFEST_FIXTURE).expect("parse stable node manifest fixture")
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
    fn stable_manifest_fixture_matches_the_canonical_manifest_exactly() {
        let canonical =
            serde_json::to_value(node_ui_manifest()).expect("serialize canonical manifest");
        assert_eq!(fixture_manifest(), canonical);
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
    fn primary_status_labels_exist_for_every_state_and_locale() {
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
    fn embedded_react_shell_is_the_only_product_surface() {
        assert!(NODE_APP_HTML.contains(r#"<div id="root"></div>"#));
        assert!(NODE_APP_HTML.contains("/assets/app.js"));
        assert!(NODE_APP_HTML.contains("/assets/app.css"));
        assert!(NODE_APP_HTML.contains("/assets/i18n.js"));
        assert!(!NODE_APP_HTML.contains("node_ui.js"));
        assert!(!NODE_APP_HTML.contains("ui-components.js"));
        assert!(!NODE_APP_HTML.contains("<script>"));
        assert!(!NODE_APP_HTML.contains("<style"));
    }

    #[test]
    fn embedded_app_assets_are_present_and_do_not_embed_control_tokens() {
        assert!(
            NODE_APP_JS.len() > 100_000,
            "React application bundle is unexpectedly small"
        );
        assert!(
            NODE_APP_CSS.len() > 10_000,
            "React application stylesheet is unexpectedly small"
        );
        for asset in [NODE_APP_HTML, NODE_APP_JS, NODE_APP_CSS] {
            assert!(!asset.contains("fuckipv6_control_token"));
            assert!(!asset.contains("Authorization"));
            assert!(!asset.contains("Bearer "));
            assert!(!asset.contains("?token="));
        }
    }
}
