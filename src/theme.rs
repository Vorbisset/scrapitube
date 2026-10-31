//! Thème sombre (palette de Tidalify) et polices.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(0x0f, 0x11, 0x17);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x15, 0x18, 0x21);
pub const SURFACE: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x2a);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x23, 0x28, 0x38);
pub const BORDER: Color32 = Color32::from_rgb(0x2c, 0x32, 0x42);
pub const TEXT: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf5);
pub const MUTED: Color32 = Color32::from_rgb(0xa0, 0xa8, 0xbc);
pub const ACCENT: Color32 = Color32::from_rgb(0x7c, 0x5c, 0xff);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x90, 0x77, 0xff);
pub const PINK: Color32 = Color32::from_rgb(0xff, 0x4f, 0x8b);
pub const SUCCESS: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
pub const WARNING: Color32 = Color32::from_rgb(0xff, 0xb5, 0x47);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x5c, 0x7a);

/// Avenir Next (fournie avec macOS) : index des graisses dans le fichier .ttc.
const SYSTEM_FONT: &str = "/System/Library/Fonts/Avenir Next.ttc";
const FONT_FACES: [(&str, u32); 3] = [("regular", 5), ("bold", 2), ("heavy", 0)];

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn heavy(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("heavy".into()))
}

pub fn apply(ctx: &egui::Context) {
    load_fonts(ctx);
    // Sans ça, egui suit le mode clair de macOS et ignore une partie du thème.
    ctx.set_theme(egui::Theme::Dark);

    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SIDEBAR;
    v.extreme_bg_color = Color32::from_rgb(0x13, 0x16, 0x1e);
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT_HOVER;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(12.0);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.inactive.weak_bg_fill = SURFACE_HOVER;
    w.inactive.bg_fill = SURFACE_HOVER;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x2d, 0x33, 0x46);
    w.hovered.bg_fill = Color32::from_rgb(0x2d, 0x33, 0x46);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.active.weak_bg_fill = ACCENT;
    w.active.bg_fill = ACCENT;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.open.weak_bg_fill = SURFACE_HOVER;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.rounding = Rounding::same(8.0);
        wv.expansion = 0.0;
    }

    style.spacing.button_padding = egui::vec2(16.0, 8.0);
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.interact_size.y = 34.0;
    style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Button, bold(15.0));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(13.0));
    style.text_styles.insert(TextStyle::Heading, heavy(30.0));
    ctx.set_style(style);
}

/// Charge Avenir Next si elle est là ; sinon les familles « bold » et « heavy » retombent
/// sur la police par défaut d'egui.
fn load_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let system = std::fs::read(SYSTEM_FONT).ok();

    for (name, index) in FONT_FACES {
        let mut family = defaults.clone();
        if let Some(bytes) = &system {
            let mut data = FontData::from_owned(bytes.clone());
            data.index = index;
            fonts.font_data.insert(name.into(), data);
            family.insert(0, name.into());
        }
        if name == "regular" {
            fonts.families.insert(FontFamily::Proportional, family);
        } else {
            fonts.families.insert(FontFamily::Name(name.into()), family);
        }
    }
    ctx.set_fonts(fonts);
}

/// Cadre arrondi type « carte ».
pub fn card() -> egui::Frame {
    egui::Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .rounding(14.0)
        .inner_margin(18.0)
}

/// Petite étiquette arrondie (« Full HD · MP4 », « Extrait … »).
pub fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.16))
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).font(bold(12.5)).color(color));
        });
}

/// Gros bouton violet (action principale).
pub fn primary_button(text: impl Into<String>, enabled: bool) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).font(bold(17.0)).color(Color32::WHITE))
        .fill(if enabled { ACCENT } else { SURFACE_HOVER })
        .stroke(Stroke::NONE)
        .rounding(12.0)
}
