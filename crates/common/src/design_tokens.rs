//! Transport-neutral design tokens shared by the native and browser surfaces.
//!
//! The Rust values in this module are the single source of truth. WebUI CSS is
//! generated from them, while GPUI converts the same semantic colors into its
//! own color type. This keeps both surfaces from silently drifting apart.

use std::sync::OnceLock;

/// A semantic color with 8-bit straight-alpha channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Creates an opaque color from an `0xRRGGBB` value.
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
            a: 0xff,
        }
    }

    /// Creates a color from an `0xRRGGBBAA` value.
    pub const fn rgba(hex: u32) -> Self {
        Self {
            r: ((hex >> 24) & 0xff) as u8,
            g: ((hex >> 16) & 0xff) as u8,
            b: ((hex >> 8) & 0xff) as u8,
            a: (hex & 0xff) as u8,
        }
    }

    /// Replaces the alpha channel while preserving RGB.
    pub const fn with_alpha(self, alpha: u8) -> Self {
        Self { a: alpha, ..self }
    }

    /// Returns the opaque `0xRRGGBB` representation, dropping alpha.
    pub const fn rgb_hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Returns the `0xRRGGBBAA` representation expected by GPUI's `rgba`.
    pub const fn rgba_hex(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | self.a as u32
    }

    /// Composites this color over an opaque background.
    pub const fn composite_over(self, background: Self) -> Self {
        let alpha = self.a as u32;
        let inverse = 255 - alpha;
        Self {
            r: (((self.r as u32 * alpha) + (background.r as u32 * inverse) + 127) / 255) as u8,
            g: (((self.g as u32 * alpha) + (background.g as u32 * inverse) + 127) / 255) as u8,
            b: (((self.b as u32 * alpha) + (background.b as u32 * inverse) + 127) / 255) as u8,
            a: 0xff,
        }
    }

    /// Formats the color as a CSS color value.
    pub fn css(self) -> String {
        if self.a == 0xff {
            return format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b);
        }
        let mut alpha = format!("{:.3}", self.a as f64 / 255.0);
        while alpha.ends_with('0') {
            alpha.pop();
        }
        if alpha.ends_with('.') {
            alpha.pop();
        }
        format!("rgba({}, {}, {}, {})", self.r, self.g, self.b, alpha)
    }
}

/// Color pair used to render an entity capability without making color the
/// only source of meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityColor {
    pub solid: Color,
    pub soft: Color,
}

/// Semantic color roles used by both UI implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    pub background: Color,
    pub background_soft: Color,
    pub background_deep: Color,
    pub panel: Color,
    pub panel_solid: Color,
    pub panel_raised: Color,
    pub panel_inset: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub input_background: Color,
    pub border: Color,
    pub border_soft: Color,
    pub border_strong: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub accent: Color,
    pub accent_secondary: Color,
    pub accent_soft: Color,
    pub accent_glow: Color,
    pub accent_pressed: Color,
    pub success: Color,
    pub success_soft: Color,
    pub success_glow: Color,
    pub warning: Color,
    pub warning_soft: Color,
    pub danger: Color,
    pub danger_soft: Color,
    pub danger_text: Color,
    pub info: Color,
    pub info_soft: Color,
    pub purple: Color,
    pub purple_soft: Color,
    pub coordinator: CapabilityColor,
    pub relay: CapabilityColor,
    pub exit: CapabilityColor,
    pub file: CapabilityColor,
}

/// Elevation values are CSS expressions because shadows are specific to the
/// browser rendering model. GPUI may consume the same palette while rendering
/// native elevation in a later adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowTokens {
    pub panel: &'static str,
    pub popover: &'static str,
    pub accent: &'static str,
}

/// A complete color theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeTokens {
    pub dark: bool,
    pub colors: ThemeColors,
    pub shadows: ShadowTokens,
}

pub const DARK_THEME: ThemeTokens = ThemeTokens {
    dark: true,
    colors: ThemeColors {
        background: Color::rgb(0x070b12),
        background_soft: Color::rgb(0x0a101a),
        background_deep: Color::rgb(0x050810),
        panel: Color::rgba(0x111a29c7),
        panel_solid: Color::rgb(0x101827),
        panel_raised: Color::rgba(0x182335d1),
        panel_inset: Color::rgba(0x060b138c),
        surface: Color::rgba(0x94a3b80f),
        surface_hover: Color::rgba(0x94a3b81c),
        input_background: Color::rgba(0x060b139e),
        border: Color::rgba(0x94a3b824),
        border_soft: Color::rgba(0x94a3b814),
        border_strong: Color::rgba(0x94a3b842),
        text: Color::rgb(0xe9eef7),
        text_secondary: Color::rgb(0xa4b4ca),
        text_muted: Color::rgb(0x6c7e96),
        text_faint: Color::rgb(0x4b5b71),
        accent: Color::rgb(0x38bdf8),
        accent_secondary: Color::rgb(0x818cf8),
        accent_soft: Color::rgba(0x38bdf81f),
        accent_glow: Color::rgba(0x38bdf859),
        accent_pressed: Color::rgb(0x0284c7),
        success: Color::rgb(0x34d399),
        success_soft: Color::rgba(0x34d3991f),
        success_glow: Color::rgba(0x34d39959),
        warning: Color::rgb(0xfbbf24),
        warning_soft: Color::rgba(0xfbbf2421),
        danger: Color::rgb(0xfb7185),
        danger_soft: Color::rgba(0xfb71851f),
        danger_text: Color::rgb(0xffb4bf),
        info: Color::rgb(0x60a5fa),
        info_soft: Color::rgba(0x60a5fa1f),
        purple: Color::rgb(0xa78bfa),
        purple_soft: Color::rgba(0xa78bfa1f),
        coordinator: CapabilityColor {
            solid: Color::rgb(0x38bdf8),
            soft: Color::rgba(0x38bdf81f),
        },
        relay: CapabilityColor {
            solid: Color::rgb(0xa78bfa),
            soft: Color::rgba(0xa78bfa1f),
        },
        exit: CapabilityColor {
            solid: Color::rgb(0x34d399),
            soft: Color::rgba(0x34d3991f),
        },
        file: CapabilityColor {
            solid: Color::rgb(0xfbbf24),
            soft: Color::rgba(0xfbbf241f),
        },
    },
    shadows: ShadowTokens {
        panel: "0 30px 70px -40px rgba(0, 0, 0, 0.95), 0 1px 0 rgba(255, 255, 255, 0.03) inset",
        popover: "0 30px 80px -20px rgba(0, 0, 0, 0.85)",
        accent: "0 14px 34px -16px rgba(56, 189, 248, 0.55)",
    },
};

pub const LIGHT_THEME: ThemeTokens = ThemeTokens {
    dark: false,
    colors: ThemeColors {
        background: Color::rgb(0xeef2f8),
        background_soft: Color::rgb(0xf5f8fc),
        background_deep: Color::rgb(0xe5ebf4),
        panel: Color::rgba(0xffffffe0),
        panel_solid: Color::rgb(0xffffff),
        panel_raised: Color::rgba(0xfffffff5),
        panel_inset: Color::rgba(0xf1f5fad9),
        surface: Color::rgba(0x0f172a0a),
        surface_hover: Color::rgba(0x0f172a12),
        input_background: Color::rgb(0xffffff),
        border: Color::rgba(0x0f172a1a),
        border_soft: Color::rgba(0x0f172a0f),
        border_strong: Color::rgba(0x0f172a33),
        text: Color::rgb(0x0f1b2d),
        text_secondary: Color::rgb(0x45566d),
        text_muted: Color::rgb(0x6b7c93),
        text_faint: Color::rgb(0x94a3b8),
        accent: Color::rgb(0x0284c7),
        accent_secondary: Color::rgb(0x6366f1),
        accent_soft: Color::rgba(0x0284c717),
        accent_glow: Color::rgba(0x0284c738),
        accent_pressed: Color::rgb(0x075985),
        success: Color::rgb(0x059669),
        success_soft: Color::rgba(0x0596691a),
        success_glow: Color::rgba(0x05966938),
        warning: Color::rgb(0xb45309),
        warning_soft: Color::rgba(0xb453091a),
        danger: Color::rgb(0xe11d48),
        danger_soft: Color::rgba(0xe11d4817),
        danger_text: Color::rgb(0x9f1239),
        info: Color::rgb(0x2563eb),
        info_soft: Color::rgba(0x2563eb1a),
        purple: Color::rgb(0x7c3aed),
        purple_soft: Color::rgba(0x7c3aed17),
        coordinator: CapabilityColor {
            solid: Color::rgb(0x0284c7),
            soft: Color::rgba(0x0284c71a),
        },
        relay: CapabilityColor {
            solid: Color::rgb(0x7c3aed),
            soft: Color::rgba(0x7c3aed1a),
        },
        exit: CapabilityColor {
            solid: Color::rgb(0x059669),
            soft: Color::rgba(0x0596691a),
        },
        file: CapabilityColor {
            solid: Color::rgb(0xb45309),
            soft: Color::rgba(0xb453091a),
        },
    },
    shadows: ShadowTokens {
        panel: "0 22px 50px -32px rgba(15, 23, 42, 0.35), 0 1px 2px rgba(15, 23, 42, 0.04)",
        popover: "0 30px 80px -24px rgba(15, 23, 42, 0.35)",
        accent: "0 14px 30px -16px rgba(2, 132, 199, 0.45)",
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStep {
    pub size_px: u8,
    pub line_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypographyTokens {
    pub sans_family: &'static str,
    pub mono_family: &'static str,
    pub xs: TypeStep,
    pub sm: TypeStep,
    pub base: TypeStep,
    pub md: TypeStep,
    pub lg: TypeStep,
    pub xl: TypeStep,
    pub xxl: TypeStep,
    pub regular_weight: u16,
    pub medium_weight: u16,
    pub semibold_weight: u16,
    pub bold_weight: u16,
}

pub const TYPOGRAPHY: TypographyTokens = TypographyTokens {
    sans_family: "\"Inter\", \"SF Pro Text\", -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans SC\", sans-serif",
    mono_family: "\"JetBrains Mono\", \"SF Mono\", ui-monospace, SFMono-Regular, \"Cascadia Code\", \"Fira Code\", Menlo, Consolas, monospace",
    xs: TypeStep {
        size_px: 11,
        line_height: 1.45,
    },
    sm: TypeStep {
        size_px: 12,
        line_height: 1.5,
    },
    base: TypeStep {
        size_px: 13,
        line_height: 1.55,
    },
    md: TypeStep {
        size_px: 14,
        line_height: 1.55,
    },
    lg: TypeStep {
        size_px: 16,
        line_height: 1.45,
    },
    xl: TypeStep {
        size_px: 20,
        line_height: 1.3,
    },
    xxl: TypeStep {
        size_px: 24,
        line_height: 1.2,
    },
    regular_weight: 400,
    medium_weight: 500,
    semibold_weight: 600,
    bold_weight: 700,
};

pub const SPACING_PX: [u16; 13] = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadiusTokens {
    pub xs_px: u16,
    pub sm_px: u16,
    pub md_px: u16,
    pub lg_px: u16,
    pub xl_px: u16,
    pub full_px: u16,
}

pub const RADII: RadiusTokens = RadiusTokens {
    xs_px: 4,
    sm_px: 9,
    md_px: 12,
    lg_px: 16,
    xl_px: 20,
    full_px: 9999,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DimensionTokens {
    pub sidebar_px: u16,
    pub topbar_px: u16,
    pub action_panel_compact_px: u16,
    pub action_panel_default_px: u16,
    pub detail_min_px: u16,
    pub detail_default_px: u16,
    pub detail_max_px: u16,
    pub bottom_nav_px: u16,
    pub content_max_px: u16,
    pub content_form_max_px: u16,
    pub mobile_breakpoint_px: u16,
    pub navigation_breakpoint_px: u16,
    pub detail_breakpoint_px: u16,
    pub control_sm_px: u16,
    pub control_md_px: u16,
    pub control_lg_px: u16,
    pub window_min_width_px: u16,
    pub window_min_height_px: u16,
    pub window_default_width_px: u16,
    pub window_default_height_px: u16,
}

/// Shared geometry for every application surface. Breakpoints are inclusive
/// lower bounds; `mobile_breakpoint_px` is the lower bound of compact mode, so
/// widths below it are the mobile single-column layout.
pub const DIMENSIONS: DimensionTokens = DimensionTokens {
    sidebar_px: 232,
    topbar_px: 56,
    action_panel_compact_px: 300,
    action_panel_default_px: 330,
    detail_min_px: 320,
    detail_default_px: 360,
    detail_max_px: 480,
    bottom_nav_px: 64,
    content_max_px: 1440,
    content_form_max_px: 960,
    mobile_breakpoint_px: 600,
    navigation_breakpoint_px: 900,
    detail_breakpoint_px: 1280,
    control_sm_px: 32,
    control_md_px: 40,
    control_lg_px: 44,
    window_min_width_px: 960,
    window_min_height_px: 640,
    window_default_width_px: 1180,
    window_default_height_px: 800,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionTokens {
    pub instant_ms: u16,
    pub fast_ms: u16,
    pub base_ms: u16,
    pub slow_ms: u16,
    pub pulse_ms: u16,
    pub reduced_ms: u16,
    pub standard_easing: &'static str,
    pub entrance_easing: &'static str,
}

pub const MOTION: MotionTokens = MotionTokens {
    instant_ms: 0,
    fast_ms: 120,
    base_ms: 180,
    slow_ms: 220,
    pulse_ms: 2600,
    reduced_ms: 1,
    standard_easing: "ease",
    entrance_easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
};

impl ThemeTokens {
    /// Returns the CSS selector owned by this theme.
    pub const fn selector(self) -> &'static str {
        if self.dark {
            ":root"
        } else {
            "[data-theme=\"light\"]"
        }
    }

    fn push_color(css: &mut String, name: &str, color: Color) {
        css.push_str("  --");
        css.push_str(name);
        css.push_str(": ");
        css.push_str(&color.css());
        css.push_str(";\n");
    }

    fn push_px(css: &mut String, name: &str, value: u16) {
        css.push_str("  --");
        css.push_str(name);
        css.push_str(": ");
        css.push_str(&value.to_string());
        css.push_str("px;\n");
    }

    fn push_ms(css: &mut String, name: &str, value: u16) {
        css.push_str("  --");
        css.push_str(name);
        css.push_str(": ");
        css.push_str(&value.to_string());
        css.push_str("ms;\n");
    }

    /// Generates the theme-specific custom-property block.
    pub fn css_block(self) -> String {
        let colors = self.colors;
        let mut css = String::new();
        css.push_str(self.selector());
        css.push_str(" {\n");
        css.push_str(if self.dark {
            "  color-scheme: dark;\n\n"
        } else {
            "  color-scheme: light;\n\n"
        });

        for (name, color) in [
            ("bg", colors.background),
            ("bg-soft", colors.background_soft),
            ("bg-deep", colors.background_deep),
            ("panel", colors.panel),
            ("panel-solid", colors.panel_solid),
            ("panel-raised", colors.panel_raised),
            ("panel-inset", colors.panel_inset),
            ("surface", colors.surface),
            ("surface-hover", colors.surface_hover),
            ("input-bg", colors.input_background),
            ("border", colors.border),
            ("border-soft", colors.border_soft),
            ("border-strong", colors.border_strong),
            ("text", colors.text),
            ("text-main", colors.text),
            ("text-secondary", colors.text_secondary),
            ("text-muted", colors.text_muted),
            ("text-faint", colors.text_faint),
            ("accent", colors.accent),
            ("accent-2", colors.accent_secondary),
            ("accent-soft", colors.accent_soft),
            ("accent-glow", colors.accent_glow),
            ("accent-pressed", colors.accent_pressed),
            ("success", colors.success),
            ("success-soft", colors.success_soft),
            ("success-glow", colors.success_glow),
            ("warning", colors.warning),
            ("warning-soft", colors.warning_soft),
            ("danger", colors.danger),
            ("danger-soft", colors.danger_soft),
            ("danger-text", colors.danger_text),
            ("info", colors.info),
            ("info-soft", colors.info_soft),
            ("purple", colors.purple),
            ("purple-soft", colors.purple_soft),
            ("capability-coordinator", colors.coordinator.solid),
            ("capability-coordinator-soft", colors.coordinator.soft),
            ("capability-relay", colors.relay.solid),
            ("capability-relay-soft", colors.relay.soft),
            ("capability-exit", colors.exit.solid),
            ("capability-exit-soft", colors.exit.soft),
            ("capability-file", colors.file.solid),
            ("capability-file-soft", colors.file.soft),
        ] {
            Self::push_color(&mut css, name, color);
        }

        css.push_str("\n  /* Source-compatible semantic aliases. */\n");
        for (alias, target) in [
            ("green", "--success"),
            ("green-soft", "--success-soft"),
            ("green-glow", "--success-glow"),
            ("amber", "--warning"),
            ("amber-soft", "--warning-soft"),
            ("red", "--danger"),
            ("red-soft", "--danger-soft"),
            ("bg-card", "--panel-solid"),
            ("tone-cyan", "--accent"),
            ("tone-cyan-soft", "--accent-soft"),
            ("tone-violet", "--purple"),
            ("tone-violet-soft", "--purple-soft"),
            ("tone-emerald", "--success"),
            ("tone-emerald-soft", "--success-soft"),
            ("tone-amber", "--warning"),
            ("tone-amber-soft", "--warning-soft"),
            ("tone-blue", "--info"),
            ("tone-blue-soft", "--info-soft"),
            ("tone-rose", "--danger"),
            ("tone-rose-soft", "--danger-soft"),
        ] {
            css.push_str("  --");
            css.push_str(alias);
            css.push_str(": var(");
            css.push_str(target);
            css.push_str(");\n");
        }

        css.push_str("\n  --radius-xs: 4px;\n  --radius-sm: 9px;\n  --radius-md: 12px;\n  --radius-lg: 16px;\n  --radius-xl: 20px;\n  --radius-full: 9999px;\n");
        css.push_str("  --shadow-panel: ");
        css.push_str(self.shadows.panel);
        css.push_str(";\n  --shadow-pop: ");
        css.push_str(self.shadows.popover);
        css.push_str(";\n  --shadow-accent: ");
        css.push_str(self.shadows.accent);
        css.push_str(";\n");
        css.push_str("}\n\n");
        css
    }
}

/// Generates the complete WebUI stylesheet from shared tokens plus the base
/// component stylesheet.
pub fn webui_css() -> &'static str {
    static WEBUI_CSS: OnceLock<String> = OnceLock::new();
    WEBUI_CSS.get_or_init(|| {
        let mut css = String::from(
            "/* Generated from crates/common/src/design_tokens.rs. Do not duplicate token values manually. */\n\n",
        );
        css.push_str("/* 1. Design tokens */\n");
        css.push_str(&DARK_THEME.css_block());
        css.push_str(&LIGHT_THEME.css_block());
        css.push_str(
            "/* Semantic tone hooks for badges, capability rings, and topology. */\n\
             [data-tone=\"neutral\"] { --tone: var(--text-muted); --tone-soft: var(--surface); }\n\
             [data-tone=\"accent\"], [data-tone=\"info\"] { --tone: var(--info); --tone-soft: var(--info-soft); }\n\
             [data-tone=\"success\"] { --tone: var(--success); --tone-soft: var(--success-soft); }\n\
             [data-tone=\"warning\"] { --tone: var(--warning); --tone-soft: var(--warning-soft); }\n\
             [data-tone=\"danger\"] { --tone: var(--danger); --tone-soft: var(--danger-soft); }\n\
             [data-capability=\"coordinator\"] { --capability-color: var(--capability-coordinator); --capability-soft: var(--capability-coordinator-soft); }\n\
             [data-capability=\"relay\"] { --capability-color: var(--capability-relay); --capability-soft: var(--capability-relay-soft); }\n\
             [data-capability=\"exit\"] { --capability-color: var(--capability-exit); --capability-soft: var(--capability-exit-soft); }\n\
             [data-capability=\"file\"] { --capability-color: var(--capability-file); --capability-soft: var(--capability-file-soft); }\n\n",
        );
        css.push_str("/* 2. Typography, spacing, dimensions, and motion */\n");
        css.push_str(":root {\n");
        css.push_str("  --font-sans: ");
        css.push_str(TYPOGRAPHY.sans_family);
        css.push_str(";\n  --font-mono: ");
        css.push_str(TYPOGRAPHY.mono_family);
        css.push_str(";\n");
        for (name, step) in [
            ("xs", TYPOGRAPHY.xs),
            ("sm", TYPOGRAPHY.sm),
            ("base", TYPOGRAPHY.base),
            ("md", TYPOGRAPHY.md),
            ("lg", TYPOGRAPHY.lg),
            ("xl", TYPOGRAPHY.xl),
            ("xxl", TYPOGRAPHY.xxl),
        ] {
            ThemeTokens::push_px(&mut css, &format!("font-size-{name}"), step.size_px.into());
            css.push_str("  --line-height-");
            css.push_str(name);
            css.push_str(": ");
            css.push_str(&step.line_height.to_string());
            css.push_str(";\n");
        }
        css.push_str("  --font-weight-regular: 400;\n  --font-weight-medium: 500;\n  --font-weight-semibold: 600;\n  --font-weight-bold: 700;\n");
        for (index, value) in SPACING_PX.into_iter().enumerate() {
            ThemeTokens::push_px(&mut css, &format!("space-{index}"), value);
        }
        ThemeTokens::push_px(&mut css, "sidebar-w", DIMENSIONS.sidebar_px);
        ThemeTokens::push_px(&mut css, "topbar-h", DIMENSIONS.topbar_px);
        ThemeTokens::push_px(
            &mut css,
            "action-panel-compact-w",
            DIMENSIONS.action_panel_compact_px,
        );
        ThemeTokens::push_px(
            &mut css,
            "action-panel-w",
            DIMENSIONS.action_panel_default_px,
        );
        ThemeTokens::push_px(&mut css, "detail-min-w", DIMENSIONS.detail_min_px);
        ThemeTokens::push_px(&mut css, "detail-w", DIMENSIONS.detail_default_px);
        ThemeTokens::push_px(&mut css, "detail-max-w", DIMENSIONS.detail_max_px);
        ThemeTokens::push_px(&mut css, "bottom-nav-h", DIMENSIONS.bottom_nav_px);
        ThemeTokens::push_px(&mut css, "content-max-w", DIMENSIONS.content_max_px);
        ThemeTokens::push_px(
            &mut css,
            "content-form-max-w",
            DIMENSIONS.content_form_max_px,
        );
        ThemeTokens::push_px(
            &mut css,
            "breakpoint-mobile",
            DIMENSIONS.mobile_breakpoint_px,
        );
        ThemeTokens::push_px(
            &mut css,
            "breakpoint-navigation",
            DIMENSIONS.navigation_breakpoint_px,
        );
        ThemeTokens::push_px(
            &mut css,
            "breakpoint-detail",
            DIMENSIONS.detail_breakpoint_px,
        );
        ThemeTokens::push_px(&mut css, "control-sm-h", DIMENSIONS.control_sm_px);
        ThemeTokens::push_px(&mut css, "control-md-h", DIMENSIONS.control_md_px);
        ThemeTokens::push_px(&mut css, "control-lg-h", DIMENSIONS.control_lg_px);
        ThemeTokens::push_px(&mut css, "window-min-w", DIMENSIONS.window_min_width_px);
        ThemeTokens::push_px(&mut css, "window-min-h", DIMENSIONS.window_min_height_px);
        ThemeTokens::push_px(
            &mut css,
            "window-default-w",
            DIMENSIONS.window_default_width_px,
        );
        ThemeTokens::push_px(
            &mut css,
            "window-default-h",
            DIMENSIONS.window_default_height_px,
        );
        ThemeTokens::push_ms(&mut css, "motion-duration-instant", MOTION.instant_ms);
        ThemeTokens::push_ms(&mut css, "motion-duration-fast", MOTION.fast_ms);
        ThemeTokens::push_ms(&mut css, "motion-duration-base", MOTION.base_ms);
        ThemeTokens::push_ms(&mut css, "motion-duration-slow", MOTION.slow_ms);
        ThemeTokens::push_ms(&mut css, "motion-duration-pulse", MOTION.pulse_ms);
        css.push_str("  --motion-ease-standard: ");
        css.push_str(MOTION.standard_easing);
        css.push_str(";\n  --motion-ease-entrance: ");
        css.push_str(MOTION.entrance_easing);
        css.push_str(";\n");
        css.push_str(&format!(
            "  --motion-duration-reduced: {}ms;\n",
            MOTION.reduced_ms
        ));
        css.push_str("}\n\n");
        css.push_str(include_str!("../assets/webui.css"));
        css
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color) -> f32 {
        fn channel(value: u8) -> f32 {
            let value = value as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (lighter, darker) = if luminance(a) > luminance(b) {
            (luminance(a), luminance(b))
        } else {
            (luminance(b), luminance(a))
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn both_themes_define_every_semantic_color_role() {
        for theme in [DARK_THEME, LIGHT_THEME] {
            let colors = theme.colors;
            for color in [
                colors.background,
                colors.background_soft,
                colors.background_deep,
                colors.panel,
                colors.panel_solid,
                colors.panel_raised,
                colors.panel_inset,
                colors.surface,
                colors.surface_hover,
                colors.input_background,
                colors.border,
                colors.border_soft,
                colors.border_strong,
                colors.text,
                colors.text_secondary,
                colors.text_muted,
                colors.text_faint,
                colors.accent,
                colors.accent_secondary,
                colors.accent_soft,
                colors.accent_glow,
                colors.success,
                colors.warning,
                colors.danger,
                colors.info,
                colors.coordinator.solid,
                colors.relay.solid,
                colors.exit.solid,
                colors.file.solid,
            ] {
                assert!(
                    color.rgba_hex() != 0,
                    "semantic colors must not silently collapse to transparent black"
                );
            }
        }
    }

    #[test]
    fn capability_colors_are_distinct_in_both_themes() {
        for theme in [DARK_THEME, LIGHT_THEME] {
            let colors = theme.colors;
            let solids = [
                colors.coordinator.solid.rgb_hex(),
                colors.relay.solid.rgb_hex(),
                colors.exit.solid.rgb_hex(),
                colors.file.solid.rgb_hex(),
            ];
            for (index, color) in solids.iter().enumerate() {
                assert!(
                    !solids[..index].contains(color),
                    "capability colors must be unique within {:?}",
                    theme
                );
            }
        }
    }

    #[test]
    fn body_text_meets_wcag_aa_for_normal_text() {
        for theme in [DARK_THEME, LIGHT_THEME] {
            assert!(
                contrast(theme.colors.text, theme.colors.background) >= 4.5,
                "body text contrast failed for {:?}: {}",
                theme,
                contrast(theme.colors.text, theme.colors.background)
            );
            assert!(
                contrast(theme.colors.text_secondary, theme.colors.background) >= 4.5,
                "secondary text contrast failed for {:?}: {}",
                theme,
                contrast(theme.colors.text_secondary, theme.colors.background)
            );
        }
    }

    #[test]
    fn generated_css_contains_both_themes_and_every_token_family() {
        let css = webui_css();
        assert!(css.contains(":root {"));
        assert!(css.contains("[data-theme=\"light\"] {"));
        for token in [
            "--bg:",
            "--text:",
            "--success:",
            "--warning:",
            "--danger:",
            "--info:",
            "--capability-coordinator:",
            "--capability-relay:",
            "--capability-exit:",
            "--capability-file:",
            "--font-sans:",
            "--font-size-base:",
            "--line-height-base:",
            "--space-1:",
            "--radius-md:",
            "--shadow-panel:",
            "--control-md-h:",
            "--detail-min-w:",
            "--detail-w:",
            "--detail-max-w:",
            "--bottom-nav-h:",
            "--content-form-max-w:",
            "--breakpoint-mobile:",
            "--breakpoint-navigation:",
            "--breakpoint-detail:",
            "--motion-duration-base:",
            "--motion-ease-standard:",
            "--motion-duration-reduced:",
        ] {
            assert!(css.contains(token), "generated CSS is missing {token}");
        }
        assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
    }

    #[test]
    fn base_stylesheet_does_not_redeclare_theme_tokens() {
        let base = include_str!("../assets/webui.css");
        assert!(!base.contains("--bg: #"));
        assert!(!base.contains("[data-theme=\"light\"] {\n  color-scheme"));
    }

    fn css_definitions(source: &str) -> std::collections::HashSet<&str> {
        let mut definitions = std::collections::HashSet::new();
        for (index, _) in source.match_indices("--") {
            let rest = &source[index + 2..];
            let Some(end) = rest.find([':', ',', ')', ';']) else {
                continue;
            };
            if rest.as_bytes().get(end) == Some(&b':') {
                definitions.insert(&rest[..end]);
            }
        }
        definitions
    }

    fn css_variable_usages(source: &str) -> std::collections::HashSet<&str> {
        let mut usages = std::collections::HashSet::new();
        for (index, _) in source.match_indices("var(--") {
            let rest = &source[index + 6..];
            let Some(end) = rest.find([',', ')']) else {
                continue;
            };
            usages.insert(&rest[..end]);
        }
        usages
    }

    #[test]
    fn base_stylesheet_variable_references_are_backed_by_tokens() {
        let generated = webui_css();
        let base = include_str!("../assets/webui.css");
        let definitions = css_definitions(generated);
        for usage in css_variable_usages(base) {
            assert!(
                definitions.contains(usage),
                "base stylesheet references undefined design token --{usage}"
            );
        }
    }
}
