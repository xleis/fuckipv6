//! Shared responsive application-layout contract.
//!
//! The native and browser shells use this module for breakpoints, stable
//! primary-navigation semantics, region order, the persistent action-panel
//! surface, object-detail presentation, and loading behavior. Rendering remains
//! platform-specific, but structural decisions cannot drift between GPUI,
//! WebUI, and the Android contract.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::design_tokens::DIMENSIONS;

/// Ordered regions of the complete application shell. The order is also the
/// accessibility reading and keyboard traversal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppRegion {
    Sidebar,
    Topbar,
    Content,
    ActionPanel,
}

impl AppRegion {
    pub const ALL: [Self; 4] = [
        Self::Sidebar,
        Self::Topbar,
        Self::Content,
        Self::ActionPanel,
    ];

    /// Stable DOM attribute value shared with the browser runtime.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Topbar => "topbar",
            Self::Content => "content",
            Self::ActionPanel => "action-panel",
        }
    }
}

/// Responsive layout classes. Boundaries are lower bounds and therefore use
/// `>=`; widths below the mobile breakpoint are mobile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutClass {
    Mobile,
    Compact,
    Desktop,
    Wide,
}

impl LayoutClass {
    pub const ALL: [Self; 4] = [Self::Mobile, Self::Compact, Self::Desktop, Self::Wide];

    pub const fn for_width(width_px: u16) -> Self {
        if width_px < DIMENSIONS.mobile_breakpoint_px {
            Self::Mobile
        } else if width_px < DIMENSIONS.navigation_breakpoint_px {
            Self::Compact
        } else if width_px < DIMENSIONS.detail_breakpoint_px {
            Self::Desktop
        } else {
            Self::Wide
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Mobile => "mobile",
            Self::Compact => "compact",
            Self::Desktop => "desktop",
            Self::Wide => "wide",
        }
    }
}

/// Navigation presentation selected by the responsive layout class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationMode {
    BottomNavigation,
    MenuDrawer,
    Sidebar,
}

impl NavigationMode {
    pub const fn for_layout(layout: LayoutClass) -> Self {
        match layout {
            LayoutClass::Mobile => Self::BottomNavigation,
            LayoutClass::Compact => Self::MenuDrawer,
            LayoutClass::Desktop | LayoutClass::Wide => Self::Sidebar,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::BottomNavigation => "bottom-navigation",
            Self::MenuDrawer => "menu-drawer",
            Self::Sidebar => "sidebar",
        }
    }
}

/// How the current page's persistent action surface is presented.
///
/// It is always part of the page structure: stacked below the content on
/// narrow layouts, narrower beside it in the desktop class, and at its full
/// shared width from the wide breakpoint onward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionPanelPresentation {
    Stacked,
    Compact,
    Full,
}

impl ActionPanelPresentation {
    pub const fn for_layout(layout: LayoutClass) -> Self {
        match layout {
            LayoutClass::Mobile | LayoutClass::Compact => Self::Stacked,
            LayoutClass::Desktop => Self::Compact,
            LayoutClass::Wide => Self::Full,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Stacked => "stacked",
            Self::Compact => "compact",
            Self::Full => "full",
        }
    }

    /// Returns `None` when the panel occupies the full inline size below the
    /// content rather than a separate desktop column.
    pub const fn width_px(self) -> Option<u16> {
        match self {
            Self::Stacked => None,
            Self::Compact => Some(DIMENSIONS.action_panel_compact_px),
            Self::Full => Some(DIMENSIONS.action_panel_default_px),
        }
    }
}

/// Presentation for object inspection explicitly opened from content.
///
/// Object details never form a fourth desktop column. At desktop widths they
/// replace the action-panel column; on narrow layouts they use a drawer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectDetailPresentation {
    Drawer,
    ActionPanelColumn,
}

impl ObjectDetailPresentation {
    pub const fn for_layout(layout: LayoutClass) -> Self {
        match layout {
            LayoutClass::Mobile | LayoutClass::Compact => Self::Drawer,
            LayoutClass::Desktop | LayoutClass::Wide => Self::ActionPanelColumn,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Drawer => "drawer",
            Self::ActionPanelColumn => "action-panel-column",
        }
    }
}

/// One top-level product module and the manifest-backed pages reached through
/// its local tabs. This shape is serialized in `assets/ui-navigation.json` and
/// consumed unchanged by GPUI, React, and the browser component runtime.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NavigationModule {
    pub id: String,
    pub label_key: String,
    pub default_page_id: String,
    pub page_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NavigationContract {
    schema_version: u32,
    modules: Vec<NavigationModule>,
}

const NAVIGATION_CONTRACT_JSON: &str = include_str!("../assets/ui-navigation.json");

static NAVIGATION_CONTRACT: LazyLock<NavigationContract> = LazyLock::new(|| {
    let contract: NavigationContract = serde_json::from_str(NAVIGATION_CONTRACT_JSON)
        .expect("embedded navigation contract must be valid JSON");
    assert_eq!(
        contract.schema_version, 1,
        "unsupported navigation contract schema"
    );
    assert_eq!(
        contract.modules.len(),
        5,
        "the product shell requires exactly five primary modules"
    );

    let mut module_ids = HashSet::new();
    let mut label_keys = HashSet::new();
    let mut page_ids = HashSet::new();
    for module in &contract.modules {
        assert!(
            module_ids.insert(module.id.as_str()),
            "duplicate navigation module id: {}",
            module.id
        );
        assert!(
            label_keys.insert(module.label_key.as_str()),
            "duplicate navigation label key: {}",
            module.label_key
        );
        assert!(
            !module.page_ids.is_empty(),
            "navigation module {} has no pages",
            module.id
        );
        assert!(
            module.page_ids.contains(&module.default_page_id),
            "navigation module {} default page {} is not in page_ids",
            module.id,
            module.default_page_id
        );
        for page_id in &module.page_ids {
            assert!(
                page_ids.insert(page_id.as_str()),
                "page {page_id} belongs to more than one primary module"
            );
        }
    }
    assert_eq!(
        page_ids.len(),
        12,
        "the first product release exposes exactly twelve manifest pages"
    );
    contract
});

/// Canonical serialized navigation contract served to browser adapters.
pub const fn navigation_contract_json() -> &'static str {
    NAVIGATION_CONTRACT_JSON
}

/// The five ordered product modules used by every application shell.
pub fn navigation_modules() -> &'static [NavigationModule] {
    &NAVIGATION_CONTRACT.modules
}

/// Find the primary module owning a manifest page.
pub fn navigation_module_for_page(page_id: &str) -> Option<&'static NavigationModule> {
    navigation_modules()
        .iter()
        .find(|module| module.page_ids.iter().any(|candidate| candidate == page_id))
}

/// Find one primary module by stable module id.
pub fn navigation_module(id: &str) -> Option<&'static NavigationModule> {
    navigation_modules().iter().find(|module| module.id == id)
}

/// Loading behavior shared by both surfaces.
pub const INITIAL_LOAD_USES_LAYOUT_MATCHING_SKELETON: bool = true;
pub const REFRESH_KEEPS_EXISTING_CONTENT: bool = true;
pub const ACTION_PANEL_LOAD_DOES_NOT_RESIZE_CONTENT: bool = true;
pub const OBJECT_DETAIL_DOES_NOT_CREATE_A_FOURTH_DESKTOP_COLUMN: bool = true;

/// Returns every base region in accessibility and keyboard traversal order.
/// ActionPanel is a first-class page region; object details are an alternate
/// presentation of that column, not an additional base region.
pub const fn app_regions() -> [AppRegion; 4] {
    AppRegion::ALL
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn breakpoint_boundaries_are_exact_and_non_overlapping() {
        assert_eq!(LayoutClass::for_width(0), LayoutClass::Mobile);
        assert_eq!(LayoutClass::for_width(599), LayoutClass::Mobile);
        assert_eq!(LayoutClass::for_width(600), LayoutClass::Compact);
        assert_eq!(LayoutClass::for_width(899), LayoutClass::Compact);
        assert_eq!(LayoutClass::for_width(900), LayoutClass::Desktop);
        assert_eq!(LayoutClass::for_width(1279), LayoutClass::Desktop);
        assert_eq!(LayoutClass::for_width(1280), LayoutClass::Wide);
        assert_eq!(LayoutClass::for_width(u16::MAX), LayoutClass::Wide);
    }

    #[test]
    fn navigation_and_action_panel_modes_follow_the_contract() {
        assert_eq!(
            NavigationMode::for_layout(LayoutClass::Mobile),
            NavigationMode::BottomNavigation
        );
        assert_eq!(
            NavigationMode::for_layout(LayoutClass::Compact),
            NavigationMode::MenuDrawer
        );
        assert_eq!(
            NavigationMode::for_layout(LayoutClass::Desktop),
            NavigationMode::Sidebar
        );
        assert_eq!(
            NavigationMode::for_layout(LayoutClass::Wide),
            NavigationMode::Sidebar
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(LayoutClass::Mobile),
            ActionPanelPresentation::Stacked
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(LayoutClass::Compact),
            ActionPanelPresentation::Stacked
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(LayoutClass::Desktop),
            ActionPanelPresentation::Compact
        );
        assert_eq!(
            ActionPanelPresentation::for_layout(LayoutClass::Wide),
            ActionPanelPresentation::Full
        );
        assert_eq!(ActionPanelPresentation::Stacked.width_px(), None);
        assert_eq!(
            ActionPanelPresentation::Compact.width_px(),
            Some(DIMENSIONS.action_panel_compact_px)
        );
        assert_eq!(
            ActionPanelPresentation::Full.width_px(),
            Some(DIMENSIONS.action_panel_default_px)
        );
        assert_eq!(
            ObjectDetailPresentation::for_layout(LayoutClass::Compact),
            ObjectDetailPresentation::Drawer
        );
        assert_eq!(
            ObjectDetailPresentation::for_layout(LayoutClass::Desktop),
            ObjectDetailPresentation::ActionPanelColumn
        );
    }

    #[test]
    fn regions_have_one_stable_reading_order() {
        assert_eq!(
            app_regions().map(AppRegion::name),
            ["sidebar", "topbar", "content", "action-panel"]
        );
    }

    #[test]
    fn primary_navigation_is_exactly_five_modules_and_twelve_pages() {
        let modules = navigation_modules();
        assert_eq!(
            modules
                .iter()
                .map(|module| module.id.as_str())
                .collect::<Vec<_>>(),
            ["overview", "nodes", "networks", "activity", "settings"]
        );
        assert_eq!(
            modules
                .iter()
                .flat_map(|module| module.page_ids.iter().map(String::as_str))
                .collect::<Vec<_>>(),
            [
                "overview",
                "peers",
                "node_runtime",
                "routing",
                "identity",
                "networks",
                "topology",
                "exit_node",
                "relay",
                "traffic",
                "logs",
                "settings",
            ]
        );
        let unique = modules
            .iter()
            .flat_map(|module| module.page_ids.iter())
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), 12);
    }

    #[test]
    fn every_page_has_exactly_one_module_and_default_destination() {
        for module in navigation_modules() {
            assert!(module.page_ids.contains(&module.default_page_id));
            assert_eq!(
                navigation_module_for_page(&module.default_page_id).map(|found| found.id.as_str()),
                Some(module.id.as_str())
            );
            for page_id in &module.page_ids {
                assert_eq!(
                    navigation_module_for_page(page_id).map(|found| found.id.as_str()),
                    Some(module.id.as_str())
                );
            }
        }
        assert!(navigation_module_for_page("missing").is_none());
        assert!(navigation_module("missing").is_none());
    }

    #[test]
    fn geometry_uses_the_product_design_values() {
        assert_eq!(DIMENSIONS.sidebar_px, 232);
        assert_eq!(DIMENSIONS.topbar_px, 56);
        assert_eq!(DIMENSIONS.action_panel_compact_px, 300);
        assert_eq!(DIMENSIONS.action_panel_default_px, 330);
        assert_eq!(DIMENSIONS.detail_default_px, 360);
        assert_eq!(DIMENSIONS.detail_min_px, 320);
        assert_eq!(DIMENSIONS.detail_max_px, 480);
        assert_eq!(DIMENSIONS.content_form_max_px, 960);
        assert_eq!(DIMENSIONS.window_min_width_px, 960);
        assert_eq!(DIMENSIONS.window_min_height_px, 640);
        const {
            assert!(DIMENSIONS.detail_min_px < DIMENSIONS.detail_default_px);
            assert!(DIMENSIONS.detail_default_px < DIMENSIONS.detail_max_px);
        }
    }

    #[test]
    fn browser_stylesheet_consumes_every_region_and_breakpoint() {
        let css = include_str!("../assets/ui-components.css");
        for region in AppRegion::ALL {
            assert!(
                css.contains(&format!("[data-ui-region=\"{}\"]", region.name())),
                "CSS is missing region {}",
                region.name()
            );
        }
        for width in [
            DIMENSIONS.mobile_breakpoint_px,
            DIMENSIONS.navigation_breakpoint_px,
            DIMENSIONS.detail_breakpoint_px,
        ] {
            assert!(
                css.contains(&format!("(min-width: {}px)", width)),
                "CSS is missing breakpoint {width}px"
            );
        }
        assert!(css.contains(".ui-skeleton"));
        assert!(css.contains(".ui-bottom-nav"));
        assert!(css.contains(".ui-action-panel"));
        assert!(css.contains(".ui-detail"));
        assert!(css.contains(".ui-local-tabs"));
    }

    #[test]
    fn browser_runtime_consumes_the_canonical_navigation_contract() {
        let js = include_str!("../assets/ui-components.js");
        assert!(js.contains("window.FuckIPv6Navigation"));
        assert!(js.contains("primaryNavigation: PRIMARY_NAVIGATION"));
        assert!(js.contains("maxWidth: 599"));
        assert!(js.contains("minWidth: 1280"));
    }
}
