//! Favorites / Annuaire — parity with Tauri `BookmarkManagerSheet`.
//! Persists via `streamcraft_core::store::bookmarks::BookmarkStore`
//! (`~/Library/Application Support/StreamCraft/bookmarks.json`).
//!
//! Called from `apps/streamcraft/src/app.rs` (`NavTab::Favorites` + star in browse chrome).
//! No Craft panel existed for this; core store already powers CLI flix/season.
//! User: "il manque la gestion des favoris"

use egui::RichText;
use streamcraft_core::store::bookmarks::BookmarkStore;
use uuid::Uuid;

use crate::theme::{self, ACCENT, BAD, BORDER, ELEVATED, MUTED, TEXT};

#[derive(Default)]
pub struct BookmarksUi {
    pub adding: bool,
    pub add_title: String,
    pub add_url: String,
    pub as_hub: bool,
    pub editing_id: Option<Uuid>,
    pub edit_draft: String,
    pub error: Option<String>,
    pub rename_focus: bool,
}

pub enum BookmarksAction {
    None,
    /// Open URL in Browse (`mark_visited` already applied).
    Open { url: String },
}

pub fn show(
    ui: &mut egui::Ui,
    store: &mut BookmarkStore,
    state: &mut BookmarksUi,
    current_url: &str,
    current_title: &str,
) -> BookmarksAction {
    let mut action = BookmarksAction::None;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.heading(RichText::new("Favorites").color(TEXT).size(22.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(24.0);
                    if state.adding {
                        if theme::ghost_button(ui, "Cancel").clicked() {
                            state.adding = false;
                            state.error = None;
                        }
                    } else if theme::primary_button(ui, "+ Add site").clicked() {
                        begin_add(state);
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(
                    RichText::new("Hubs power flix digest & season-by-name. Same file as the CLI.")
                        .color(MUTED)
                        .size(12.0),
                );
            });
            ui.add_space(16.0);

            if state.adding {
                add_form(ui, store, state, current_url, current_title);
            } else if store.items.is_empty() {
                empty_state(ui, state);
            } else {
                list(ui, store, state, &mut action);
            }
        });

    action
}

/// Empty form: the star in the Browse bar adds the current page, and the form's
/// "Use current page" button fills it on demand.
fn begin_add(state: &mut BookmarksUi) {
    state.adding = true;
    state.add_title.clear();
    state.add_url.clear();
    state.as_hub = true;
    state.error = None;
}

fn coerce_https(url: &str) -> String {
    let url = url.trim().to_string();
    if url.is_empty() || url.contains("://") {
        url
    } else {
        format!("https://{url}")
    }
}

fn empty_state(ui: &mut egui::Ui, state: &mut BookmarksUi) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("No favorites yet").color(TEXT).size(16.0).strong());
            ui.label(
                RichText::new("Add hubs or useful pages. No embedded catalogue.")
                    .color(MUTED)
                    .size(13.0),
            );
            ui.add_space(12.0);
            if theme::primary_button(ui, "Add a site…").clicked() {
                begin_add(state);
            }
        });
    });
}

fn add_form(
    ui: &mut egui::Ui,
    store: &mut BookmarkStore,
    state: &mut BookmarksUi,
    current_url: &str,
    current_title: &str,
) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.set_max_width(520.0);
            ui.label(RichText::new("Display name").color(MUTED).size(12.0));
            let title_resp = ui.add(
                egui::TextEdit::singleline(&mut state.add_title)
                    .desired_width(f32::INFINITY)
                    .hint_text("e.g. My hub"),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("URL").color(MUTED).size(12.0));
            let url_resp = ui.add(
                egui::TextEdit::singleline(&mut state.add_url)
                    .desired_width(f32::INFINITY)
                    .hint_text("https://…"),
            );
            ui.add_space(8.0);
            ui.checkbox(
                &mut state.as_hub,
                RichText::new("Save as hub (homepage)").color(TEXT),
            );
            ui.label(
                RichText::new(if state.as_hub {
                    "Only the origin is kept (scheme + host)."
                } else {
                    "Exact page URL."
                })
                .color(MUTED)
                .size(11.0),
            );
            ui.add_space(8.0);
            if theme::ghost_button(ui, "Use current page").clicked() && !current_url.is_empty() {
                state.add_url = current_url.to_string();
                if state.add_title.trim().is_empty() {
                    state.add_title = current_title.trim().to_string();
                }
            }
            if let Some(err) = &state.error {
                ui.add_space(6.0);
                ui.label(RichText::new(err).color(BAD).size(12.0));
            }
            ui.add_space(12.0);
            let submit = theme::primary_button(ui, "Add").clicked()
                || ((title_resp.lost_focus() || url_resp.lost_focus() || title_resp.has_focus() || url_resp.has_focus())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if submit {
                submit_add(store, state);
            }
        });
    });
}

fn submit_add(store: &mut BookmarkStore, state: &mut BookmarksUi) {
    let mut url = coerce_https(&state.add_url);
    if state.as_hub {
        if let Some(hub) = BookmarkStore::hub_origin_url(&url) {
            url = hub;
        }
    }
    match store.add(state.add_title.clone(), url) {
        Ok(_) => {
            state.adding = false;
            state.error = None;
        }
        Err(e) => state.error = Some(e),
    }
}

fn list(
    ui: &mut egui::Ui,
    store: &mut BookmarkStore,
    state: &mut BookmarksUi,
    action: &mut BookmarksAction,
) {
    let items: Vec<_> = store.items.clone();
    for b in items {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            egui::Frame::new()
                .fill(ELEVATED)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width((ui.available_width() - 24.0).max(280.0));
                    ui.horizontal(|ui| {
                        if state.editing_id == Some(b.id) {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut state.edit_draft)
                                    .desired_width(220.0),
                            );
                            if state.rename_focus {
                                resp.request_focus();
                                state.rename_focus = false;
                            }
                            if resp.lost_focus()
                                || (resp.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                match store.rename(b.id, state.edit_draft.clone()) {
                                    Ok(()) => {
                                        state.editing_id = None;
                                        state.error = None;
                                    }
                                    Err(e) => state.error = Some(e),
                                }
                            }
                        } else {
                            ui.label(
                                RichText::new(b.display_title())
                                    .color(TEXT)
                                    .size(15.0)
                                    .strong(),
                            );
                        }
                        if b.is_hub() {
                            ui.label(
                                RichText::new("Hub")
                                    .color(ACCENT)
                                    .size(11.0)
                                    .strong(),
                            );
                        }
                    });
                    ui.label(RichText::new(&b.url).color(MUTED).size(12.0).monospace());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if theme::ghost_button(ui, "Open").clicked() {
                            match store.mark_visited(b.id) {
                                Ok(()) => {
                                    state.editing_id = None;
                                    *action = BookmarksAction::Open { url: b.url.clone() };
                                }
                                Err(e) => state.error = Some(e),
                            }
                        }
                        if theme::ghost_button(ui, "Rename").clicked() {
                            state.editing_id = Some(b.id);
                            state.edit_draft = b.title.clone();
                            state.rename_focus = true;
                            state.error = None;
                        }
                        if theme::ghost_button(ui, "Delete").clicked() {
                            match store.remove(b.id) {
                                Ok(()) => {
                                    if state.editing_id == Some(b.id) {
                                        state.editing_id = None;
                                    }
                                    state.error = None;
                                }
                                Err(e) => state.error = Some(e),
                            }
                        }
                    });
                });
        });
        ui.add_space(8.0);
    }

    if let Some(err) = &state.error {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(err).color(BAD).size(12.0));
        });
        ui.add_space(6.0);
    }

    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.label(
            RichText::new(format!(
                "{} site{} · {}",
                store.items.len(),
                if store.items.len() == 1 { "" } else { "s" },
                BookmarkStore::path().display()
            ))
            .color(MUTED)
            .size(11.0),
        );
    });
}
