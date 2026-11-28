use egui::RichText;
use streamcraft_core::media::models::DetectedMedia;

use crate::detect;
use crate::theme::{self, ACCENT, BORDER, MUTED, PANEL, TEXT};

pub enum DetectedAction {
    None,
    OpenDownload(usize),
    Clear,
}

pub fn show(ui: &mut egui::Ui, detected: &[DetectedMedia]) -> DetectedAction {
    let mut action = DetectedAction::None;
    ui.painter().rect_filled(ui.max_rect(), 0.0, PANEL);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.heading(RichText::new("Detected").color(TEXT).size(18.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(12.0);
            if theme::ghost_button(ui, "Clear").clicked() {
                action = DetectedAction::Clear;
            }
        });
    });
    ui.add_space(8.0);
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if detected.is_empty() {
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Play a video — streams appear here.")
                            .color(MUTED)
                            .size(13.0),
                    );
                });
                return;
            }
            for (i, m) in detected.iter().enumerate() {
                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(crate::theme::ELEVATED)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .corner_radius(10.0)
                    .inner_margin(12.0)
                    .outer_margin(egui::Margin::symmetric(12, 0))
                    .show(ui, |ui| {
                        // Every card spans the panel; long text is cut, not widening it.
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(detect::kind_label(m.kind))
                                .color(ACCENT)
                                .size(11.0)
                                .strong(),
                        );
                        if let Some(t) = &m.suggested_title {
                            ui.add(
                                egui::Label::new(RichText::new(t).color(TEXT).size(13.0).strong())
                                    .truncate(),
                            );
                        }
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(m.source_url.as_str())
                                    .color(MUTED)
                                    .monospace()
                                    .size(11.0),
                            )
                            .truncate(), // egui shows the full URL on hover
                        );
                        if let Some(n) = m.segment_count {
                            ui.label(RichText::new(format!("{n} segments")).color(MUTED).size(11.0));
                        }
                        ui.add_space(8.0);
                        if theme::primary_button(ui, "Download").clicked() {
                            action = DetectedAction::OpenDownload(i);
                        }
                    });
            }
        });
    action
}
