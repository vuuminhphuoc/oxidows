//! Drawing: toolbar with address bar, sidebar, virtualised file table, status bar, dialogs.

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use explorer_core::{SortKey, breadcrumbs, format_bytes, format_size_kb, format_time, icon_for};
use explorer_sys::DriveKind;

use crate::{App, Dialog};

pub fn toolbar(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(app.history.can_back(), egui::Button::new("⬅")).on_hover_text("Back (Alt+Left)").clicked() {
                app.back();
            }
            if ui
                .add_enabled(app.history.can_forward(), egui::Button::new("➡"))
                .on_hover_text("Forward (Alt+Right)")
                .clicked()
            {
                app.forward();
            }
            if ui.add_enabled(app.cur.parent().is_some(), egui::Button::new("⬆")).on_hover_text("Up (Alt+Up)").clicked() {
                app.up();
            }
            if ui.button("⟳").on_hover_text("Refresh (F5)").clicked() {
                app.refresh();
                app.load_drives();
            }

            // Search box on the right, address bar takes the rest.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.filter).hint_text("🔍 Search this folder").desired_width(200.0),
                );
                if std::mem::take(&mut app.focus_search) {
                    search.request_focus();
                }
                if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.filter.clear();
                    search.surrender_focus();
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| address_bar(app, ui));
            });
        });
        ui.add_space(2.0);
    });
}

fn address_bar(app: &mut App, ui: &mut Ui) {
    let width = ui.available_width();
    if let Some(text) = app.address_edit.as_mut() {
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(width - 8.0));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() {
            let submit = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let text = app.address_edit.take().unwrap_or_default();
            if submit {
                app.submit_address(&text);
            }
        }
        return;
    }
    // Breadcrumbs; clicking the empty part of the bar switches to typing a path.
    let frame = egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.inactive.bg_stroke)
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(4, 1));
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            let crumbs = breadcrumbs(&app.cur);
            // Long paths: show the last segments that fit, like Explorer.
            let skip = crumbs.len().saturating_sub(((width / 90.0) as usize).max(2));
            let mut go = None;
            if skip > 0 {
                ui.label(" … ");
            }
            for (i, (label, path)) in crumbs.iter().enumerate().skip(skip) {
                if i > skip {
                    ui.label(RichText::new(" › ").weak());
                }
                if ui.add(egui::Button::new(label).frame(false)).clicked() {
                    go = Some(path.clone());
                }
            }
            go
        });
    if let Some(p) = resp.inner {
        app.navigate(p);
    } else if resp.response.interact(Sense::click()).clicked() {
        app.address_edit = Some(app.cur.to_string_lossy().into_owned());
    }
}

pub fn sidebar(app: &mut App, ui: &mut Ui) {
    egui::Panel::left("sidebar").resizable(true).default_size(190.0).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(6.0);
            let mut go: Option<PathBuf> = None;
            let place = |ui: &mut Ui, icon: &str, name: &str, path: &PathBuf, cur: &PathBuf| {
                let selected = path == cur;
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    ui.add(egui::Button::selectable(selected, format!("{icon}  {name}")).min_size(egui::vec2(0.0, 22.0)))
                        .clicked()
                })
                .inner
            };
            if place(ui, "🏠", "Home", &app.home, &app.cur) {
                go = Some(app.home.clone());
            }
            for (name, path) in &app.places {
                let icon = match *name {
                    "Desktop" => "🖥",
                    "Downloads" => "⬇",
                    "Documents" => "📄",
                    "Pictures" => "🖼",
                    "Music" => "🎵",
                    "Videos" => "🎞",
                    _ => "📁",
                };
                if place(ui, icon, name, path, &app.cur) {
                    go = Some(path.clone());
                }
            }
            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("This PC").weak());
            for d in &app.drives {
                let root = d.root.to_string_lossy();
                let letter = root.trim_end_matches('\\');
                let label = match (d.label.is_empty(), d.kind) {
                    (false, _) => format!("{} ({letter})", d.label),
                    (true, DriveKind::Fixed) => format!("Local Disk ({letter})"),
                    (true, DriveKind::Removable) => format!("USB Drive ({letter})"),
                    (true, DriveKind::Network) => format!("Network ({letter})"),
                    (true, DriveKind::Optical) => format!("DVD Drive ({letter})"),
                    (true, DriveKind::Other) => letter.to_string(),
                };
                let icon = match d.kind {
                    DriveKind::Network => "🖧",
                    DriveKind::Optical => "💿",
                    DriveKind::Removable => "💾",
                    _ => "🖴",
                };
                let r = place(ui, icon, &label, &d.root, &app.cur);
                if r {
                    go = Some(d.root.clone());
                }
                if d.total > 0 {
                    let used = 1.0 - d.free as f32 / d.total as f32;
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.add(
                            egui::ProgressBar::new(used)
                                .desired_width(ui.available_width() - 4.0)
                                .desired_height(5.0)
                                .fill(if used > 0.9 { egui::Color32::from_rgb(218, 38, 38) } else { ui.visuals().selection.bg_fill }),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(
                            RichText::new(format!("{} free of {}", format_bytes(d.free), format_bytes(d.total)))
                                .small()
                                .weak(),
                        );
                    });
                }
            }
            if let Some(p) = go {
                app.navigate(p);
            }
        });
    });
}

pub fn status_bar(app: &mut App, ui: &mut Ui) {
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("{} items", app.view.len()));
            if !app.selected.is_empty() {
                ui.separator();
                let sel = app.selected_entries();
                let bytes: u64 = sel.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
                if sel.iter().all(|e| !e.is_dir) {
                    ui.label(format!("{} selected  {}", sel.len(), format_bytes(bytes)));
                } else {
                    ui.label(format!("{} selected", sel.len()));
                }
            }
            if app.loading {
                ui.separator();
                ui.spinner();
                ui.label("Loading…");
            } else if let Some(t) = app.list_time {
                ui.separator();
                ui.label(RichText::new(format!("read in {:.1} ms", t.as_secs_f64() * 1000.0)).weak());
            }
            if app.ops_running > 0 {
                ui.separator();
                ui.spinner();
                ui.label("Working…");
            }
            if let Some(e) = &app.list_error {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, e);
            } else if let Some(m) = &app.status {
                ui.separator();
                ui.label(m);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut app.show_hidden, "Hidden items");
            });
        });
    });
}

/// What a row click or menu asked for; applied after the table so `app` is not borrowed twice.
enum Action {
    Click(usize, egui::Modifiers),
    Open(usize),
    ContextSelect(usize),
    Copy(bool),
    Paste(PathBuf),
    CopyPath,
    Rename,
    Delete(bool),
    Properties(PathBuf),
    NewFolder,
    Refresh,
    Terminal,
    ClearSelection,
}

pub fn file_list(app: &mut App, ui: &mut Ui) {
    let mut action: Option<Action> = None;

    // Background (empty space below the rows): clear selection, folder context menu.
    let bg = ui.interact(ui.max_rect(), ui.id().with("bg"), Sense::click());
    if bg.clicked() {
        action = Some(Action::ClearSelection);
    }
    let cur = app.cur.clone();
    bg.context_menu(|ui| {
        if ui.button("New folder    Ctrl+Shift+N").clicked() {
            action = Some(Action::NewFolder);
            ui.close();
        }
        if ui.button("Paste    Ctrl+V").clicked() {
            action = Some(Action::Paste(cur.clone()));
            ui.close();
        }
        if ui.button("Refresh    F5").clicked() {
            action = Some(Action::Refresh);
            ui.close();
        }
        ui.separator();
        if ui.button("Open in Terminal").clicked() {
            action = Some(Action::Terminal);
            ui.close();
        }
        if ui.button("Properties").clicked() {
            action = Some(Action::Properties(cur.clone()));
            ui.close();
        }
    });

    if app.view.is_empty() && !app.loading {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            let msg = if app.list_error.is_some() {
                "This folder can't be opened."
            } else if app.filter.is_empty() {
                "This folder is empty."
            } else {
                "No items match your search."
            };
            ui.label(RichText::new(msg).weak());
        });
    }

    let row_h = 22.0;
    let mut sort_click = None;
    let header = |h: &mut egui_extras::TableRow, key: SortKey, title: &str, app: &App, out: &mut Option<SortKey>| {
        h.col(|ui| {
            let arrow = if app.sort == key { if app.ascending { " ⏶" } else { " ⏷" } } else { "" };
            if ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false)).clicked() {
                *out = Some(key);
            }
        });
    };
    let n = app.view.len();
    let weak = ui.visuals().weak_text_color();
    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(200.0).clip(true))
        .column(Column::initial(135.0).at_least(80.0).clip(true))
        .column(Column::initial(160.0).at_least(60.0).clip(true))
        .column(Column::initial(95.0).at_least(60.0));
    if let Some(pos) = app.scroll_to.take().filter(|&p| p < n) {
        table = table.scroll_to_row(pos, None);
    }
    table
        .header(row_h, |mut h| {
            header(&mut h, SortKey::Name, "Name", app, &mut sort_click);
            header(&mut h, SortKey::Modified, "Date modified", app, &mut sort_click);
            header(&mut h, SortKey::Type, "Type", app, &mut sort_click);
            header(&mut h, SortKey::Size, "Size", app, &mut sort_click);
        })
        .body(|body| {
            body.rows(row_h, n, |mut row| {
                let pos = row.index();
                let e = &app.entries[app.view[pos]];
                row.set_selected(app.selected.contains(&e.name));
                let dim = e.hidden || e.system;
                let name_text = if dim { RichText::new(&e.name).color(weak) } else { RichText::new(&e.name) };
                row.col(|ui| {
                    let icon = RichText::new(icon_for(e));
                    ui.label(if e.is_dir { icon.color(egui::Color32::from_rgb(230, 180, 60)) } else { icon });
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(name_text).selectable(false).truncate());
                    if e.link {
                        ui.label(RichText::new(" ⤴").weak());
                    }
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(RichText::new(format_time(e.modified, app.utc_offset)).color(weak)).selectable(false));
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(RichText::new(e.type_name()).color(weak)).selectable(false));
                });
                row.col(|ui| {
                    if !e.is_dir {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(format_size_kb(e.size)).color(weak)).selectable(false));
                        });
                    }
                });
                let resp = row.response();
                if resp.double_clicked() {
                    action = Some(Action::Open(pos));
                } else if resp.clicked() {
                    action = Some(Action::Click(pos, resp.ctx.input(|i| i.modifiers)));
                } else if resp.secondary_clicked() {
                    action = Some(Action::ContextSelect(pos));
                }
                let target = app.cur.join(&e.name);
                let is_dir = e.is_dir;
                resp.context_menu(|ui| {
                    if ui.button(RichText::new("Open").strong()).clicked() {
                        action = Some(Action::Open(pos));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Cut    Ctrl+X").clicked() {
                        action = Some(Action::Copy(true));
                        ui.close();
                    }
                    if ui.button("Copy    Ctrl+C").clicked() {
                        action = Some(Action::Copy(false));
                        ui.close();
                    }
                    if is_dir && ui.button("Paste into folder").clicked() {
                        action = Some(Action::Paste(target.clone()));
                        ui.close();
                    }
                    if ui.button("Copy as path").clicked() {
                        action = Some(Action::CopyPath);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Rename    F2").clicked() {
                        action = Some(Action::Rename);
                        ui.close();
                    }
                    if ui.button("Delete    Del").clicked() {
                        action = Some(Action::Delete(false));
                        ui.close();
                    }
                    if ui.button("Delete permanently    Shift+Del").clicked() {
                        action = Some(Action::Delete(true));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Properties    Alt+Enter").clicked() {
                        action = Some(Action::Properties(target.clone()));
                        ui.close();
                    }
                });
            });
        });

    if let Some(k) = sort_click {
        app.set_sort(k);
    }
    match action {
        Some(Action::Click(pos, m)) => {
            if m.shift {
                app.select_range(pos);
            } else if m.command {
                app.toggle(pos);
            } else {
                app.select_only(pos);
            }
        }
        Some(Action::Open(pos)) => {
            if let Some(e) = app.entry_at(pos).cloned() {
                app.open_entry(&e);
            }
        }
        Some(Action::ContextSelect(pos)) => {
            let name = app.entry_at(pos).map(|e| e.name.clone());
            if name.is_some_and(|n| !app.selected.contains(&n)) {
                app.select_only(pos);
            }
        }
        Some(Action::Copy(cut)) => app.copy_selection(cut),
        Some(Action::Paste(dest)) => app.paste_into(dest),
        Some(Action::CopyPath) => {
            let text = app
                .selected_paths()
                .iter()
                .map(|p| format!("\"{}\"", p.display()))
                .collect::<Vec<_>>()
                .join("\r\n");
            ui.ctx().copy_text(text);
        }
        Some(Action::Rename) => app.begin_rename(),
        Some(Action::Delete(permanent)) => app.delete_selection(permanent),
        Some(Action::Properties(p)) => app.show_properties(&p),
        Some(Action::NewFolder) => app.begin_new_folder(),
        Some(Action::Refresh) => app.refresh(),
        Some(Action::Terminal) => {
            if let Err(e) = explorer_sys::open_terminal(&app.cur) {
                app.status = Some(format!("Terminal: {e}"));
            }
        }
        Some(Action::ClearSelection) => {
            app.selected.clear();
            app.cursor = None;
        }
        None => {}
    }
}

pub fn dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.dialog.take() else { return };
    let (title, text, error, opened) = match &mut d {
        Dialog::Rename { text, error, opened, .. } => ("Rename", text, error, opened),
        Dialog::NewFolder { text, error, opened } => ("New folder", text, error, opened),
    };
    let mut submit = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("dialog")).show(ui.ctx(), |ui| {
        ui.set_width(360.0);
        ui.heading(title);
        ui.add_space(6.0);
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        if !*opened {
            *opened = true;
            r.request_focus();
            // Select the name without its extension, as Explorer does.
            let stem = match text.rfind('.') {
                Some(i) if i > 0 && title == "Rename" => text[..i].chars().count(),
                _ => text.chars().count(),
            };
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                let range = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem));
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), r.id);
            }
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submit = true;
        }
        if let Some(e) = error {
            ui.colored_label(ui.visuals().error_fg_color, *e);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                submit = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if modal.should_close() {
        cancel = true;
    }
    if submit {
        match app.finish_dialog(&d) {
            None => return,
            Some(e) => match &mut d {
                Dialog::Rename { error, .. } | Dialog::NewFolder { error, .. } => *error = Some(e),
            },
        }
    }
    if !cancel {
        app.dialog = Some(d);
    }
}
