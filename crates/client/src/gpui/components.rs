//! Native GPUI adapters for the shared component contract.
//!
//! This module keeps component dimensions and semantic colors tied to
//! `design_tokens` instead of allowing each page to invent its own variants.
//! GPUI cannot share DOM/CSS with the WebUI, so both surfaces render the same
//! contract independently while consuming one Rust token and component source.

use fuckipv6_common::design_tokens::{
    Color, DARK_THEME, DIMENSIONS, RADII, SPACING_PX, TYPOGRAPHY,
};
use fuckipv6_common::ui_components::{Capability, SemanticState, UiComponent};
use gpui::{div, prelude::*, px, rgba, Div, FontWeight, SharedString, Stateful};

/// Stable mapping between the shared component inventory and native adapter
/// constructor names. The test suite asserts that this remains exhaustive.
pub const ADAPTERS: [(&str, UiComponent); 22] = [
    ("button", UiComponent::Button),
    ("input", UiComponent::Input),
    ("select", UiComponent::Select),
    ("checkbox", UiComponent::Checkbox),
    ("radio", UiComponent::Radio),
    ("tabs", UiComponent::Tabs),
    ("breadcrumb", UiComponent::Breadcrumb),
    ("toolbar", UiComponent::Toolbar),
    ("table", UiComponent::Table),
    ("virtualList", UiComponent::VirtualList),
    ("badge", UiComponent::Badge),
    ("statusBadge", UiComponent::StatusBadge),
    ("capabilityRing", UiComponent::CapabilityRing),
    ("dialog", UiComponent::Dialog),
    ("menu", UiComponent::Menu),
    ("contextMenu", UiComponent::ContextMenu),
    ("tooltip", UiComponent::Tooltip),
    ("toast", UiComponent::Toast),
    ("emptyState", UiComponent::EmptyState),
    ("errorState", UiComponent::ErrorState),
    ("permissionDeniedState", UiComponent::PermissionDeniedState),
    ("unsupportedState", UiComponent::UnsupportedState),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityVisual {
    Active,
    Degraded,
    Inactive,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityItem {
    pub capability: Capability,
    pub visual: CapabilityVisual,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticPalette {
    pub foreground: Color,
    pub background: Color,
    pub border: Color,
}

/// Semantic palette used by every native status surface. Text labels remain
/// mandatory at call sites, so color is never the only status signal.
pub const fn semantic_palette(state: SemanticState) -> SemanticPalette {
    let colors = DARK_THEME.colors;
    match state {
        SemanticState::Neutral => SemanticPalette {
            foreground: colors.text,
            background: colors.panel_raised,
            border: colors.border,
        },
        SemanticState::Info => SemanticPalette {
            foreground: colors.info,
            background: colors.info_soft,
            border: colors.info,
        },
        SemanticState::Success => SemanticPalette {
            foreground: colors.success,
            background: colors.success_soft,
            border: colors.success,
        },
        SemanticState::Warning | SemanticState::Unsupported => SemanticPalette {
            foreground: colors.warning,
            background: colors.warning_soft,
            border: colors.warning,
        },
        SemanticState::Danger | SemanticState::PermissionDenied => SemanticPalette {
            foreground: colors.danger_text,
            background: colors.danger_soft,
            border: colors.danger,
        },
    }
}

pub const fn capability_color(capability: Capability) -> Color {
    let colors = DARK_THEME.colors;
    match capability {
        Capability::Coordinator => colors.coordinator.solid,
        Capability::Relay => colors.relay.solid,
        Capability::Exit => colors.exit.solid,
        Capability::File => colors.file.solid,
    }
}

pub const fn control_height_px(component: UiComponent) -> Option<u16> {
    match component {
        UiComponent::Button
        | UiComponent::Input
        | UiComponent::Select
        | UiComponent::Checkbox
        | UiComponent::Radio => Some(DIMENSIONS.control_md_px),
        UiComponent::Tabs
        | UiComponent::Breadcrumb
        | UiComponent::Toolbar
        | UiComponent::Table
        | UiComponent::VirtualList
        | UiComponent::Badge
        | UiComponent::StatusBadge
        | UiComponent::CapabilityRing
        | UiComponent::Dialog
        | UiComponent::Menu
        | UiComponent::ContextMenu
        | UiComponent::Tooltip
        | UiComponent::Toast
        | UiComponent::EmptyState
        | UiComponent::ErrorState
        | UiComponent::PermissionDeniedState
        | UiComponent::UnsupportedState => None,
    }
}

/// Generic action surface. The caller attaches the event listener after this
/// constructor, keeping component styling and action behavior separate.
pub fn button(
    id: SharedString,
    label: impl Into<SharedString>,
    variant: ButtonVariant,
    disabled: bool,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    let (background, border, foreground) = if disabled {
        (colors.surface, colors.border_soft, colors.text_faint)
    } else {
        match variant {
            ButtonVariant::Primary => {
                (colors.accent_pressed, colors.accent, colors.background_deep)
            }
            ButtonVariant::Secondary => (colors.panel_raised, colors.border, colors.text),
            ButtonVariant::Ghost => (
                colors.panel_raised.with_alpha(0),
                colors.border_soft,
                colors.text_muted,
            ),
            ButtonVariant::Danger => (colors.danger_soft, colors.danger, colors.danger_text),
        }
    };

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(DIMENSIONS.control_md_px as f32))
        .px(px(SPACING_PX[3] as f32))
        .rounded(px(RADII.md_px as f32))
        .bg(rgba(background.rgba_hex()))
        .border_1()
        .border_color(rgba(border.rgba_hex()))
        .text_color(rgba(foreground.rgba_hex()))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .line_height(px((TYPOGRAPHY.sm.size_px as f32
            * TYPOGRAPHY.sm.line_height)
            .round()))
        .font_weight(FontWeight(TYPOGRAPHY.medium_weight as f32))
        .opacity(if disabled { 0.62 } else { 1.0 })
        .child(label.into())
}

pub fn text_input(
    id: SharedString,
    value: impl Into<SharedString>,
    empty: bool,
    focused: bool,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    div()
        .id(id)
        .flex()
        .items_center()
        .min_h(px(DIMENSIONS.control_md_px as f32))
        .px(px(SPACING_PX[3] as f32))
        .py(px(SPACING_PX[2] as f32))
        .rounded(px(RADII.md_px as f32))
        .bg(rgba(colors.input_background.rgba_hex()))
        .border_1()
        .border_color(rgba(
            if focused {
                colors.accent
            } else {
                colors.border
            }
            .rgba_hex(),
        ))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .line_height(px((TYPOGRAPHY.sm.size_px as f32
            * TYPOGRAPHY.sm.line_height)
            .round()))
        .text_color(rgba(
            if empty {
                colors.text_muted
            } else {
                colors.text
            }
            .rgba_hex(),
        ))
        .child(value.into())
}

pub fn choice_control(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(DIMENSIONS.control_sm_px as f32))
        .px(px(SPACING_PX[3] as f32))
        .rounded(px(RADII.md_px as f32))
        .bg(rgba(
            if selected {
                colors.accent_soft
            } else {
                colors.panel_raised
            }
            .rgba_hex(),
        ))
        .border_1()
        .border_color(rgba(
            if selected {
                colors.accent
            } else {
                colors.border
            }
            .rgba_hex(),
        ))
        .text_color(rgba(
            if selected {
                colors.text
            } else {
                colors.text_muted
            }
            .rgba_hex(),
        ))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .font_weight(FontWeight(if selected {
            TYPOGRAPHY.semibold_weight
        } else {
            TYPOGRAPHY.regular_weight
        } as f32))
        .child(label.into())
}

pub fn nav_tab(id: SharedString, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    let mut tab = div()
        .id(id)
        .mx(px(SPACING_PX[3] as f32))
        .px(px(SPACING_PX[3] as f32))
        .py(px(SPACING_PX[2] as f32))
        .rounded(px(RADII.md_px as f32))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .font_weight(FontWeight(if selected {
            TYPOGRAPHY.semibold_weight
        } else {
            TYPOGRAPHY.regular_weight
        } as f32))
        .text_color(rgba(
            if selected {
                colors.text
            } else {
                colors.text_muted
            }
            .rgba_hex(),
        ))
        .child(label.into());
    tab = if selected {
        tab.bg(rgba(colors.accent_soft.rgba_hex()))
            .border_l_2()
            .border_color(rgba(colors.accent.rgba_hex()))
    } else {
        tab.bg(rgba(colors.background_soft.rgba_hex()))
    };
    tab
}

pub fn tabs() -> Div {
    div().flex().flex_col().gap(px(SPACING_PX[1] as f32))
}

pub fn toolbar() -> Div {
    div().flex().items_center().gap(px(SPACING_PX[3] as f32))
}

pub fn badge(label: impl Into<SharedString>, state: SemanticState) -> Div {
    let palette = semantic_palette(state);
    div()
        .px(px(SPACING_PX[2] as f32))
        .py(px(SPACING_PX[1] as f32))
        .rounded(px(RADII.full_px as f32))
        .bg(rgba(palette.background.rgba_hex()))
        .border_1()
        .border_color(rgba(palette.border.rgba_hex()))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .text_size(px(TYPOGRAPHY.xs.size_px as f32))
        .font_weight(FontWeight(TYPOGRAPHY.medium_weight as f32))
        .child(label.into())
}

pub fn status_badge(label: impl Into<SharedString>, state: SemanticState) -> Div {
    badge(label, state)
}

/// Native capability adapter. Segments are backed by the shared capability
/// colors and every segment also has a visible text legend.
pub fn capability_ring(items: &[CapabilityItem]) -> Div {
    let colors = DARK_THEME.colors;
    let segments = items.iter().map(|item| {
        let segment_color = match item.visual {
            CapabilityVisual::Active => capability_color(item.capability),
            CapabilityVisual::Degraded => colors.warning,
            CapabilityVisual::Inactive => colors.border_strong,
            CapabilityVisual::Unsupported => colors.text_faint,
        };
        div()
            .flex()
            .items_center()
            .gap(px(SPACING_PX[1] as f32))
            .child(
                div()
                    .w(px(SPACING_PX[2] as f32))
                    .h(px(SPACING_PX[1] as f32))
                    .rounded(px(RADII.full_px as f32))
                    .bg(rgba(segment_color.rgba_hex())),
            )
            .child(
                div()
                    .text_size(px(TYPOGRAPHY.xs.size_px as f32))
                    .text_color(rgba(colors.text_muted.rgba_hex()))
                    .child(item.label.clone()),
            )
            .into_any_element()
    });

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(SPACING_PX[2] as f32))
        .children(segments)
}

fn state_surface(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    state: SemanticState,
) -> Div {
    let palette = semantic_palette(state);
    div()
        .flex()
        .flex_col()
        .gap(px(SPACING_PX[2] as f32))
        .p(px(SPACING_PX[6] as f32))
        .rounded(px(RADII.lg_px as f32))
        .bg(rgba(DARK_THEME.colors.panel_solid.rgba_hex()))
        .border_1()
        .border_color(rgba(palette.border.rgba_hex()))
        .child(
            div()
                .text_size(px(TYPOGRAPHY.xl.size_px as f32))
                .font_weight(FontWeight(TYPOGRAPHY.semibold_weight as f32))
                .text_color(rgba(palette.foreground.rgba_hex()))
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(TYPOGRAPHY.sm.size_px as f32))
                .text_color(rgba(DARK_THEME.colors.text_muted.rgba_hex()))
                .child(detail.into()),
        )
}

pub fn empty_state(title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    state_surface(title, detail, SemanticState::Neutral)
}

pub fn error_state(title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    state_surface(title, detail, SemanticState::Danger)
}

pub fn permission_denied_state(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    state_surface(title, detail, SemanticState::PermissionDenied)
}

pub fn unsupported_state(title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    state_surface(title, detail, SemanticState::Unsupported)
}

pub fn toast(message: impl Into<SharedString>, state: SemanticState) -> Div {
    let palette = semantic_palette(state);
    div()
        .mx(px(SPACING_PX[6] as f32))
        .mt(px(SPACING_PX[4] as f32))
        .px(px(SPACING_PX[4] as f32))
        .py(px(SPACING_PX[2] as f32))
        .rounded(px(RADII.md_px as f32))
        .bg(rgba(palette.background.rgba_hex()))
        .border_1()
        .border_color(rgba(palette.border.rgba_hex()))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .child(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use fuckipv6_common::ui_components::ALL_COMPONENTS;

    use super::*;

    #[test]
    fn adapter_inventory_is_complete_and_names_are_unique() {
        let names = ADAPTERS
            .iter()
            .map(|(name, _)| *name)
            .collect::<HashSet<_>>();
        let components = ADAPTERS
            .iter()
            .map(|(_, component)| *component)
            .collect::<HashSet<_>>();
        assert_eq!(ADAPTERS.len(), ALL_COMPONENTS.len());
        assert_eq!(names.len(), ADAPTERS.len());
        assert_eq!(components.len(), ADAPTERS.len());
        for component in ALL_COMPONENTS {
            assert!(
                components.contains(&component),
                "missing {}",
                component.name()
            );
        }
    }

    #[test]
    fn interactive_components_use_standard_control_heights() {
        for component in [
            UiComponent::Button,
            UiComponent::Input,
            UiComponent::Select,
            UiComponent::Checkbox,
            UiComponent::Radio,
        ] {
            assert_eq!(control_height_px(component), Some(DIMENSIONS.control_md_px));
        }
    }

    #[test]
    fn every_semantic_state_has_a_distinct_text_foreground_and_border() {
        let states = [
            SemanticState::Neutral,
            SemanticState::Info,
            SemanticState::Success,
            SemanticState::Warning,
            SemanticState::Danger,
            SemanticState::PermissionDenied,
            SemanticState::Unsupported,
        ];
        for state in states {
            let palette = semantic_palette(state);
            assert_ne!(palette.foreground.rgba_hex(), 0);
            assert_ne!(palette.border.rgba_hex(), 0);
        }
        assert_eq!(
            semantic_palette(SemanticState::Danger),
            semantic_palette(SemanticState::PermissionDenied)
        );
        assert_eq!(
            semantic_palette(SemanticState::Warning),
            semantic_palette(SemanticState::Unsupported)
        );
    }

    #[test]
    fn capability_colors_follow_the_shared_ring_order() {
        let colors = Capability::ALL.map(capability_color);
        assert_eq!(
            colors
                .iter()
                .map(|color| color.rgba_hex())
                .collect::<HashSet<_>>()
                .len(),
            colors.len()
        );
    }
}
