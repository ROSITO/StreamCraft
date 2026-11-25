//! Download confirmation sheet — quality / naming (egui ink-studio).
//! Opened from `app.rs` / `detected.rs` when user picks a DetectedMedia.

use egui::RichText;
use streamcraft_core::media::export_naming::{ExportMetadata, ExportNaming, ExportNamingPreset};
use streamcraft_core::media::models::DetectedMedia;
use streamcraft_core::store::prefs::AppPreferences;
use uuid::Uuid;

use crate::detect;
use crate::theme::{self, ACCENT, BAD, BORDER, ELEVATED, MUTED, PANEL, TEXT};

pub struct DownloadSheet {
    pub media: DetectedMedia,
    pub metadata: ExportMetadata,
    pub variant_id: Option<Uuid>,
    pub audio_id: Option<Uuid>,
    pub subtitle_id: Option<Uuid>,
}

impl DownloadSheet {
    pub fn new(media: DetectedMedia, prefs: &AppPreferences) -> Self {
        let title = media
            .suggested_title
            .clone()
            .unwrap_or_else(|| "video".into());
        let variant_id = media.preferred_variant_id.or_else(|| {
            media.variants.first().map(|v| v.id)
        });
        let audio_id = media
            .audio_tracks
            .iter()
            .find(|t| t.is_default)
            .or_else(|| media.audio_tracks.first())
            .map(|t| t.id);
        Self {
            media,
            metadata: ExportMetadata {
                title,
                year: None,
                show: None,
                season: None,
                episode: None,
                episode_title: None,
                naming_preset: Some(prefs.naming_preset),
            },
            variant_id,
            audio_id,
            subtitle_id: None,
        }
    }
}

pub enum SheetAction {
    None,
    Cancel,
    Confirm,
}

pub fn show(
    ui: &mut egui::Ui,
    sheet: &mut DownloadSheet,
    prefs: &AppPreferences,
) -> SheetAction {
    let mut action = SheetAction::None;
    let full = ui.max_rect();
    ui.painter()
        .rect_filled(full, 0.0, egui::Color32::from_black_alpha(160));

    let panel_w = 420.0_f32.min(full.width() - 40.0);
    let panel_h = 520.0_f32.min(full.height() - 40.0);
    let panel = egui::Rect::from_center_size(full.center(), egui::vec2(panel_w, panel_h));

    egui::Area::new(egui::Id::unique(egui::IdSalt::new("download_sheet")))
        .fixed_pos(panel.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(12.0)
                .inner_margin(18.0)
                .show(ui, |ui| {
                    ui.set_min_size(egui::vec2(panel_w - 4.0, panel_h - 4.0));
                    ui.heading(RichText::new("Download").color(TEXT).size(20.0));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(detect::kind_label(sheet.media.kind))
                            .color(ACCENT)
                            .size(12.0)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(sheet.media.source_url.as_str())
                            .color(MUTED)
                            .monospace()
                            .size(11.0),
                    );
                    if matches!(
                        sheet.media.protection,
                        streamcraft_core::MediaProtection::Drm { .. }
                    ) {
                        ui.label(RichText::new("DRM detected").color(BAD).size(12.0));
                    }
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    egui::ScrollArea::vertical()
                        .max_height(panel_h - 160.0)
                        .show(ui, |ui| {
                            field(ui, "Title", &mut sheet.metadata.title);
                            ui.add_space(6.0);
                            let mut year = sheet.metadata.year.clone().unwrap_or_default();
                            field(ui, "Year", &mut year);
                            sheet.metadata.year = if year.trim().is_empty() {
                                None
                            } else {
                                Some(year)
                            };

                            ui.add_space(10.0);
                            ui.label(RichText::new("Naming").color(MUTED).size(11.0));
                            let presets = [
                                ExportNamingPreset::Flat,
                                ExportNamingPreset::JellyfinMovie,
                                ExportNamingPreset::JellyfinSeries,
                                ExportNamingPreset::Custom,
                            ];
                            let mut preset = sheet
                                .metadata
                                .naming_preset
                                .unwrap_or(prefs.naming_preset);
                            egui::ComboBox::from_id_salt("naming_preset")
                                .selected_text(preset.label())
                                .show_ui(ui, |ui| {
                                    for p in presets {
                                        ui.selectable_value(&mut preset, p, p.label());
                                    }
                                });
                            sheet.metadata.naming_preset = Some(preset);
                            if preset == ExportNamingPreset::JellyfinSeries {
                                ui.add_space(6.0);
                                let mut show = sheet.metadata.show.clone().unwrap_or_default();
                                let mut season = sheet.metadata.season.clone().unwrap_or_default();
                                let mut episode = sheet.metadata.episode.clone().unwrap_or_default();
                                field(ui, "Show", &mut show);
                                field(ui, "Season", &mut season);
                                field(ui, "Episode", &mut episode);
                                sheet.metadata.show = nonempty(show);
                                sheet.metadata.season = nonempty(season);
                                sheet.metadata.episode = nonempty(episode);
                            }

                            if !sheet.media.variants.is_empty() {
                                ui.add_space(10.0);
                                ui.label(RichText::new("Quality").color(MUTED).size(11.0));
                                let current = sheet
                                    .variant_id
                                    .and_then(|id| {
                                        sheet
                                            .media
                                            .variants
                                            .iter()
                                            .find(|v| v.id == id)
                                            .map(|v| v.display_label())
                                    })
                                    .unwrap_or_else(|| "Auto".into());
                                egui::ComboBox::from_id_salt("variant")
                                    .selected_text(current)
                                    .show_ui(ui, |ui| {
                                        for v in &sheet.media.variants {
                                            ui.selectable_value(
                                                &mut sheet.variant_id,
                                                Some(v.id),
                                                v.display_label(),
                                            );
                                        }
                                    });
                            }

                            if !sheet.media.audio_tracks.is_empty() {
                                ui.add_space(8.0);
                                ui.label(RichText::new("Audio").color(MUTED).size(11.0));
                                let label = sheet
                                    .audio_id
                                    .and_then(|id| {
                                        sheet.media.audio_tracks.iter().find(|t| t.id == id)
                                    })
                                    .map(track_label)
                                    .unwrap_or_else(|| "Default".into());
                                egui::ComboBox::from_id_salt("audio")
                                    .selected_text(label)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut sheet.audio_id, None, "Embedded");
                                        for t in &sheet.media.audio_tracks {
                                            ui.selectable_value(
                                                &mut sheet.audio_id,
                                                Some(t.id),
                                                track_label(t),
                                            );
                                        }
                                    });
                            }

                            if !sheet.media.subtitle_tracks.is_empty() {
                                ui.add_space(8.0);
                                ui.label(RichText::new("Subtitles").color(MUTED).size(11.0));
                                let label = sheet
                                    .subtitle_id
                                    .and_then(|id| {
                                        sheet.media.subtitle_tracks.iter().find(|t| t.id == id)
                                    })
                                    .map(track_label)
                                    .unwrap_or_else(|| "None".into());
                                egui::ComboBox::from_id_salt("subs")
                                    .selected_text(label)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut sheet.subtitle_id, None, "None");
                                        for t in &sheet.media.subtitle_tracks {
                                            ui.selectable_value(
                                                &mut sheet.subtitle_id,
                                                Some(t.id),
                                                track_label(t),
                                            );
                                        }
                                    });
                            }

                            ui.add_space(12.0);
                            let preset = sheet
                                .metadata
                                .naming_preset
                                .unwrap_or(prefs.naming_preset);
                            let template = if preset == ExportNamingPreset::Custom {
                                prefs.resolved_naming_template()
                            } else {
                                preset.template().to_string()
                            };
                            let ctx = sheet.metadata.clone().into_context(
                                detect::kind_label(sheet.media.kind),
                                "mp4",
                            );
                            let rel = ExportNaming::relative_path(&template, &ctx);
                            let preview = prefs.export_dir.join(&rel);
                            ui.label(RichText::new("Export path").color(MUTED).size(11.0));
                            ui.label(
                                RichText::new(preview.display().to_string())
                                    .color(TEXT)
                                    .monospace()
                                    .size(11.0),
                            );
                        });

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::ghost_button(ui, "Cancel").clicked() {
                            action = SheetAction::Cancel;
                        }
                        ui.add_space(8.0);
                        if theme::primary_button(ui, "Start download").clicked() {
                            action = SheetAction::Confirm;
                        }
                    });
                });
        });

    // Swallow clicks behind sheet
    let _ = ui.interact(full, ui.scope_id().with("sheet_dim"), egui::Sense::click());
    action
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(RichText::new(label).color(MUTED).size(11.0));
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(ui.available_width())
            .margin(egui::Margin::symmetric(10, 6)),
    );
}

fn nonempty(s: String) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

fn track_label(t: &streamcraft_core::media::models::MediaTrack) -> String {
    t.name
        .clone()
        .or_else(|| t.language.clone())
        .unwrap_or_else(|| t.id.to_string())
}

#[allow(dead_code)]
fn _elevated() -> egui::Color32 {
    ELEVATED
}
