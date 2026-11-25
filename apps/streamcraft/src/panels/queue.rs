use std::sync::Arc;

use egui::RichText;
use streamcraft_core::download::queue::DownloadQueue;
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
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                egui::Frame::new()
                    .fill(crate::theme::ELEVATED)
                    .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                    .corner_radius(10.0)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.set_min_width((ui.available_width() - 48.0).max(200.0));
                        let state_color = match j.state {
                            DownloadState::Completed => OK,
                            DownloadState::Failed => BAD,
                            DownloadState::Cancelled => MUTED,
                            _ => ACCENT,
                        };
                        ui.label(
                            RichText::new(format!("{:?}", j.state))
                                .color(state_color)
                                .size(11.0)
                                .strong(),
                        );
                        if let Some(t) = &j.media.suggested_title {
                            ui.label(RichText::new(t).color(TEXT).size(13.0));
                        }
                        ui.label(
                            RichText::new(j.media.source_url.as_str())
                                .color(MUTED)
                                .monospace()
                                .size(11.0),
                        );
                        ui.add(
                            egui::ProgressBar::new(j.progress as f32)
                                .desired_width(280.0)
                                .show_percentage(),
                        );
                        if let Some(err) = &j.error_message {
                            ui.label(RichText::new(err).color(BAD).size(11.0));
                        }
                        if let Some(p) = &j.destination {
                            ui.label(
                                RichText::new(p.display().to_string())
                                    .color(MUTED)
                                    .size(11.0),
                            );
                        }
                        ui.horizontal(|ui| {
                            match j.state {
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
                                    if let Some(p) = &j.destination {
                                        if theme::primary_button(ui, "Open").clicked() {
                                            action = QueueAction::Open(p.clone());
                                        }
                                    }
                                    if theme::ghost_button(ui, "Remove").clicked() {
                                        action = QueueAction::Remove(j.id);
                                    }
                                }
                            }
                        });
                    });
            });
            ui.add_space(10.0);
        }
    });
    action
}

#[allow(dead_code)]
pub fn _queue_type(_: &Arc<DownloadQueue>) {}
