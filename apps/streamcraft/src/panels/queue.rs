use egui::RichText;
use streamcraft_core::media::models::{DownloadJob, DownloadState};
use uuid::Uuid;

use crate::theme::{self, ACCENT, BAD, MUTED, OK, TEXT};

pub enum QueueAction {
    None,
    Cancel(Uuid),
    Resume(Uuid),
    Assemble(Uuid),
    Remove(Uuid),
    Open(std::path::PathBuf),
    /// Open the "Ajuster sync" sheet on an exported file.
    Sync(std::path::PathBuf, String),
}

pub fn show(ui: &mut egui::Ui, jobs: &[DownloadJob]) -> QueueAction {
    let mut action = QueueAction::None;
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.heading(RichText::new("Queue").color(TEXT).size(22.0));
    });
    ui.add_space(12.0);
    if jobs.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new("Nothing downloading.").color(MUTED));
        });
        return action;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for j in jobs {
            // Vertical card with side margins: a horizontal wrapper laid every field
            // on one line, pushing the progress bar and buttons off-screen.
            egui::Frame::new()
                .fill(crate::theme::ELEVATED)
                .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                .corner_radius(10.0)
                .inner_margin(14.0)
                .outer_margin(egui::Margin::symmetric(24, 0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let state_color = match j.state {
                        DownloadState::Completed => OK,
                        DownloadState::Failed => BAD,
                        DownloadState::Cancelled => MUTED,
                        _ => ACCENT,
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:?}", j.state))
                                .color(state_color)
                                .size(11.0)
                                .strong(),
                        );
                        if let Some(t) = &j.media.suggested_title {
                            ui.add(
                                egui::Label::new(RichText::new(t).color(TEXT).size(13.0))
                                    .truncate(),
                            );
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(j.media.source_url.as_str())
                                .color(MUTED)
                                .monospace()
                                .size(11.0),
                        )
                        .truncate(),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(j.progress as f32)
                            .desired_width(ui.available_width())
                            .show_percentage(),
                    );
                    if let Some(err) = &j.error_message {
                        ui.add(egui::Label::new(RichText::new(err).color(BAD).size(11.0)).wrap());
                    }
                    if let Some(p) = &j.destination {
                        ui.add(
                            egui::Label::new(
                                RichText::new(p.display().to_string())
                                    .color(MUTED)
                                    .size(11.0),
                            )
                            .truncate(),
                        );
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| match j.state {
                        DownloadState::Queued
                        | DownloadState::Downloading
                        | DownloadState::Assembling => {
                            if theme::ghost_button(ui, "Cancel").clicked() {
                                action = QueueAction::Cancel(j.id);
                            }
                        }
                        DownloadState::Failed | DownloadState::Cancelled => {
                            if theme::ghost_button(ui, "Resume").clicked() {
                                action = QueueAction::Resume(j.id);
                            }
                            if theme::ghost_button(ui, "Assemble").clicked() {
                                action = QueueAction::Assemble(j.id);
                            }
                            if theme::ghost_button(ui, "Remove").clicked() {
                                action = QueueAction::Remove(j.id);
                            }
                        }
                        DownloadState::Completed => {
                            if let Some(p) = &j.destination
                                && theme::primary_button(ui, "Open").clicked()
                            {
                                action = QueueAction::Open(p.clone());
                            }
                            if let Some(p) = &j.destination
                                && theme::ghost_button(ui, "Sync…").clicked()
                            {
                                action = QueueAction::Sync(p.clone(), j.media.display_title());
                            }
                            if theme::ghost_button(ui, "Remove").clicked() {
                                action = QueueAction::Remove(j.id);
                            }
                        }
                    });
                });
            ui.add_space(10.0);
        }
    });
    action
}

