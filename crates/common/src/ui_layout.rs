//! Shared responsive application-layout contract.
//!
//! The native and browser shells use this module for breakpoints, stable
//! primary-navigation semantics, region order, detail-panel bounds, and loading
//! behavior. Rendering remains platform-specific, but the structural decisions
//! cannot drift between GPUI, WebUI, and the Android contract.

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
    Detail,
}

impl AppRegion {
    pub const ALL: [Self; 4] = [Self::Sidebar, Self::Topbar, Self::Content, Self::Detail];

    /// Stable DOM attribute value shared with the browser runtime.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Topbar => "topbar",
            Self::Content => "content",
            Self::Detail => "detail",
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
    SidebarWithDetail,
}

impl NavigationMode {
    pub const fn for_layout(layout: LayoutClass) -> Self {
        match layout {
            LayoutClass::Mobile => Self::BottomNavigation,
            LayoutClass::Compact => Self::MenuDrawer,
            LayoutClass::Desktop => Self::Sidebar,
            LayoutClass::Wide => Self::SidebarWithDetail,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::BottomNavigation => "bottom-navigation",
            Self::MenuDrawer => "menu-drawer",
            Self::Sidebar => "sidebar",
            Self::SidebarWithDetail => "sidebar-with-detail",
        }
    }
}

/// How the context/detail panel is presented at a given width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailPresentation {
    Hidden,
    Drawer,
    Persistent,
}

impl DetailPresentation {
    pub const fn for_layout(layout: LayoutClass) -> Self {
        match layout {
            LayoutClass::Mobile | LayoutClass::Compact => Self::Drawer,
            LayoutClass::Desktop => Self::Hidden,
            LayoutClass::Wide => Self::Persistent,
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
        11,
        "the first product release exposes exactly eleven manifest pages"
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

/// Stable local tabs for manifest-driven pages. Data is shown first; actions
/// remain reachable without changing the page or losing its selected network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSection {
    Data,
    Actions,
}

impl PageSection {
    pub const ALL: [Self; 2] = [Self::Data, Self::Actions];
    pub const DEFAULT: Self = Self::Data;

    pub const fn name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Actions => "actions",
        }
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Data => "page_tab.data",
            Self::Actions => "page_tab.actions",
        }
    }
}

/// Loading behavior shared by both surfaces.
pub const INITIAL_LOAD_USES_LAYOUT_MATCHING_SKELETON: bool = true;
pub const REFRESH_KEEPS_EXISTING_CONTENT: bool = true;
pub const DETAIL_LOAD_DOES_NOT_RESIZE_CONTENT: bool = true;

/// Returns every region that exists in the logical shell contract. Detail is
/// represented as a drawer below the wide breakpoint rather than being removed
/// from the information architecture.
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
    fn navigation_and_detail_modes_follow_the_contract() {
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
            NavigationMode::SidebarWithDetail
        );
        assert_eq!(
            DetailPresentation::for_layout(LayoutClass::Desktop),
            DetailPresentation::Hidden
        );
        assert_eq!(
            DetailPresentation::for_layout(LayoutClass::Wide),
            DetailPresentation::Persistent
        );
    }

    #[test]
    fn regions_have_one_stable_reading_order() {
        assert_eq!(
            app_regions().map(AppRegion::name),
            ["sidebar", "topbar", "content", "detail"]
        );
    }

    #[test]
    fn primary_navigation_is_exactly_five_modules_and_eleven_pages() {
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
        assert_eq!(unique.len(), 11);
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
    fn page_sections_keep_a_stable_default() {
        assert_eq!(PageSection::ALL.len(), 2);
        assert_eq!(PageSection::DEFAULT, PageSection::Data);
        assert_eq!(PageSection::Data.name(), "data");
        assert_eq!(PageSection::Actions.label_key(), "page_tab.actions");
    }

    #[test]
    fn geometry_uses_the_product_design_values() {
        assert_eq!(DIMENSIONS.sidebar_px, 232);
        assert_eq!(DIMENSIONS.topbar_px, 56);
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
