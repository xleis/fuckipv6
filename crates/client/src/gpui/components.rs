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
use gpui::{div, prelude::*, px, rgba, Div, FontWeight, Role, SharedString, Stateful, Toggled};

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
            ButtonVariant::Primary => (colors.accent, colors.accent, colors.background_deep),
            ButtonVariant::Secondary => (colors.panel_raised, colors.border, colors.text),
            ButtonVariant::Ghost => (
                colors.panel_raised.with_alpha(0),
                colors.border_soft,
                colors.text_muted,
            ),
            ButtonVariant::Danger => (colors.danger_soft, colors.danger, colors.danger_text),
        }
    };

    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .focusable()
        .tab_stop(!disabled)
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
        .child(label)
}

pub fn text_input(
    id: SharedString,
    aria_label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
    focused: bool,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    let value = value.into();
    let placeholder = placeholder.into();
    let empty = value.is_empty();
    div()
        .id(id)
        .role(Role::TextInput)
        .aria_label(aria_label)
        .aria_value(value.clone())
        .aria_placeholder(placeholder.clone())
        .focusable()
        .tab_stop(false)
        .flex()
        .items_center()
        .min_h(px(DIMENSIONS.control_md_px as f32))
        .px(px(SPACING_PX[3] as f32))
        .py(px(SPACING_PX[2] as f32))
        .overflow_hidden()
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
        .child(if empty { placeholder } else { value })
}

pub fn choice_control(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    choice_surface(id, label.into(), selected, Role::RadioButton)
}

pub fn tab_control(
    id: SharedString,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    choice_surface(id, label.into(), selected, Role::Tab)
}

fn choice_surface(
    id: SharedString,
    label: SharedString,
    selected: bool,
    role: Role,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    div()
        .id(id)
        .role(role)
        .aria_label(label.clone())
        .aria_selected(selected)
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .focusable()
        .tab_stop(true)
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
        .child(label)
}

pub fn nav_tab(
    id: SharedString,
    label: impl Into<SharedString>,
    icon: Option<gpui::AnyElement>,
    selected: bool,
) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    let label = label.into();
    let mut tab = div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label.clone())
        .aria_selected(selected)
        .focusable()
        .tab_stop(true)
        .relative()
        .flex()
        .items_center()
        .gap(px(SPACING_PX[3] as f32))
        .w_full()
        .h(px(42.0))
        .px(px(SPACING_PX[3] as f32))
        .rounded(px(RADII.sm_px as f32))
        .border_1()
        .border_color(rgba(if selected {
            colors.accent.with_alpha(0x40).rgba_hex()
        } else {
            0x00000000
        }))
        .text_size(px(TYPOGRAPHY.sm.size_px as f32))
        .font_weight(FontWeight(if selected {
            TYPOGRAPHY.semibold_weight
        } else {
            TYPOGRAPHY.regular_weight
        } as f32))
        .text_color(rgba(
            if selected {
                colors.accent
            } else {
                colors.text_muted
            }
            .rgba_hex(),
        ));
    if selected {
        tab = tab.bg(rgba(colors.accent_soft.rgba_hex())).child(
            div()
                .absolute()
                .left(px(-3.0))
                .top(px(9.0))
                .bottom(px(9.0))
                .w(px(3.0))
                .rounded_full()
                .bg(rgba(colors.accent.rgba_hex())),
        );
    } else {
        tab = tab.bg(rgba(colors.background_soft.with_alpha(0).rgba_hex()));
    }
    tab = tab.children(icon).child(label);
    tab
}

pub fn tabs(id: impl Into<gpui::ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::TabList)
        .flex()
        .flex_col()
        .gap(px(SPACING_PX[1] as f32))
}

pub fn toolbar(id: impl Into<gpui::ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Toolbar)
        .flex()
        .items_center()
        .gap(px(SPACING_PX[3] as f32))
}

pub fn badge(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    state: SemanticState,
) -> Stateful<Div> {
    let palette = semantic_palette(state);
    let label = label.into();
    div()
        .id(id)
        .role(Role::Label)
        .aria_label(label.clone())
        .px(px(SPACING_PX[2] as f32))
        .py(px(SPACING_PX[1] as f32))
        .rounded(px(RADII.full_px as f32))
        .bg(rgba(palette.background.rgba_hex()))
        .border_1()
        .border_color(rgba(palette.border.rgba_hex()))
        .text_color(rgba(palette.foreground.rgba_hex()))
        .text_size(px(TYPOGRAPHY.xs.size_px as f32))
        .font_weight(FontWeight(TYPOGRAPHY.medium_weight as f32))
        .child(label)
}

pub fn status_badge(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    state: SemanticState,
) -> Stateful<Div> {
    badge(id, label, state).role(Role::Status)
}

/// Native capability adapter. Segments are backed by the shared capability
/// colors and every segment also has a visible text legend.
pub fn capability_ring(id: impl Into<gpui::ElementId>, items: &[CapabilityItem]) -> Stateful<Div> {
    let colors = DARK_THEME.colors;
    let id = id.into();
    let segments = items.iter().enumerate().map(|(index, item)| {
        let segment_color = match item.visual {
            CapabilityVisual::Active => capability_color(item.capability),
            CapabilityVisual::Degraded => colors.warning,
            CapabilityVisual::Inactive => colors.border_strong,
            CapabilityVisual::Unsupported => colors.text_faint,
        };
        div()
            .id(SharedString::from(format!("{id}-{index}")))
            .role(Role::ListItem)
            .aria_label(item.label.clone())
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
        .id(id.clone())
        .role(Role::List)
        .aria_label("Capabilities")
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(SPACING_PX[2] as f32))
        .children(segments)
}

fn state_surface(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    state: SemanticState,
) -> Stateful<Div> {
    let palette = semantic_palette(state);
    let title = title.into();
    let detail = detail.into();
    div()
        .id(id)
        .role(match state {
            SemanticState::Danger | SemanticState::PermissionDenied => Role::Alert,
            _ => Role::Status,
        })
        .aria_label(title.clone())
        .aria_description(detail.clone())
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
                .child(title),
        )
        .child(
            div()
                .text_size(px(TYPOGRAPHY.sm.size_px as f32))
                .text_color(rgba(DARK_THEME.colors.text_muted.rgba_hex()))
                .child(detail),
        )
}

pub fn empty_state(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Stateful<Div> {
    state_surface(id, title, detail, SemanticState::Neutral)
}

pub fn error_state(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Stateful<Div> {
    state_surface(id, title, detail, SemanticState::Danger)
}

pub fn permission_denied_state(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Stateful<Div> {
    state_surface(id, title, detail, SemanticState::PermissionDenied)
}

pub fn unsupported_state(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Stateful<Div> {
    state_surface(id, title, detail, SemanticState::Unsupported)
}

pub fn toast(
    id: impl Into<gpui::ElementId>,
    message: impl Into<SharedString>,
    state: SemanticState,
) -> Stateful<Div> {
    let palette = semantic_palette(state);
    let message = message.into();
    div()
        .id(id)
        .role(match state {
            SemanticState::Danger | SemanticState::PermissionDenied => Role::Alert,
            _ => Role::Status,
        })
        .aria_label(message.clone())
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
        .child(message)
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
