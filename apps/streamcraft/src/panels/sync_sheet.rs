//! "Ajuster sync" sheet — port of Swift `AVSyncSheet`: diagnostics, player-like
//! realign, linear drift (atempo), constant offset, and auto repair.

use std::path::PathBuf;

use egui::RichText;
use streamcraft_ffmpeg::MediaStreamTiming;

use crate::theme::{self, ACCENT, BAD, BORDER, MUTED, PANEL, TEXT};

const LEAD_RANGE: std::ops::RangeInclusive<f64> = -3.0..=5.0;
const SHIFT_RANGE: std::ops::RangeInclusive<f64> = -2.0..=2.0;
const LEAD_PRESETS: [f64; 5] = [0.5, 1.0, 1.5, 2.0, 3.0];
const SHIFT_PRESETS: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];

pub struct SyncSheet {
    pub path: PathBuf,
    pub title: String,
    pub timing: Option<MediaStreamTiming>,
    /// Audio lead at the end of the file, seconds (+ = sound early at the end).
    pub end_lead: f64,
    /// Constant audio shift, seconds (+ = delay audio).
    pub shift: f64,
    pub busy: bool,
    pub status: Option<String>,
}

impl SyncSheet {
    pub fn new(path: PathBuf, title: String) -> Self {
        Self {
            path,
            title,
            timing: None,
            end_lead: 1.0,
            shift: 0.0,
            busy: false,
            status: None,
        }
    }
}

pub enum SyncOp {
    PlayerAlign,
    Drift(f64),
    Shift(f64),
    Auto,
}

pub enum SyncAction {
    None,
    Close,
    Run(SyncOp),
}

fn ms(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

pub fn show(ui: &mut egui::Ui, sheet: &mut SyncSheet) -> SyncAction {
    let mut action = SyncAction::None;
    let full = ui.max_rect();
    ui.painter()
        .rect_filled(full, 0.0, egui::Color32::from_black_alpha(160));
    let panel_w = 460.0_f32.min(full.width() - 40.0);
    let panel_h = 600.0_f32.min(full.height() - 40.0);
    let panel = egui::Rect::from_center_size(full.center(), egui::vec2(panel_w, panel_h));

    egui::Area::new(egui::Id::unique(egui::IdSalt::new("sync_sheet")))
        .fixed_pos(panel.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(12.0)
                .inner_margin(18.0)
                .show(ui, |ui| {
                    ui.set_width(panel_w - 36.0);
                    ui.set_max_height(panel_h - 36.0);
                    ui.heading(RichText::new("Ajuster sync").color(TEXT).size(20.0));
                    ui.add(egui::Label::new(RichText::new(&sheet.title).color(TEXT)).truncate());
                    let file = sheet
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    ui.add(egui::Label::new(RichText::new(file).color(MUTED).size(11.0)).truncate());
                    ui.add_space(6.0);
                    match &sheet.timing {
                        Some(t) => {
                            let start = match (t.video_start, t.audio_start) {
                                (Some(v), Some(a)) => ms((v - a).abs()),
                                _ => 0,
                            };
                            let dur = ms(t.duration_delta().unwrap_or(0.0).abs());
                            ui.label(
                                RichText::new(format!("Écart départ ~{start} ms · écart durée ~{dur} ms"))
                                    .color(MUTED)
                                    .size(12.0),
                            );
                            if let Some(w) = streamcraft_core::assemble::av_sync::mismatch_warning(t) {
                                ui.add(egui::Label::new(RichText::new(w).color(BAD).size(12.0)).wrap());
                            }
                        }
                        None => {
                            ui.label(RichText::new("Analyse…").color(MUTED).size(12.0));
                        }
                    }
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(panel_h - 220.0)
                        .show(ui, |ui| {
                            section(ui, "Recommandé");
                            hint(ui, "Si le son part juste puis dérive : timeline vidéo étirée (trous PTS). « Recaler » compacte la timeline et accélère l’audio d’autant ; sinon il rogne la piste la plus longue, comme le lecteur.");
                            if let Some(skew) = sheet
                                .timing
                                .as_ref()
                                .and_then(MediaStreamTiming::video_timeline_skew_ratio)
                                .filter(|s| (s - 1.0).abs() > 0.002)
                            {
                                ui.label(
                                    RichText::new(format!("Timeline vidéo anormale : ×{skew:.4}"))
                                        .color(BAD)
                                        .size(12.0)
                                        .strong(),
                                );
                            }
                            ui.add_enabled_ui(!sheet.busy, |ui| {
                                if theme::primary_button(ui, "Recaler (skew / piste courte)").clicked() {
                                    action = SyncAction::Run(SyncOp::PlayerAlign);
                                }
                            });
                            hint(ui, "Les fichiers déjà « réparés » plusieurs fois restent souvent irrécupérables — préfère un export neuf.");

                            ui.add_space(12.0);
                            section(ui, "Dérive linéaire (son qui s’éloigne à rythme constant)");
                            let lead = ms(sheet.end_lead);
                            ui.label(
                                RichText::new(format!("{}{lead} ms en fin", if lead > 0 { "+" } else { "" }))
                                    .color(TEXT)
                                    .size(16.0)
                                    .strong(),
                            );
                            ui.add(
                                egui::Slider::new(&mut sheet.end_lead, LEAD_RANGE)
                                    .step_by(0.1)
                                    .suffix(" s"),
                            );
                            hint(
                                ui,
                                &match lead {
                                    l if l > 0 => format!("Son en avance de {l} ms à la fin → on ralentit progressivement l’audio."),
                                    l if l < 0 => format!("Son en retard de {} ms à la fin → on accélère progressivement l’audio.", -l),
                                    _ => "Regarde la fin de l’épisode : si le son a ~1 s d’avance, mets +1,0 s.".into(),
                                },
                            );
                            ui.horizontal_wrapped(|ui| {
                                for v in LEAD_PRESETS {
                                    if theme::ghost_button(ui, &format!("+{v:.1}s")).clicked() {
                                        sheet.end_lead = v;
                                    }
                                }
                            });
                            ui.add_enabled_ui(!sheet.busy && lead != 0, |ui| {
                                if theme::primary_button(ui, "Corriger cette dérive").clicked() {
                                    action = SyncAction::Run(SyncOp::Drift(sheet.end_lead));
                                }
                            });

                            ui.add_space(12.0);
                            section(ui, "Décalage constant");
                            let shift = ms(sheet.shift);
                            ui.label(
                                RichText::new(format!("{}{shift} ms", if shift > 0 { "+" } else { "" }))
                                    .color(TEXT)
                                    .size(16.0)
                                    .strong(),
                            );
                            ui.add(
                                egui::Slider::new(&mut sheet.shift, SHIFT_RANGE)
                                    .step_by(0.05)
                                    .suffix(" s"),
                            );
                            hint(ui, "Écart fixe restant après le recalage (son toujours en avance/retard d’autant partout). + = retarder l’audio.");
                            ui.horizontal_wrapped(|ui| {
                                for v in SHIFT_PRESETS {
                                    let label = match ms(v) {
                                        0 => "0".to_string(),
                                        m if m > 0 => format!("+{m}"),
                                        m => m.to_string(),
                                    };
                                    if theme::ghost_button(ui, &label).clicked() {
                                        sheet.shift = v;
                                    }
                                }
                            });
                            ui.add_enabled_ui(!sheet.busy && shift != 0, |ui| {
                                if theme::primary_button(ui, "Appliquer ce décalage fixe").clicked() {
                                    action = SyncAction::Run(SyncOp::Shift(sheet.shift));
                                }
                            });
                        });

                    ui.separator();
                    if let Some(s) = &sheet.status {
                        ui.add(egui::Label::new(RichText::new(s).color(ACCENT).size(12.0)).wrap());
                    }
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!sheet.busy, |ui| {
                            if theme::ghost_button(ui, "Réparation auto (PTS)").clicked() {
                                action = SyncAction::Run(SyncOp::Auto);
                            }
                        });
                        if sheet.busy {
                            ui.spinner();
                        }
                        if theme::ghost_button(ui, "Fermer").clicked() {
                            action = SyncAction::Close;
                        }
                    });
                });
        });
    action
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).color(TEXT).size(13.0).strong());
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(MUTED).size(11.0)).wrap());
}
