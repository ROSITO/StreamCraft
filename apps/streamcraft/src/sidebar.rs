//! Left navigation rail — collapsed (icons) / expanded (icon + label).
//!
//! Caller: `app.rs` `StreamCraftApp::ui` (replaces the inline `Panel::left("rail")` +
//! `rail_item` that only showed "SC" text). No prior sidebar module.
//! User: "tu ajuste le menu pour qu'il ressemble à un vrai menu, icone + texte, mode ouvert ou fermé"

use egui::{RichText, Sense, TextureHandle, Vec2};

use crate::theme::{ACCENT, BAD, BORDER, MUTED, OK, PANEL, TEXT};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavTab {
    Browse,
    Favorites,
    Queue,
    Settings,
}

const COLLAPSED_W: f32 = 64.0;
const EXPANDED_W: f32 = 196.0;

pub fn show(
    ui: &mut egui::Ui,
    tab: &mut NavTab,
    expanded: &mut bool,
    brand: &TextureHandle,
    ffmpeg_ok: bool,
) {
    let width = if *expanded { EXPANDED_W } else { COLLAPSED_W };
    egui::Panel::left("rail")
        .exact_size(width)
        .resizable(false)
        .show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, PANEL);

            ui.add_space(14.0);
            header(ui, brand, expanded);
            ui.add_space(20.0);

            nav_item(ui, tab, NavTab::Browse, "Browse", *expanded, |painter, rect, color| {
                paint_browse(painter, rect, color);
            });
            ui.add_space(4.0);
            nav_item(
                ui,
                tab,
                NavTab::Favorites,
                "Favorites",
                *expanded,
                |painter, rect, color| {
                    paint_favorites(painter, rect, color);
                },
            );
            ui.add_space(4.0);
            nav_item(ui, tab, NavTab::Queue, "Queue", *expanded, |painter, rect, color| {
                paint_queue(painter, rect, color);
            });
            ui.add_space(4.0);
            nav_item(
                ui,
                tab,
                NavTab::Settings,
                "Settings",
                *expanded,
                |painter, rect, color| {
                    paint_settings(painter, rect, color);
                },
            );

            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                footer(ui, expanded, ffmpeg_ok);
            });
        });
}

fn header(ui: &mut egui::Ui, brand: &TextureHandle, expanded: &mut bool) {
    let icon_sz = 32.0;
    if *expanded {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let resp = ui.add(
                egui::Image::new(brand)
                    .fit_to_exact_size(Vec2::splat(icon_sz))
                    .corner_radius(8.0)
                    .sense(Sense::click()),
            );
            if resp.clicked() {
                *expanded = false;
            }
            resp.on_hover_text("Collapse sidebar");
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("StreamCraft").color(TEXT).size(14.0).strong());
                ui.label(RichText::new("Desktop").color(MUTED).size(11.0));
            });
        });
    } else {
        ui.vertical_centered(|ui| {
            let resp = ui.add(
                egui::Image::new(brand)
                    .fit_to_exact_size(Vec2::splat(icon_sz))
                    .corner_radius(8.0)
                    .sense(Sense::click()),
            );
            if resp.clicked() {
                *expanded = true;
            }
            resp.on_hover_text("Expand sidebar");
        });
    }
}

fn nav_item(
    ui: &mut egui::Ui,
    tab: &mut NavTab,
    value: NavTab,
    label: &str,
    expanded: bool,
    paint_icon: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) {
    let selected = *tab == value;
    let height = 40.0;
    let full = if expanded {
        egui::vec2(ui.available_width() - 16.0, height)
    } else {
        egui::vec2(44.0, height)
    };

    ui.horizontal(|ui| {
        if expanded {
            ui.add_space(8.0);
        } else {
            let side = ((ui.available_width() - full.x) * 0.5).max(0.0);
            ui.add_space(side);
        }

        let (rect, resp) = ui.allocate_exact_size(full, Sense::click());
        let fill = if selected {
            ACCENT.gamma_multiply(0.22)
        } else if resp.hovered() {
            BORDER
        } else {
            PANEL
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        if selected {
            ui.painter().rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(1.0, ACCENT),
                egui::StrokeKind::Inside,
            );
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 1.0, rect.min.y + 8.0),
                egui::vec2(3.0, rect.height() - 16.0),
            );
            ui.painter().rect_filled(bar, 2.0, ACCENT);
        }

        let icon_color = if selected { TEXT } else { MUTED };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.min.x + 22.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        );
        paint_icon(ui.painter(), icon_rect, icon_color);

        if expanded {
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                egui::FontId::proportional(13.5),
                if selected { TEXT } else { MUTED },
            );
            let pos = egui::pos2(
                rect.min.x + 42.0,
                rect.center().y - galley.size().y * 0.5,
            );
            ui.painter()
                .galley(pos, galley, if selected { TEXT } else { MUTED });
        } else {
            resp.clone().on_hover_text(label);
        }

        if resp.clicked() {
            *tab = value;
        }
    });
}

fn footer(ui: &mut egui::Ui, expanded: &mut bool, ffmpeg_ok: bool) {
    let toggle_label = if *expanded { "«" } else { "»" };
    let toggle_hint = if *expanded {
        "Collapse sidebar"
    } else {
        "Expand sidebar"
    };

    if *expanded {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let c = if ffmpeg_ok { OK } else { BAD };
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, c);
            ui.label(RichText::new("ffmpeg").color(MUTED).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new(toggle_label).color(MUTED).size(14.0))
                            .fill(PANEL)
                            .frame(false),
                    )
                    .on_hover_text(toggle_hint)
                    .clicked()
                {
                    *expanded = false;
                }
            });
        });
    } else {
        ui.vertical_centered(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new(toggle_label).color(MUTED).size(14.0))
                        .fill(PANEL)
                        .frame(false),
                )
                .on_hover_text(toggle_hint)
                .clicked()
            {
                *expanded = true;
            }
            ui.add_space(8.0);
            let c = if ffmpeg_ok { OK } else { BAD };
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, c);
            ui.add_space(2.0);
            ui.label(RichText::new("ffmpeg").color(MUTED).size(9.0));
        });
    }
}

fn paint_browse(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    painter.rect_stroke(rect.shrink(1.0), 3.0, stroke, egui::StrokeKind::Outside);
    let c = rect.center();
    let pts = [
        egui::pos2(c.x - 3.0, c.y - 5.0),
        egui::pos2(c.x + 5.0, c.y),
        egui::pos2(c.x - 3.0, c.y + 5.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        pts.to_vec(),
        color,
        egui::Stroke::NONE,
    ));
}

fn paint_favorites(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    // Non-convex 5-point star via PathShape (convex_polygon would collapse to a pentagon).
    let c = rect.center();
    let outer = 8.0_f32;
    let inner = 3.4_f32;
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let a = -std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::PI / 5.0;
        let r = if i % 2 == 0 { outer } else { inner };
        pts.push(egui::pos2(c.x + a.cos() * r, c.y + a.sin() * r));
    }
    painter.add(egui::Shape::Path(egui::epaint::PathShape {
        points: pts,
        closed: true,
        fill: color,
        stroke: egui::Stroke::NONE.into(),
    }));
}

fn paint_queue(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    let x0 = rect.min.x + 2.0;
    let x1 = rect.max.x - 2.0;
    for (i, y) in [rect.min.y + 4.0, rect.center().y, rect.max.y - 4.0]
        .into_iter()
        .enumerate()
    {
        let inset = i as f32 * 1.5;
        painter.line_segment(
            [egui::pos2(x0 + inset, y), egui::pos2(x1 - inset, y)],
            stroke,
        );
    }
}

fn paint_settings(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    let c = rect.center();
    painter.circle_stroke(c, 6.5, stroke);
    painter.circle_filled(c, 2.2, color);
    for i in 0..6 {
        let a = (i as f32) * std::f32::consts::TAU / 6.0;
        let inner = c + egui::vec2(a.cos(), a.sin()) * 5.0;
        let outer = c + egui::vec2(a.cos(), a.sin()) * 8.5;
        painter.line_segment([inner, outer], stroke);
    }
}
