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
    use crate::node::{node_ui_manifest, NodeUiManifest};
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

    fn embedded_labels() -> Value {
        marker_json(NODE_UI_JS, "/* LABELS_BEGIN */", "/* LABELS_END */")
    }

    fn embedded_error_text() -> Value {
        marker_json(NODE_UI_JS, "/* ERROR_TEXT_BEGIN */", "/* ERROR_TEXT_END */")
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
    fn every_manifest_page_action_field_and_confirmation_has_a_label() {
        let manifest = node_ui_manifest();
        let labels_value = embedded_labels();
        let labels = labels_value.as_object().expect("label object");
        for key in required_label_keys(&manifest) {
            assert!(
                labels
                    .get(&key)
                    .and_then(Value::as_str)
                    .is_some_and(|label| !label.is_empty()),
                "missing non-empty label for {key}"
            );
        }
    }

    #[test]
    fn browser_error_text_covers_transport_and_problem_codes() {
        let errors_value = embedded_error_text();
        let errors = errors_value.as_object().expect("error object");
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
            assert!(
                errors
                    .get(code)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty()),
                "missing error text for {code}"
            );
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
    fn browser_surface_is_manifest_driven_and_fails_closed() {
        assert!(NODE_MANIFEST_HTML.contains("state.manifest.pages.map"));
        assert!(NODE_MANIFEST_HTML.contains("action.fields.map"));
        assert!(NODE_MANIFEST_HTML.contains("ui.executionBlockReason"));
        assert!(NODE_MANIFEST_HTML.contains("ui.validateSnapshotResponse"));
        assert!(NODE_MANIFEST_HTML.contains("ui.commandEnvelope"));
        assert!(NODE_MANIFEST_HTML.contains("function failClosed"));
        assert!(NODE_MANIFEST_HTML.contains("state.failedClosed"));
        assert!(NODE_MANIFEST_HTML.contains(r#"headers.set("Authorization", "Bearer " + token())"#));
        assert!(NODE_MANIFEST_HTML.contains("/api/node/events?"));
        assert!(NODE_MANIFEST_HTML.contains("token="));
        assert!(!NODE_MANIFEST_HTML.contains(r#"data-page="overview""#));
        assert!(NODE_UI_JS.contains("NODE_UI_MANIFEST_CONTRACT_BEGIN"));
        assert!(NODE_UI_JS.contains("function executionBlockReason"));
        assert!(NODE_UI_JS.contains("contract_violation"));
        assert!(NODE_UI_JS.contains("function projectPage"));
        assert!(NODE_UI_JS
            .contains(r#"field.validation === "port" || field.validation === "distinct_ports""#));
    }
}
