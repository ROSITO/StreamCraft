use egui::RichText;
use streamcraft_core::media::export_naming::ExportNamingPreset;
use streamcraft_core::store::prefs::{
    default_export_dir, AppPreferences, ProxyMode, VpnIndicatorFilter,
};

use crate::theme::{self, ACCENT, MUTED, OK, TEXT};

pub fn show(
    ui: &mut egui::Ui,
    prefs: &mut AppPreferences,
    ffmpeg_ok: bool,
    ffmpeg_source: &str,
    status: &mut String,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.heading(RichText::new("Settings").color(TEXT).size(22.0));
            });
            ui.add_space(16.0);

            section(ui, "Export", |ui| {
                ui.label(
                    RichText::new(prefs.export_dir.display().to_string())
                        .color(TEXT)
                        .monospace()
                        .size(12.0),
                );
                ui.horizontal(|ui| {
                    if theme::ghost_button(ui, "Choose…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            prefs.export_dir = path;
                            let _ = prefs.save();
                            *status = "Export folder updated".into();
                        }
                    }
                    if theme::ghost_button(ui, "Reset").clicked() {
                        prefs.export_dir = default_export_dir();
                        let _ = prefs.save();
                        *status = "Export folder reset".into();
                    }
                    if theme::ghost_button(ui, "Open").clicked() {
                        let _ = open::that(&prefs.export_dir);
                    }
                });
            });

            section(ui, "Downloads", |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Parallel segments").color(MUTED));
                    ui.add(egui::DragValue::new(&mut prefs.max_concurrency).range(1..=64));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Concurrent jobs").color(MUTED));
                    ui.add(egui::DragValue::new(&mut prefs.max_concurrent_jobs).range(1..=8));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Retries").color(MUTED));
                    ui.add(egui::DragValue::new(&mut prefs.max_retries).range(0..=12));
                });
                ui.checkbox(&mut prefs.ask_quality, "Ask quality before download");
            });

            section(ui, "Naming", |ui| {
                egui::ComboBox::from_id_salt("settings_naming")
                    .selected_text(prefs.naming_preset.label())
                    .show_ui(ui, |ui| {
                        for p in [
                            ExportNamingPreset::Flat,
                            ExportNamingPreset::JellyfinMovie,
                            ExportNamingPreset::JellyfinSeries,
                            ExportNamingPreset::Custom,
                        ] {
                            ui.selectable_value(&mut prefs.naming_preset, p, p.label());
                        }
                    });
                ui.label(RichText::new(prefs.naming_preset.hint()).color(MUTED).size(11.0));
                if prefs.naming_preset == ExportNamingPreset::Custom {
                    let mut tmpl = prefs
                        .naming_template
                        .clone()
                        .unwrap_or_else(|| ExportNamingPreset::Custom.template().into());
                    ui.add(
                        egui::TextEdit::singleline(&mut tmpl)
                            .desired_width(ui.available_width())
                            .hint_text("{title}.{ext}"),
                    );
                    prefs.naming_template = Some(tmpl);
                }
            });

            section(ui, "Browser", |ui| {
                ui.checkbox(&mut prefs.block_popups, "Block popups");
                ui.checkbox(&mut prefs.adblock, "Adblock rules");
                ui.checkbox(&mut prefs.block_redirects, "Block non-http redirects");
                ui.checkbox(&mut prefs.auto_enqueue_hls, "Auto-offer HLS downloads");
                ui.checkbox(&mut prefs.auto_enqueue_dash, "Auto-offer DASH downloads");
                ui.checkbox(
                    &mut prefs.auto_enqueue_progressive,
                    "Auto-offer progressive downloads",
                );
                ui.checkbox(
                    &mut prefs.drm_gate,
                    "Mark DRM streams (gate deferred to Phase 9)",
                );
            });

            section(ui, "Proxy", |ui| {
                egui::ComboBox::from_id_salt("proxy_mode")
                    .selected_text(format!("{:?}", prefs.proxy_mode))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut prefs.proxy_mode, ProxyMode::System, "System");
                        ui.selectable_value(&mut prefs.proxy_mode, ProxyMode::Http, "HTTP");
                        ui.selectable_value(&mut prefs.proxy_mode, ProxyMode::Socks5, "SOCKS5");
                    });
                if !matches!(prefs.proxy_mode, ProxyMode::System) {
                    ui.horizontal(|ui| {
                        ui.label("Host");
                        ui.add(
                            egui::TextEdit::singleline(&mut prefs.proxy_host).desired_width(180.0),
                        );
                        ui.label("Port");
                        ui.add(egui::DragValue::new(&mut prefs.proxy_port).range(1..=65535));
                    });
                    ui.horizontal(|ui| {
                        ui.label("User");
                        ui.add(
                            egui::TextEdit::singleline(&mut prefs.proxy_username)
                                .desired_width(120.0),
                        );
                        ui.label("Pass");
                        ui.add(
                            egui::TextEdit::singleline(&mut prefs.proxy_password)
                                .password(true)
                                .desired_width(120.0),
                        );
                    });
                }
            });

            section(ui, "VPN indicator", |ui| {
                egui::ComboBox::from_id_salt("vpn_filter")
                    .selected_text(format!("{:?}", prefs.vpn_indicator_filter))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut prefs.vpn_indicator_filter,
                            VpnIndicatorFilter::All,
                            "All",
                        );
                        ui.selectable_value(
                            &mut prefs.vpn_indicator_filter,
                            VpnIndicatorFilter::IgnoreTailscale,
                            "Ignore Tailscale",
                        );
                        ui.selectable_value(
                            &mut prefs.vpn_indicator_filter,
                            VpnIndicatorFilter::NordVpnOnly,
                            "NordVPN only",
                        );
                    });
            });

            section(ui, "FFmpeg", |ui| {
                let color = if ffmpeg_ok { OK } else { theme::BAD };
                ui.label(
                    RichText::new(if ffmpeg_ok {
                        format!("Ready ({ffmpeg_source})")
                    } else {
                        "Missing — run ./scripts/fetch-ffmpeg.sh".into()
                    })
                    .color(color),
                );
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                if theme::primary_button(ui, "Save").clicked() {
                    match prefs.save() {
                        Ok(()) => *status = "Preferences saved".into(),
                        Err(e) => *status = format!("Save failed: {e}"),
                    }
                }
            });
            ui.add_space(32.0);
        });
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).color(ACCENT).size(13.0).strong());
            ui.add_space(8.0);
            add(ui);
            ui.add_space(18.0);
        });
    });
}
