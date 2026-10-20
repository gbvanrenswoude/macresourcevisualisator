//! Colours, fonts and the egui style, for dark and light appearance.

use egui::epaint::text::Blob;
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Visuals};
use std::sync::Arc;
use tessera_core::disk::{Category, Kind};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub bg: Color32,
    pub bg_glow_a: Color32,
    pub bg_glow_b: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub highlight: Color32,
    pub shadow: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub text3: Color32,
    pub track: Color32,
    pub grid: Color32,
    pub tile_text: Color32,

    pub blue: Color32,
    pub violet: Color32,
    pub teal: Color32,
    pub cyan: Color32,
    pub pink: Color32,
    pub orange: Color32,
    pub amber: Color32,
    pub green: Color32,
    pub red: Color32,
    pub indigo: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: hex(0x0A0C12),
            bg_glow_a: Color32::from_rgba_unmultiplied(110, 90, 255, 30),
            bg_glow_b: Color32::from_rgba_unmultiplied(20, 200, 180, 18),
            card: Color32::from_rgba_unmultiplied(255, 255, 255, 9),
            card_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 17),
            highlight: Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            text: hex(0xEEF1F7),
            text2: hex(0xA5AEBD),
            text3: hex(0x6C7584),
            track: Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            grid: Color32::from_rgba_unmultiplied(255, 255, 255, 9),
            tile_text: Color32::WHITE,
            blue: hex(0x5B8CFF),
            violet: hex(0x9B7BFF),
            teal: hex(0x2DD4BF),
            cyan: hex(0x38BDF8),
            pink: hex(0xF472B6),
            orange: hex(0xFB923C),
            amber: hex(0xFBBF24),
            green: hex(0x34D399),
            red: hex(0xF87171),
            indigo: hex(0x818CF8),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            bg: hex(0xF3F5F9),
            bg_glow_a: Color32::from_rgba_unmultiplied(120, 100, 255, 26),
            bg_glow_b: Color32::from_rgba_unmultiplied(20, 190, 170, 20),
            card: Color32::from_rgba_unmultiplied(255, 255, 255, 215),
            card_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 245),
            border: Color32::from_rgba_unmultiplied(15, 23, 42, 16),
            highlight: Color32::from_rgba_unmultiplied(255, 255, 255, 120),
            shadow: Color32::from_rgba_unmultiplied(15, 23, 42, 22),
            text: hex(0x0F172A),
            text2: hex(0x4B5565),
            text3: hex(0x8A93A3),
            track: Color32::from_rgba_unmultiplied(15, 23, 42, 14),
            grid: Color32::from_rgba_unmultiplied(15, 23, 42, 10),
            tile_text: Color32::WHITE,
            blue: hex(0x3B6FF0),
            violet: hex(0x7C5CF0),
            teal: hex(0x0FB5A3),
            cyan: hex(0x0EA5E9),
            pink: hex(0xE6519A),
            orange: hex(0xF07B2C),
            amber: hex(0xE8A10C),
            green: hex(0x16B07A),
            red: hex(0xE5484D),
            indigo: hex(0x5B63E8),
        }
    }

    pub fn for_ctx(ctx: &egui::Context) -> Self {
        if ctx.global_style().visuals.dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }

    pub fn category(&self, c: Category) -> Color32 {
        match c {
            Category::Apps => hex(0x3B82F6),
            Category::Projects => hex(0x8B5CF6),
            Category::Developer => hex(0x14B8A6),
            Category::AiModels => hex(0xEC4899),
            Category::Media => hex(0xF97316),
            Category::Documents => hex(0xEAB308),
            Category::AppData => hex(0x6366F1),
            Category::Caches => hex(0xF43F5E),
            Category::Trash => hex(0xFB7185),
            Category::System => hex(0x64748B),
            Category::Other => hex(0x8B95A7),
        }
    }

    /// Tile colour for a node of `kind` inside a block of base colour `base`.
    pub fn kind_color(&self, kind: Kind, base: Color32) -> Color32 {
        match kind {
            Kind::Artifacts | Kind::Cache => hex(0xF43F5E),
            Kind::Git => hex(0xF59E0B),
            Kind::App => hex(0x3B82F6),
            Kind::Project => hex(0x8B5CF6),
            Kind::VirtualMachine | Kind::VmDisk => hex(0x14B8A6),
            Kind::Video => hex(0xF97316),
            Kind::Image | Kind::MediaLibrary => hex(0xFB923C),
            Kind::Audio => hex(0xEC4899),
            Kind::Archive | Kind::DiskImage => hex(0xEAB308),
            Kind::Model => hex(0xD946EF),
            Kind::Database => hex(0x6366F1),
            _ => base,
        }
    }

    pub fn severity(&self, level: u8) -> Color32 {
        match level {
            0 => self.green,
            1 => self.amber,
            2 => self.orange,
            _ => self.red,
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        visuals.panel_fill = self.bg;
        visuals.window_fill = if self.dark {
            hex(0x161A22)
        } else {
            hex(0xFFFFFF)
        };
        visuals.window_stroke = egui::Stroke::new(1.0, self.border);
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(10);
        visuals.window_shadow = egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: self.shadow,
        };
        visuals.popup_shadow = egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: self.shadow,
        };
        visuals.selection.bg_fill = self.blue.gamma_multiply(0.5);
        visuals.hyperlink_color = self.blue;
        visuals.override_text_color = Some(self.text);
        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.corner_radius = egui::CornerRadius::same(7);
        }
        visuals.widgets.inactive.weak_bg_fill = self.track;
        visuals.widgets.inactive.bg_fill = self.track;
        visuals.widgets.hovered.weak_bg_fill = self.card_hover;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        ctx.set_visuals_of(
            if self.dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
            visuals,
        );
    }
}

// ---------------------------------------------------------------------------
// Fonts: San Francisco straight from the system, at several weights.

pub const REGULAR: &str = "sf-regular";
pub const MEDIUM: &str = "sf-medium";
pub const SEMIBOLD: &str = "sf-semibold";
pub const BOLD: &str = "sf-bold";
pub const ROUNDED: &str = "sf-rounded";

/// Memory-map a system font. Its pages stay clean and shared with every
/// other app using the font, instead of a private copy per weight (SF Pro
/// alone is 8 MB).
fn map_font(path: &str) -> Option<Blob> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len() as usize;
    if len == 0 {
        return None;
    }
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return None;
    }
    // Never unmapped: the fonts are used for the life of the app. The files
    // live on the sealed system volume, so they cannot change underneath.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    Some(Arc::new(bytes))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let sf = map_font("/System/Library/Fonts/SFNS.ttf");
    let rounded = map_font("/System/Library/Fonts/SFNSRounded.ttf");
    let mono = map_font("/System/Library/Fonts/SFNSMono.ttf");

    let fallback: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut add = |name: &str, blob: &Option<Blob>, weight: f32| {
        let family = FontFamily::Name(name.into());
        let mut chain = Vec::new();
        if let Some(blob) = blob {
            let mut data = FontData::from_blob(blob.clone(), 0);
            data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", weight)]);
            defs.font_data.insert(name.to_string(), Arc::new(data));
            chain.push(name.to_string());
        }
        chain.extend(fallback.iter().cloned());
        defs.families.insert(family, chain);
    };
    add(REGULAR, &sf, 400.0);
    add(MEDIUM, &sf, 510.0);
    add(SEMIBOLD, &sf, 590.0);
    add(BOLD, &sf, 700.0);
    add(ROUNDED, &rounded, 650.0);

    // Default families.
    if sf.is_some() {
        defs.families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, REGULAR.into());
    }
    if let Some(blob) = mono {
        defs.font_data
            .insert("sf-mono".into(), Arc::new(FontData::from_blob(blob, 0)));
        defs.families
            .get_mut(&FontFamily::Monospace)
            .unwrap()
            .insert(0, "sf-mono".into());
    }
    ctx.set_fonts(defs);
}

pub fn font(size: f32, family: &'static str) -> FontId {
    FontId::new(size, FontFamily::Name(family.into()))
}
