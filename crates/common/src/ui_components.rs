//! Canonical component contract shared by WebUI and the native GPUI surface.
//!
//! Components are addressed by stable `data-component` names and root classes.
//! Browser behavior lives in `assets/ui-components.js`; presentation lives in
//! `assets/ui-components.css` and consumes only the semantic design tokens.

use crate::design_tokens::webui_css;

/// Stable component identifiers used by tests, browser code, and native
/// adapters. Adding a component requires updating this inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiComponent {
    Button,
    Input,
    Select,
    Checkbox,
    Radio,
    Tabs,
    Breadcrumb,
    Toolbar,
    Table,
    VirtualList,
    Badge,
    StatusBadge,
    CapabilityRing,
    Dialog,
    Menu,
    ContextMenu,
    Tooltip,
    Toast,
    EmptyState,
    ErrorState,
    PermissionDeniedState,
    UnsupportedState,
}

impl UiComponent {
    /// Stable name used for `data-component` and browser constructors.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Input => "input",
            Self::Select => "select",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::Tabs => "tabs",
            Self::Breadcrumb => "breadcrumb",
            Self::Toolbar => "toolbar",
            Self::Table => "table",
            Self::VirtualList => "virtual-list",
            Self::Badge => "badge",
            Self::StatusBadge => "status-badge",
            Self::CapabilityRing => "capability-ring",
            Self::Dialog => "dialog",
            Self::Menu => "menu",
            Self::ContextMenu => "context-menu",
            Self::Tooltip => "tooltip",
            Self::Toast => "toast",
            Self::EmptyState => "empty-state",
            Self::ErrorState => "error-state",
            Self::PermissionDeniedState => "permission-denied-state",
            Self::UnsupportedState => "unsupported-state",
        }
    }

    /// Root class (or classes) owned by this component.
    pub const fn root_class(self) -> &'static str {
        match self {
            Self::Button => "ui-button",
            Self::Input => "ui-input",
            Self::Select => "ui-select",
            Self::Checkbox => "ui-check",
            Self::Radio => "ui-radio",
            Self::Tabs => "ui-tabs",
            Self::Breadcrumb => "ui-breadcrumb",
            Self::Toolbar => "ui-toolbar",
            Self::Table => "ui-table-wrap",
            Self::VirtualList => "ui-virtual-list",
            Self::Badge => "ui-badge",
            Self::StatusBadge => "ui-status-badge",
            Self::CapabilityRing => "ui-capability-ring",
            Self::Dialog => "ui-dialog-backdrop",
            Self::Menu => "ui-menu",
            Self::ContextMenu => "ui-context-menu",
            Self::Tooltip => "ui-tooltip",
            Self::Toast => "ui-toast",
            Self::EmptyState => "ui-state ui-state-empty",
            Self::ErrorState => "ui-state ui-state-error",
            Self::PermissionDeniedState => "ui-state ui-state-permission-denied",
            Self::UnsupportedState => "ui-state ui-state-unsupported",
        }
    }

    /// Human-readable accessibility obligations. These are part of the
    /// contract rather than an optional implementation detail.
    pub const fn accessibility(self) -> &'static [&'static str] {
        match self {
            Self::Button => &["native-button", "visible-focus", "disabled-state"],
            Self::Input | Self::Select => {
                &["associated-label", "description", "error", "visible-focus"]
            }
            Self::Checkbox | Self::Radio => &["native-control", "group-label", "visible-focus"],
            Self::Tabs => &["tablist", "arrow-keys", "aria-selected", "tabpanel"],
            Self::Breadcrumb => &["navigation-label", "aria-current"],
            Self::Toolbar => &["toolbar-role", "arrow-keys", "accessible-name"],
            Self::Table => &["caption", "column-headers", "row-focus"],
            Self::VirtualList => &["listbox-role", "aria-setsize", "aria-posinset"],
            Self::Badge => &["text-not-color-only"],
            Self::StatusBadge => &["text-not-color-only", "aria-live-optional"],
            Self::CapabilityRing => &["legend", "text-not-color-only"],
            Self::Dialog => &[
                "dialog-role",
                "modal",
                "focus-trap",
                "restore-focus",
                "escape",
            ],
            Self::Menu | Self::ContextMenu => {
                &["menu-role", "menuitem-role", "arrow-keys", "escape"]
            }
            Self::Tooltip => &["describedby", "hover-and-focus", "escape"],
            Self::Toast => &["status-role", "polite-or-assertive", "dismiss"],
            Self::EmptyState => &["heading", "description", "optional-action"],
            Self::ErrorState => &["alert-role", "retry", "diagnostic-id"],
            Self::PermissionDeniedState => &["reason", "owner-action"],
            Self::UnsupportedState => &["reason", "alternative"],
        }
    }
}

/// Complete component inventory. Keep it exhaustive so a new requirement
/// cannot silently omit the browser and native implementations.
pub const ALL_COMPONENTS: [UiComponent; 22] = [
    UiComponent::Button,
    UiComponent::Input,
    UiComponent::Select,
    UiComponent::Checkbox,
    UiComponent::Radio,
    UiComponent::Tabs,
    UiComponent::Breadcrumb,
    UiComponent::Toolbar,
    UiComponent::Table,
    UiComponent::VirtualList,
    UiComponent::Badge,
    UiComponent::StatusBadge,
    UiComponent::CapabilityRing,
    UiComponent::Dialog,
    UiComponent::Menu,
    UiComponent::ContextMenu,
    UiComponent::Tooltip,
    UiComponent::Toast,
    UiComponent::EmptyState,
    UiComponent::ErrorState,
    UiComponent::PermissionDeniedState,
    UiComponent::UnsupportedState,
];

/// Visual and announcement semantics for status surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticState {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
    PermissionDenied,
    Unsupported,
}

impl SemanticState {
    pub const fn tone(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Danger | Self::PermissionDenied => "danger",
            Self::Unsupported => "warning",
        }
    }

    /// Stable message key. User-visible strings are supplied by the
    /// localization layer rather than baked into this contract.
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Neutral => "state.neutral",
            Self::Info => "state.info",
            Self::Success => "state.success",
            Self::Warning => "state.warning",
            Self::Danger => "state.error",
            Self::PermissionDenied => "state.permission_denied",
            Self::Unsupported => "state.unsupported",
        }
    }

    pub const fn aria_live(self) -> &'static str {
        match self {
            Self::Danger => "assertive",
            _ => "polite",
        }
    }
}

/// Entity capability represented by a segment in a capability ring. The
/// textual legend is mandatory; color alone is never the full meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Coordinator,
    Relay,
    Exit,
    File,
}

impl Capability {
    pub const ALL: [Self; 4] = [Self::Coordinator, Self::Relay, Self::Exit, Self::File];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Relay => "relay",
            Self::Exit => "exit",
            Self::File => "file",
        }
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Coordinator => "capability.coordinator",
            Self::Relay => "capability.relay",
            Self::Exit => "capability.exit",
            Self::File => "capability.file",
        }
    }
}

/// Browser component behavior served by both WebUI implementations.
pub fn webui_component_js() -> &'static str {
    include_str!("../assets/ui-components.js")
}

/// Full browser stylesheet: generated design tokens plus the shared component
/// layer. Existing page-specific CSS remains in `webui.css`.
pub fn complete_webui_css() -> &'static str {
    static CSS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSS.get_or_init(|| {
        format!(
            "{}\n{}",
            webui_css(),
            include_str!("../assets/ui-components.css")
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn inventory_covers_the_entire_w5_02_contract() {
        let names = ALL_COMPONENTS.map(UiComponent::name);
        for required in [
            "button",
            "input",
            "select",
            "checkbox",
            "radio",
            "tabs",
            "breadcrumb",
            "toolbar",
            "table",
            "virtual-list",
            "badge",
            "status-badge",
            "capability-ring",
            "dialog",
            "menu",
            "context-menu",
            "tooltip",
            "toast",
            "empty-state",
            "error-state",
            "permission-denied-state",
            "unsupported-state",
        ] {
            assert!(names.contains(&required), "missing component {required}");
        }
        assert_eq!(names.len(), names.iter().collect::<HashSet<_>>().len());
    }

    #[test]
    fn every_component_has_a_root_selector_and_accessibility_contract() {
        let css = complete_webui_css();
        for component in ALL_COMPONENTS {
            assert!(
                !component.accessibility().is_empty(),
                "{} has no accessibility contract",
                component.name()
            );
            let selector = format!(
                ".{}",
                component.root_class().split_whitespace().next().unwrap()
            );
            assert!(
                css.contains(&selector),
                "component {} is missing root selector {selector}",
                component.name()
            );
            assert!(
                css.contains(&format!("[data-component=\"{}\"]", component.name())),
                "component {} is missing its stable data contract",
                component.name()
            );
        }
    }

    #[test]
    fn semantic_states_are_textual_and_map_to_shared_tone_variables() {
        for state in [
            SemanticState::Neutral,
            SemanticState::Info,
            SemanticState::Success,
            SemanticState::Warning,
            SemanticState::Danger,
            SemanticState::PermissionDenied,
            SemanticState::Unsupported,
        ] {
            assert!(state.label_key().starts_with("state."));
            assert!(!state.tone().is_empty());
        }
        assert_eq!(SemanticState::Danger.aria_live(), "assertive");
        assert_eq!(SemanticState::PermissionDenied.tone(), "danger");
        assert_eq!(SemanticState::Unsupported.tone(), "warning");
    }

    #[test]
    fn every_capability_has_a_distinct_name_and_translation_key() {
        let names = Capability::ALL.map(Capability::name);
        let keys = Capability::ALL.map(Capability::label_key);
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), names.len());
        assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
    }

    #[test]
    fn browser_runtime_exposes_every_component_constructor() {
        let js = webui_component_js();
        for constructor in [
            "button:",
            "input:",
            "select:",
            "checkbox:",
            "radio:",
            "tabs:",
            "breadcrumb:",
            "toolbar:",
            "table:",
            "virtualList:",
            "badge:",
            "statusBadge:",
            "capabilityRing:",
            "dialog:",
            "menu:",
            "contextMenu:",
            "tooltip:",
            "toast:",
            "emptyState:",
            "errorState:",
            "permissionDeniedState:",
            "unsupportedState:",
        ] {
            assert!(
                js.contains(constructor),
                "missing browser constructor {constructor}"
            );
        }
        assert!(js.contains("textContent"));
        assert!(js.contains("aria"));
    }

    #[test]
    fn browser_runtime_uses_inventory_component_names() {
        let js = webui_component_js();
        assert!(js.contains("const COMPONENT_NAMES = new Set(Object.values(COMPONENTS));"));
        assert!(js.contains("if (!COMPONENT_NAMES.has(name))"));
        assert!(
            !js.contains("COMPONENTS[\""),
            "constructors must use inventory keys, not hyphenated DOM names"
        );
        for state_constructor in [
            "COMPONENTS.emptyState",
            "COMPONENTS.errorState",
            "COMPONENTS.permissionDeniedState",
            "COMPONENTS.unsupportedState",
        ] {
            assert!(
                js.contains(state_constructor),
                "state constructor must use {state_constructor}"
            );
        }
    }

    #[test]
    fn browser_runtime_never_interprets_user_text_as_html() {
        let js = webui_component_js();
        for unsafe_api in [".innerHTML", "insertAdjacentHTML", "outerHTML"] {
            assert!(
                !js.contains(unsafe_api),
                "user-provided text must not pass through {unsafe_api}"
            );
        }
    }
}
