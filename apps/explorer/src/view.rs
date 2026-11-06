//! Drawing: toolbar with address bar, sidebar, virtualised file table, status bar, dialogs.

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use explorer_core::{
    SortKey, format_attributes, format_bytes, format_size_kb, format_time, icon_for, location_crumbs, parent_path,
};
use explorer_sys::DriveKind;

use crate::{App, Dialog, IconKey, IconState, MapDrive, ViewMode};

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("tabs").show(ui, |ui| {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            let (mut switch, mut close) = (None, None);
            for i in 0..app.tabs.len() {
                let title = app.tab_title(i);
                let active = i == app.active_tab;
                let short: String = if title.chars().count() > 24 {
                    title.chars().take(22).chain(Some('…')).collect()
                } else {
                    title.clone()
                };
                let r = ui
                    .add(egui::Button::selectable(active, short).min_size(egui::vec2(90.0, 22.0)))
                    .on_hover_text(&title);
                if r.clicked() {
                    switch = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if ui
                    .add(egui::Button::new(RichText::new("×").weak()).frame(false))
                    .on_hover_text("Close tab (Ctrl+W)")
                    .clicked()
                {
                    close = Some(i);
                }
                ui.add_space(4.0);
            }
            if ui.button("+").on_hover_text("New tab (Ctrl+T)").clicked() {
                let home = app.home.clone();
                app.new_tab(home, true);
            }
            if let Some(i) = switch {
                app.switch_tab(i);
            } else if let Some(i) = close {
                app.close_tab(i);
            }
        });
        ui.add_space(1.0);
    });
}

pub fn toolbar(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(app.history.can_back(), egui::Button::new("⬅")).on_hover_text("Back (Alt+Left)").clicked()
            {
                app.back();
            }
            if ui
                .add_enabled(app.history.can_forward(), egui::Button::new("➡"))
                .on_hover_text("Forward (Alt+Right)")
                .clicked()
            {
                app.forward();
            }
            if ui
                .add_enabled(parent_path(&app.cur).is_some(), egui::Button::new("⬆"))
                .on_hover_text("Up (Alt+Up)")
                .clicked()
            {
                app.up();
            }
            if ui.button("⟳").on_hover_text("Refresh (F5)").clicked() {
                app.refresh();
                app.load_drives();
            }
            ui.menu_button("…", |ui| {
                if ui.button("New folder").clicked() {
                    app.begin_new_folder();
                    ui.close();
                }
                if app.in_recycle_bin() {
                    if ui.add_enabled(!app.selected.is_empty(), egui::Button::new("Restore selected items")).clicked() {
                        app.restore_selection();
                        ui.close();
                    }
                    if ui.button("Empty Recycle Bin").clicked() {
                        app.empty_bin();
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Map network drive…").clicked() {
                    app.begin_map_drive();
                    ui.close();
                }
                let mapped: Vec<char> = app
                    .drives
                    .iter()
                    .filter(|d| d.kind == DriveKind::Network)
                    .filter_map(|d| d.root.to_string_lossy().chars().next())
                    .collect();
                if !mapped.is_empty() {
                    ui.menu_button("Disconnect network drive", |ui| {
                        for l in mapped {
                            if ui.button(format!("{l}:")).clicked() {
                                app.unmap(l);
                                ui.close();
                            }
                        }
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("View:");
                    ui.selectable_value(&mut app.view_mode, ViewMode::Details, "Details");
                    ui.selectable_value(&mut app.view_mode, ViewMode::Icons, "Large icons");
                });
                ui.horizontal(|ui| {
                    ui.label("Pane:");
                    for (p, label) in [(crate::pane::Pane::Preview, "Preview"), (crate::pane::Pane::Details, "Details")]
                    {
                        if ui.selectable_label(app.pane == p, label).clicked() {
                            app.toggle_pane(p);
                        }
                    }
                });
                ui.checkbox(&mut app.show_hidden, "Hidden items");
                ui.checkbox(&mut app.show_extensions, "File name extensions");
                if ui.button("Open in Terminal").clicked() {
                    if let Err(e) = explorer_sys::open_terminal(&app.cur) {
                        app.status = Some(format!("Terminal: {e}"));
                    }
                    ui.close();
                }
                if ui.button("Properties").clicked() {
                    let c = app.cur.clone();
                    app.show_properties(&c);
                    ui.close();
                }
            });

            // Search box on the right, address bar takes the rest.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.filter)
                        .hint_text("🔍 Search (Enter: subfolders)")
                        .desired_width(200.0),
                );
                if std::mem::take(&mut app.focus_search) {
                    search.request_focus();
                }
                if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.filter.clear();
                    search.surrender_focus();
                }
                // Enter searches subfolders too; clearing the box goes back to the folder.
                if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !app.filter.trim().is_empty()
                {
                    app.start_deep_search(app.filter.trim().to_string());
                }
                if app.deep_search.is_some() && app.filter.trim().is_empty() {
                    app.end_deep_search();
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
    let resp = frame.show(ui, |ui| {
        ui.set_width(width - 16.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        let crumbs = location_crumbs(&app.cur);
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

/// Requests and clicks collected while drawing the folder tree (it can't borrow `App` mutably).
#[derive(Default)]
struct TreeOut {
    go: Option<PathBuf>,
    load: Vec<PathBuf>,
}

/// Expandable node: arrow + clickable label; children are listed lazily the first time it opens.
fn tree_node(
    ui: &mut Ui,
    tree: &std::collections::HashMap<PathBuf, Vec<String>>,
    cur: &PathBuf,
    path: &PathBuf,
    label: &str,
    out: &mut TreeOut,
    depth: usize,
) -> egui::Response {
    let id = ui.make_persistent_id(("tree", path));
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let open = state.is_open();
    let (_, header, _) = state
        .show_header(ui, |ui| {
            ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                ui.add(egui::Button::selectable(path == cur, label).min_size(egui::vec2(0.0, 20.0)))
            })
            .inner
        })
        .body(|ui| match tree.get(path) {
            Some(children) if depth < 32 => {
                for name in children {
                    let child = path.join(name);
                    tree_node(ui, tree, cur, &child, &format!("📁 {name}"), out, depth + 1);
                }
                if children.is_empty() {
                    ui.label(RichText::new("(empty)").small().weak());
                }
            }
            Some(_) => {}
            None => {
                ui.spinner();
            }
        });
    if open && !tree.contains_key(path) {
        out.load.push(path.clone());
    }
    if header.inner.clicked() {
        out.go = Some(path.clone());
    }
    header.inner
}

pub fn sidebar(app: &mut App, ui: &mut Ui) {
    egui::Panel::left("sidebar").resizable(true).default_size(190.0).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(6.0);
            let mut go: Option<PathBuf> = None;
            let mut unmap: Option<char> = None;
            let mut tree_out = TreeOut::default();
            let place = |ui: &mut Ui, icon: &str, name: &str, path: &PathBuf, cur: &PathBuf| {
                let selected = path == cur;
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    ui.add(
                        egui::Button::selectable(selected, format!("{icon}  {name}")).min_size(egui::vec2(0.0, 22.0)),
                    )
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
                tree_node(ui, &app.tree, &app.cur, path, &format!("{icon}  {name}"), &mut tree_out, 0);
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
                let r = tree_node(ui, &app.tree, &app.cur, &d.root, &format!("{icon}  {label}"), &mut tree_out, 0);
                if d.kind == DriveKind::Network {
                    let letter = letter.chars().next().unwrap_or('Z');
                    r.context_menu(|ui| {
                        if ui.button("Disconnect").clicked() {
                            unmap = Some(letter);
                            ui.close();
                        }
                    });
                }
                if d.total > 0 {
                    let used = 1.0 - d.free as f32 / d.total as f32;
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.add(
                            egui::ProgressBar::new(used)
                                .desired_width(ui.available_width() - 4.0)
                                .desired_height(5.0)
                                .fill(if used > 0.9 {
                                    egui::Color32::from_rgb(218, 38, 38)
                                } else {
                                    ui.visuals().selection.bg_fill
                                }),
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
            ui.add_space(6.0);
            let network = PathBuf::from(explorer_core::NETWORK);
            if place(ui, "🖧", "Network", &network, &app.cur) {
                go = Some(network);
            }
            let bin = PathBuf::from(explorer_core::RECYCLE_BIN);
            if place(ui, "🗑", "Recycle Bin", &bin, &app.cur) {
                go = Some(bin);
            }
            if let Some(l) = unmap {
                app.unmap(l);
            }
            if go.is_none() {
                go = tree_out.go;
            }
            for p in tree_out.load {
                app.load_tree(p);
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
            if let Some(q) = &app.deep_search {
                ui.separator();
                ui.label(format!("Results for \"{q}\" in subfolders"));
            }
            if app.loading {
                ui.separator();
                ui.spinner();
                ui.label(if app.deep_search.is_some() { "Searching…" } else { "Loading…" });
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
    /// The shell's menu for the clicked row (selecting it first), or the folder background.
    ShellMenu(Option<usize>),
    Restore,
    EmptyBin,
    OpenInNewTab(usize),
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
    // The sidebar or toolbar may have navigated earlier in this frame.
    app.update_view();
    let mut action: Option<Action> = None;

    // Background (empty space below the rows): clear selection, folder context menu.
    let bg = ui.interact(ui.max_rect(), ui.id().with("bg"), Sense::click());
    if bg.clicked() {
        action = Some(Action::ClearSelection);
    }
    let cur = app.cur.clone();
    // On Windows the shell's own menus are used (all installed handlers); egui menus are the
    // fallback for the virtual Network page and for non-Windows builds.
    let shell_menus = app.hwnd != 0
        && !matches!(
            explorer_core::classify(&app.cur),
            explorer_core::Place::Network | explorer_core::Place::RecycleBin
        );
    let in_bin = app.in_recycle_bin();
    if shell_menus && bg.secondary_clicked() {
        action = Some(Action::ShellMenu(None));
    }
    if in_bin {
        bg.context_menu(|ui| {
            if ui.button("Empty Recycle Bin").clicked() {
                action = Some(Action::EmptyBin);
                ui.close();
            }
            if ui.button("Refresh    F5").clicked() {
                action = Some(Action::Refresh);
                ui.close();
            }
        });
    }
    let bg_menu = |ui: &mut Ui, action: &mut Option<Action>| {
        if ui.button("New folder    Ctrl+Shift+N").clicked() {
            *action = Some(Action::NewFolder);
            ui.close();
        }
        if ui.button("Paste    Ctrl+V").clicked() {
            *action = Some(Action::Paste(cur.clone()));
            ui.close();
        }
        if ui.button("Refresh    F5").clicked() {
            *action = Some(Action::Refresh);
            ui.close();
        }
        ui.separator();
        if ui.button("Open in Terminal").clicked() {
            *action = Some(Action::Terminal);
            ui.close();
        }
        if ui.button("Properties").clicked() {
            *action = Some(Action::Properties(cur.clone()));
            ui.close();
        }
    };
    if !shell_menus && !in_bin {
        bg.context_menu(|ui| bg_menu(ui, &mut action));
    }

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
            if app.needs_sign_in && ui.button("Sign in…").clicked() {
                app.sign_in();
            }
        });
    }

    let row_h = 22.0;
    let mut sort_click = None;
    // Right-clicking any header opens the column chooser, as in Explorer.
    let mut toggle_col: Option<SortKey> = None;
    let (col_created, col_attrs) = (app.col_created, app.col_attrs);
    let mut header =
        |h: &mut egui_extras::TableRow, key: SortKey, title: &str, app: &App, out: &mut Option<SortKey>| {
            h.col(|ui| {
                let arrow = if app.sort == key { if app.ascending { " ⏶" } else { " ⏷" } } else { "" };
                let resp = ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false));
                if resp.clicked() {
                    *out = Some(key);
                }
                resp.context_menu(|ui| {
                    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Name"));
                    for (k, label, on) in [
                        (SortKey::Created, "Date created", col_created),
                        (SortKey::Attributes, "Attributes", col_attrs),
                    ] {
                        let mut on = on;
                        if ui.checkbox(&mut on, label).clicked() {
                            toggle_col = Some(k);
                            ui.close();
                        }
                    }
                });
            });
        };
    let n = app.view.len();
    let weak = ui.visuals().weak_text_color();
    let mut wanted_icons: Vec<IconKey> = Vec::new();
    if app.view_mode == ViewMode::Icons {
        icon_grid(app, ui, &mut action, &mut wanted_icons, shell_menus, in_bin);
    } else {
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(120.0).clip(true))
            .column(Column::initial(135.0).at_least(80.0).clip(true))
            .column(Column::initial(160.0).at_least(60.0).clip(true))
            .column(Column::initial(95.0).at_least(60.0));
        if col_created {
            table = table.column(Column::initial(135.0).at_least(80.0).clip(true));
        }
        if col_attrs {
            table = table.column(Column::initial(80.0).at_least(40.0).clip(true));
        }
        // Column widths are remembered per column set.
        table = table.id_salt(("details", col_created, col_attrs, app.pane));
        if let Some(pos) = app.scroll_to.take().filter(|&p| p < n) {
            table = table.scroll_to_row(pos, None);
        }
        table
            .header(row_h, |mut h| {
                header(&mut h, SortKey::Name, "Name", app, &mut sort_click);
                let bin = app.in_recycle_bin();
                header(
                    &mut h,
                    SortKey::Modified,
                    if bin { "Date deleted" } else { "Date modified" },
                    app,
                    &mut sort_click,
                );
                header(&mut h, SortKey::Type, if bin { "Original location" } else { "Type" }, app, &mut sort_click);
                header(&mut h, SortKey::Size, "Size", app, &mut sort_click);
                if col_created {
                    header(&mut h, SortKey::Created, "Date created", app, &mut sort_click);
                }
                if col_attrs {
                    header(&mut h, SortKey::Attributes, "Attributes", app, &mut sort_click);
                }
            })
            .body(|body| {
                body.rows(row_h, n, |mut row| {
                    let pos = row.index();
                    let e = &app.entries[app.view[pos]];
                    row.set_selected(app.selected.contains(&e.name));
                    let dim = e.hidden || e.system;
                    let shown = if app.show_extensions || e.is_dir {
                        e.display()
                    } else {
                        let d = e.display();
                        d.rfind('.').filter(|&i| i > 0).map_or(d, |i| &d[..i])
                    };
                    let name_text = if dim { RichText::new(shown).color(weak) } else { RichText::new(shown) };
                    row.col(|ui| {
                        let key = IconKey::for_entry(e, &app.cur);
                        match app.icons.get(&key) {
                            Some(IconState::Ready(tex)) => {
                                ui.add(egui::Image::new((tex.id(), egui::vec2(16.0, 16.0))));
                            }
                            state => {
                                if state.is_none() {
                                    wanted_icons.push(key);
                                }
                                let icon = RichText::new(icon_for(e));
                                ui.label(if e.is_dir {
                                    icon.color(egui::Color32::from_rgb(230, 180, 60))
                                } else {
                                    icon
                                });
                            }
                        }
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(name_text).selectable(false).truncate());
                        if e.link {
                            ui.label(RichText::new(" ⤴").weak());
                        }
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(format_time(e.modified, app.utc_offset)).color(weak))
                                .selectable(false),
                        );
                    });
                    row.col(|ui| {
                        let third = match &e.note {
                            Some(n) => n.clone(),
                            None => e.type_name(),
                        };
                        ui.add(egui::Label::new(RichText::new(third).color(weak)).selectable(false));
                    });
                    row.col(|ui| {
                        if !e.is_dir {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(format_size_kb(e.size)).color(weak))
                                        .selectable(false),
                                );
                            });
                        }
                    });
                    if col_created {
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format_time(e.created, app.utc_offset)).color(weak))
                                    .selectable(false),
                            );
                        });
                    }
                    if col_attrs {
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format_attributes(e.attrs)).color(weak))
                                    .selectable(false),
                            );
                        });
                    }
                    let resp = row.response();
                    if resp.middle_clicked() && e.is_dir {
                        action = Some(Action::OpenInNewTab(pos));
                    } else if resp.double_clicked() {
                        action = Some(Action::Open(pos));
                    } else if resp.clicked() {
                        action = Some(Action::Click(pos, resp.ctx.input(|i| i.modifiers)));
                    } else if resp.secondary_clicked() {
                        action =
                            Some(if shell_menus { Action::ShellMenu(Some(pos)) } else { Action::ContextSelect(pos) });
                    }
                    let target = app.child(&e.name);
                    let is_dir = e.is_dir;
                    if shell_menus {
                        return;
                    }
                    if in_bin {
                        resp.context_menu(|ui| {
                            if ui.button(RichText::new("Restore").strong()).clicked() {
                                action = Some(Action::Restore);
                                ui.close();
                            }
                            if ui.button("Delete permanently    Del").clicked() {
                                action = Some(Action::Delete(true));
                                ui.close();
                            }
                        });
                        return;
                    }
                    resp.context_menu(|ui| {
                        if ui.button(RichText::new("Open").strong()).clicked() {
                            action = Some(Action::Open(pos));
                            ui.close();
                        }
                        if is_dir && ui.button("Open in new tab").clicked() {
                            action = Some(Action::OpenInNewTab(pos));
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
    }

    app.request_icons(wanted_icons);
    app.request_thumbs();
    match toggle_col {
        Some(SortKey::Created) => app.col_created = !app.col_created,
        Some(SortKey::Attributes) => app.col_attrs = !app.col_attrs,
        _ => {}
    }
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
        Some(Action::ShellMenu(pos)) => {
            if let Some(pos) = pos {
                let name = app.entry_at(pos).map(|e| e.name.clone());
                if name.is_some_and(|n| !app.selected.contains(&n)) {
                    app.select_only(pos);
                }
            }
            let shift = ui.input(|i| i.modifiers.shift);
            app.shell_menu(pos.is_some(), shift);
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
            let text =
                app.selected_paths().iter().map(|p| format!("\"{}\"", p.display())).collect::<Vec<_>>().join("\r\n");
            ui.ctx().copy_text(text);
        }
        Some(Action::Rename) => app.begin_rename(),
        Some(Action::Restore) => app.restore_selection(),
        Some(Action::OpenInNewTab(pos)) => {
            if let Some(e) = app.entry_at(pos).cloned() {
                let p = app.child(&e.name);
                app.new_tab(p, false);
            }
        }
        Some(Action::EmptyBin) => app.empty_bin(),
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

fn map_drive_dialog(app: &mut App, ui: &mut Ui, mut m: MapDrive) {
    let (mut ok, mut cancel) = (false, false);
    let modal = egui::Modal::new(egui::Id::new("map-drive")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);
        ui.heading("Map network drive");
        ui.add_space(6.0);
        egui::Grid::new("map-grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
            ui.label("Drive:");
            egui::ComboBox::from_id_salt("letter").selected_text(format!("{}:", m.letter)).show_ui(ui, |ui| {
                for &l in &m.letters {
                    ui.selectable_value(&mut m.letter, l, format!("{l}:"));
                }
            });
            ui.end_row();
            ui.label("Folder:");
            let r = ui.add(egui::TextEdit::singleline(&mut m.path).hint_text(r"\\server\share").desired_width(300.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            ui.end_row();
        });
        ui.checkbox(&mut m.persistent, "Reconnect at sign-in");
        ui.label(RichText::new("Windows asks for a user name and password if the share needs one.").small().weak());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let valid = m.path.trim().starts_with(r"\\") && m.path.trim().len() > 3;
            if ui.add_enabled(valid, egui::Button::new("Finish")).clicked() {
                ok = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if ok {
        app.finish_map_drive(&m);
    } else if !cancel && !modal.should_close() {
        app.dialog = Some(Dialog::MapDrive(m));
    }
}

pub fn dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.dialog.take() else { return };
    let (title, text, error, opened) = match &mut d {
        Dialog::Rename { text, error, opened, .. } => ("Rename", text, error, opened),
        Dialog::NewFolder { text, error, opened } => ("New folder", text, error, opened),
        Dialog::MapDrive(_) => {
            let Dialog::MapDrive(m) = d else { unreachable!() };
            return map_drive_dialog(app, ui, m);
        }
        Dialog::ConfirmPurge(items) => {
            let (mut yes, mut no) = (false, false);
            let modal = egui::Modal::new(egui::Id::new("purge")).show(ui.ctx(), |ui| {
                ui.set_width(380.0);
                ui.heading("Delete permanently");
                ui.add_space(6.0);
                ui.label(match items.as_slice() {
                    [one] => format!("Are you sure you want to permanently delete \"{}\"?", one.display()),
                    many => format!("Are you sure you want to permanently delete these {} items?", many.len()),
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    yes = ui.button("Yes").clicked();
                    no = ui.button("No").clicked();
                });
            });
            if yes {
                let items = std::mem::take(items);
                app.purge_items(&items);
            } else if !no && !modal.should_close() {
                app.dialog = Some(d);
            }
            return;
        }
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
                Dialog::MapDrive(_) | Dialog::ConfirmPurge(_) => {}
            },
        }
    }
    if !cancel {
        app.dialog = Some(d);
    }
}

/// Large icons view: a virtualised grid of tiles with shell thumbnails.
fn icon_grid(
    app: &mut App,
    ui: &mut Ui,
    action: &mut Option<Action>,
    wanted_icons: &mut Vec<IconKey>,
    shell_menus: bool,
    in_bin: bool,
) {
    let (tile_w, tile_h, img) = (112.0_f32, 124.0_f32, 96.0_f32);
    let n = app.view.len();
    let cols = ((ui.available_width() / tile_w).floor() as usize).max(1);
    let rows = n.div_ceil(cols);
    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(pos) = app.scroll_to.take().filter(|&p| p < n) {
        scroll = scroll.vertical_scroll_offset((pos / cols) as f32 * tile_h);
    }
    let weak = ui.visuals().weak_text_color();
    let sel_fill = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    let hover_fill = ui.visuals().widgets.hovered.weak_bg_fill;
    scroll.show_rows(ui, tile_h, rows, |ui, range| {
        for r in range {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for c in 0..cols {
                    let pos = r * cols + c;
                    if pos >= n {
                        break;
                    }
                    let e = &app.entries[app.view[pos]];
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(tile_w, tile_h), Sense::click());
                    let selected = app.selected.contains(&e.name);
                    if selected {
                        ui.painter().rect_filled(rect.shrink(2.0), 4.0, sel_fill);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect.shrink(2.0), 4.0, hover_fill);
                    }
                    let img_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.center().x, rect.top() + 6.0 + img / 2.0),
                        egui::vec2(img, img),
                    );
                    let path = app.child(&e.name);
                    let thumb = if e.is_dir || in_bin { None } else { app.thumbs.get(&path) };
                    match thumb {
                        Some(IconState::Ready(tex)) => {
                            let s = tex.size_vec2();
                            let k = (img / s.x.max(s.y)).min(1.0);
                            let r = egui::Rect::from_center_size(img_rect.center(), s * k);
                            ui.painter().image(
                                tex.id(),
                                r,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        other => {
                            if other.is_none() && !e.is_dir && !in_bin {
                                app.thumb_wanted.push(path.clone());
                            }
                            let key = IconKey::for_entry(e, &app.cur);
                            match app.icons.get(&key) {
                                Some(IconState::Ready(tex)) => {
                                    let r = egui::Rect::from_center_size(img_rect.center(), egui::vec2(48.0, 48.0));
                                    ui.painter().image(
                                        tex.id(),
                                        r,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        egui::Color32::WHITE,
                                    );
                                }
                                state => {
                                    if state.is_none() {
                                        wanted_icons.push(key);
                                    }
                                    let color = if e.is_dir {
                                        egui::Color32::from_rgb(230, 180, 60)
                                    } else {
                                        ui.visuals().text_color()
                                    };
                                    ui.painter().text(
                                        img_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        icon_for(e),
                                        egui::FontId::proportional(44.0),
                                        color,
                                    );
                                }
                            }
                        }
                    }
                    let mut job = egui::text::LayoutJob::simple(
                        e.display().to_string(),
                        egui::FontId::proportional(12.5),
                        if e.hidden || e.system { weak } else { ui.visuals().text_color() },
                        tile_w - 8.0,
                    );
                    job.wrap.max_rows = 2;
                    job.halign = Align::Center;
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let text_pos = egui::pos2(rect.center().x, img_rect.bottom() + 4.0);
                    ui.painter().galley(text_pos, galley, ui.visuals().text_color());

                    if resp.middle_clicked() && e.is_dir {
                        *action = Some(Action::OpenInNewTab(pos));
                    } else if resp.double_clicked() {
                        *action = Some(Action::Open(pos));
                    } else if resp.clicked() {
                        *action = Some(Action::Click(pos, resp.ctx.input(|i| i.modifiers)));
                    } else if resp.secondary_clicked() {
                        *action =
                            Some(if shell_menus { Action::ShellMenu(Some(pos)) } else { Action::ContextSelect(pos) });
                    }
                }
            });
        }
    });
}
