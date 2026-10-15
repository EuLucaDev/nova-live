//! The Nova Foods interface themes, shared by every app in the suite.
//!
//! The fourteen themes are Nova Photo's (`novaphoto/crates/ui-egui/src/theme.rs`): the four Pro
//! brightness levels, Studio, Classic, Solarized Dark, Adwaita, Breeze and Soft. Each app maps a
//! [`Palette`] onto its own tokens, so a theme looks the same everywhere while app-specific colours
//! (sheet cells, the page, the timeline) stay the app's own.
//!
//! No dependencies: colours are plain `[r, g, b, a]` (unmultiplied), so any egui version can use it,
//! on every target including wasm.
#![forbid(unsafe_code)]

mod palettes;

/// Unmultiplied RGBA.
pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ThemeKind {
    Pro,
    ProMedium,
    ProLightGray,
    ProLight,
    Studio,
    StudioLight,
    Classic,
    SolarizedDark,
    Adwaita,
    AdwaitaDark,
    BreezeLight,
    BreezeDark,
    SoftDark,
    SoftLight,
}

impl ThemeKind {
    /// Menu order, the same as Nova Photo's Window › Theme.
    pub const ALL: [ThemeKind; 14] = [
        ThemeKind::Pro,
        ThemeKind::ProMedium,
        ThemeKind::ProLightGray,
        ThemeKind::ProLight,
        ThemeKind::Studio,
        ThemeKind::StudioLight,
        ThemeKind::Classic,
        ThemeKind::SolarizedDark,
        ThemeKind::Adwaita,
        ThemeKind::AdwaitaDark,
        ThemeKind::BreezeLight,
        ThemeKind::BreezeDark,
        ThemeKind::SoftDark,
        ThemeKind::SoftLight,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            ThemeKind::Pro => "Pro (Dark)",
            ThemeKind::ProMedium => "Pro (Medium Gray)",
            ThemeKind::ProLightGray => "Pro (Light Gray)",
            ThemeKind::ProLight => "Pro (Light)",
            ThemeKind::Studio => "Studio (Dark)",
            ThemeKind::StudioLight => "Studio (Light)",
            ThemeKind::Classic => "Classic",
            ThemeKind::SolarizedDark => "Solarized Dark",
            ThemeKind::Adwaita => "Adwaita (Light)",
            ThemeKind::AdwaitaDark => "Adwaita (Dark)",
            ThemeKind::BreezeLight => "Breeze Light",
            ThemeKind::BreezeDark => "Breeze Dark",
            ThemeKind::SoftDark => "Soft Dark",
            ThemeKind::SoftLight => "Soft Light",
        }
    }

    /// Stable id for preferences and commands (`window.theme.<id>`), the same as Nova Photo's.
    pub const fn id(self) -> &'static str {
        match self {
            ThemeKind::Pro => "pro",
            ThemeKind::ProMedium => "proMedium",
            ThemeKind::ProLightGray => "proLightGray",
            ThemeKind::ProLight => "proLight",
            ThemeKind::Studio => "studio",
            ThemeKind::StudioLight => "studioLight",
            ThemeKind::Classic => "classic",
            ThemeKind::SolarizedDark => "solarizedDark",
            ThemeKind::Adwaita => "adwaita",
            ThemeKind::AdwaitaDark => "adwaitaDark",
            ThemeKind::BreezeLight => "breezeLight",
            ThemeKind::BreezeDark => "breezeDark",
            ThemeKind::SoftDark => "softDark",
            ThemeKind::SoftLight => "softLight",
        }
    }

    /// Accepts the id, the label and the aliases Nova Photo accepts.
    pub fn from_name(s: &str) -> Option<Self> {
        let key: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-' | '(' | ')'))
            .flat_map(char::to_lowercase)
            .collect();
        Some(match key.as_str() {
            "pro" | "prodark" => ThemeKind::Pro,
            "promedium" | "promediumgray" | "mediumgray" => ThemeKind::ProMedium,
            "prolightgray" | "lightgray" => ThemeKind::ProLightGray,
            "prolight" | "lightest" => ThemeKind::ProLight,
            "studio" | "studiodark" => ThemeKind::Studio,
            "studiolight" => ThemeKind::StudioLight,
            "classic" | "win2000" | "retro" => ThemeKind::Classic,
            "solarizeddark" | "solarized" => ThemeKind::SolarizedDark,
            "adwaita" | "adwaitalight" | "gnome" => ThemeKind::Adwaita,
            "adwaitadark" | "gnomedark" => ThemeKind::AdwaitaDark,
            "breezelight" | "breeze" | "kde" | "plasma" => ThemeKind::BreezeLight,
            "breezedark" | "kdedark" | "plasmadark" => ThemeKind::BreezeDark,
            "softdark" => ThemeKind::SoftDark,
            "softlight" => ThemeKind::SoftLight,
            _ => return None,
        })
    }

    pub const fn palette(self) -> Palette {
        palettes::palette(self)
    }

    /// Dark surfaces (light text).
    pub const fn is_dark(self) -> bool {
        matches!(
            self,
            ThemeKind::Pro
                | ThemeKind::ProMedium
                | ThemeKind::Studio
                | ThemeKind::SolarizedDark
                | ThemeKind::AdwaitaDark
                | ThemeKind::BreezeDark
                | ThemeKind::SoftDark
        )
    }
}

/// A theme's colours and shape, with Nova Photo's token names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Window chrome: title bar, toolbars, menus bar.
    pub chrome: Rgba,
    /// The work area around the document.
    pub canvas: Rgba,
    pub canvas_dot: Rgba,
    /// Dock background behind panels.
    pub dock: Rgba,
    /// Panels, cards, popups.
    pub card: Rgba,
    pub card_border: Rgba,
    /// Inputs, fields, dropdowns.
    pub field: Rgba,
    pub field_border: Rgba,
    pub hover: Rgba,
    pub pressed: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub text_faint: Rgba,
    pub icon: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub accent_border: Rgba,
    pub accent_text: Rgba,
    pub separator: Rgba,
    pub shadow: Rgba,
    pub primary_bg: Rgba,
    pub primary_text: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
    /// Classic: 3D bevels instead of flat fills.
    pub bevel: bool,
    /// Pro: Photoshop-grammar layout (Nova Photo only).
    pub pro: bool,
    pub round_chips: bool,
    pub tab_strip: Rgba,
    pub row_selected: Rgba,
    pub histogram_bg: Rgba,
    pub histogram_level: u8,
    pub caption_close: Rgba,
    pub caption_close_text: Rgba,
    pub scrim: Rgba,
}

/// Mix two colours: `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])]
}

/// The same colour with another alpha.
pub const fn alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_name(k.id()), Some(k));
            assert_eq!(ThemeKind::from_name(k.label()), Some(k));
            assert!(seen.insert(k.id()));
        }
    }

    #[test]
    fn darkness_matches_text_contrast() {
        for k in ThemeKind::ALL {
            let p = k.palette();
            let lum = |c: Rgba| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
            assert_eq!(k.is_dark(), lum(p.text) > lum(p.card), "{}", k.id());
        }
    }
}
