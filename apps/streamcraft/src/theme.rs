//! StreamCraft visual tokens — ink studio + brand violet.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0e, 0x0f, 0x12);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x18, 0x1e);
pub const ELEVATED: Color32 = Color32::from_rgb(0x1e, 0x21, 0x29);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x38);
pub const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf0, 0xeb);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x87, 0x98);
pub const ACCENT: Color32 = Color32::from_rgb(0x9b, 0x6b, 0xff);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x3d, 0x2a, 0x5c);
pub const OK: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
pub const BAD: Color32 = Color32::from_rgb(0xf0, 0x71, 0x78);

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(0);
    style.interaction.selectable_labels = false;

    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(28.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();

    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.override_text_color = Some(TEXT);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    visuals.widgets.inactive.bg_fill = ELEVATED;
    visuals.widgets.inactive.weak_bg_fill = ELEVATED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x28, 0x2c, 0x38);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x28, 0x2c, 0x38);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
    visuals.widgets.active.bg_fill = ACCENT_DIM;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.active.corner_radius = CornerRadius::same(8);
    visuals.widgets.open.bg_fill = ELEVATED;
    visuals.widgets.open.corner_radius = CornerRadius::same(8);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.panel_fill = PANEL;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = ELEVATED;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(0);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.striped = false;

    style.visuals = visuals;
    ctx.set_style_of(Theme::Dark, style);
}

pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(TEXT).strong())
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 36.0)),
    )
}

pub fn ghost_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).color(MUTED))
            .fill(ELEVATED)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 36.0)),
    )
}
