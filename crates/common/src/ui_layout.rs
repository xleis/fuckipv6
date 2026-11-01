//! Shared responsive application-layout contract.
//!
//! The native and browser shells use this module for breakpoints, stable
//! primary-navigation semantics, region order, detail-panel bounds, and loading
//! behavior. Rendering remains platform-specific, but the structural decisions
//! cannot drift between GPUI, WebUI, and the Android contract.

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

/// One stable top-level navigation destination. The first version has exactly
/// five primary destinations; deeper node pages remain reachable from the full
/// navigation drawer until W6 installs the final information architecture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimaryNavItem {
    pub id: &'static str,
    pub label_key: &'static str,
    pub page_id: &'static str,
}

pub const PRIMARY_NAVIGATION: [PrimaryNavItem; 5] = [
    PrimaryNavItem {
        id: "overview",
        label_key: "nav.overview",
        page_id: "overview",
    },
    PrimaryNavItem {
        id: "nodes",
        label_key: "nav.nodes",
        page_id: "peers",
    },
    PrimaryNavItem {
        id: "networks",
        label_key: "nav.networks",
        page_id: "networks",
    },
    PrimaryNavItem {
        id: "activity",
        label_key: "nav.activity",
        page_id: "logs",
    },
    PrimaryNavItem {
        id: "settings",
        label_key: "nav.settings",
        page_id: "settings",
    },
];

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
    fn primary_navigation_is_exactly_five_unique_destinations() {
        let ids = PRIMARY_NAVIGATION.map(|item| item.id);
        let keys = PRIMARY_NAVIGATION.map(|item| item.label_key);
        let pages = PRIMARY_NAVIGATION.map(|item| item.page_id);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
        assert_eq!(pages.iter().collect::<HashSet<_>>().len(), pages.len());
        assert_eq!(
            ids,
            ["overview", "nodes", "networks", "activity", "settings"]
        );
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
    fn browser_runtime_exposes_the_same_primary_navigation() {
        let js = include_str!("../assets/ui-components.js");
        for item in PRIMARY_NAVIGATION {
            assert!(
                js.contains(&format!("id: \"{}\"", item.id)),
                "runtime is missing navigation item {}",
                item.id
            );
            assert!(
                js.contains(&format!("pageId: \"{}\"", item.page_id)),
                "runtime is missing destination {}",
                item.page_id
            );
        }
        assert!(js.contains("maxWidth: 599"));
        assert!(js.contains("minWidth: 1280"));
    }
}
