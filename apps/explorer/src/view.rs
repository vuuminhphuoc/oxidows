//! Drawing: toolbar with address bar, sidebar, virtualised file table, status bar, dialogs.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use explorer_core::{
    SortKey, classify, format_attributes, format_bytes, format_size_kb, format_time, icon_for, parent_path,
};
use explorer_sys::DriveKind;

use ui_common::Icon;

use crate::{App, Dialog, IconKey, IconState, MapDrive, ViewMode};

/// Temp-data key of the Details rubber band: [start x on screen, start as a fractional row].
const DETAILS_BAND: &str = "details-band";

/// The selection rectangle in Details. Rows are uniform, so the band is tracked in row units
/// and survives scrolling; the rows it spans (headings excluded) become the selection.
fn details_band(
    app: &mut App,
    ui: &mut Ui,
    start: Option<[f32; 2]>,
    drawn: &[(usize, egui::Rect)],
    rows_map: &[isize],
) {
    let id = egui::Id::new(DETAILS_BAND);
    let base_id = egui::Id::new("details-band-base");
    if let Some(s) = start {
        let base = if ui.input(|i| i.modifiers.command) { app.selected.clone() } else { Default::default() };
        ui.data_mut(|d| {
            d.insert_temp(id, s);
            d.insert_temp(base_id, base);
        });
    }
    let Some([x0, start]) = ui.data(|d| d.get_temp::<[f32; 2]>(id)) else { return };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
    let (Some(p), Some(&(k0, r0)), Some(&(k1, r1))) = (pos.filter(|_| down), drawn.first(), drawn.last()) else {
        ui.data_mut(|d| {
            d.remove::<[f32; 2]>(id);
            d.remove::<std::collections::HashSet<String>>(base_id);
        });
        return;
    };
    let stride = if k1 > k0 { (r1.top() - r0.top()) / (k1 - k0) as f32 } else { r0.height() };
    let cur = k0 as f32 + (p.y - r0.top()) / stride.max(1.0);
    let (a, b) = (start.min(cur).max(0.0), start.max(cur).max(0.0));
    let n_rows = if rows_map.is_empty() { app.view.len() } else { rows_map.len() };
    let names: std::collections::HashSet<String> = (a.floor() as usize..=(b.floor() as usize))
        .take_while(|&r| r < n_rows)
        .filter_map(|r| if rows_map.is_empty() { Some(r) } else { usize::try_from(rows_map[r]).ok() })
        .filter_map(|pos| app.entry_at(pos).map(|e| e.name.clone()))
        .collect();
    let base: std::collections::HashSet<String> = ui.data(|d| d.get_temp(base_id)).unwrap_or_default();
    app.selected = base.union(&names).cloned().collect();
    let y = |f: f32| r0.top() + (f - k0 as f32) * stride;
    let band = egui::Rect::from_two_pos(egui::pos2(x0, y(start)), p);
    let clip = egui::Rect::from_min_max(r0.min, r1.max);
    let sel = ui.visuals().selection.bg_fill;
    ui.painter().with_clip_rect(clip).rect(
        band,
        0.0,
        sel.gamma_multiply(0.25),
        egui::Stroke::new(1.0, sel),
        egui::StrokeKind::Inside,
    );
    ui.ctx().request_repaint();
}

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("tabs").show(ui, |ui| {
        ui.add_space(3.0);
        let bar = ui.max_rect();
        ui.horizontal(|ui| {
            let (mut switch, mut close) = (None, None);
            // Dragging a tab: along the bar reorders, away from it opens it in a new window.
            let (mut rects, mut dragged, mut released) = (Vec::new(), None, None);
            for i in 0..app.tabs.len() {
                let title = app.tab_title(i);
                let active = i == app.active_tab;
                let short: String = if title.chars().count() > 24 {
                    title.chars().take(22).chain(Some('…')).collect()
                } else {
                    title.clone()
                };
                let r = ui
                    .add(
                        egui::Button::selectable(active, Icon::FolderFilled.with(short))
                            .min_size(egui::vec2(120.0, 28.0))
                            .sense(Sense::click_and_drag()),
                    )
                    .on_hover_text(&title);
                rects.push(r.rect);
                if r.dragged() {
                    dragged = Some(i);
                }
                if r.drag_stopped() {
                    released = Some(i);
                }
                if r.clicked() {
                    switch = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if ui
                    .add(egui::Button::new(Icon::Close.sized(10.0)).frame_when_inactive(false))
                    .on_hover_text("Close tab (Ctrl+W)")
                    .clicked()
                {
                    close = Some(i);
                }
                ui.add_space(4.0);
            }
            if ui_common::tool_button(ui, Icon::Add, true, "New tab (Ctrl+T)").clicked() {
                app.new_tab(PathBuf::from(explorer_core::HOME), true);
            }
            let pointer = ui.input(|i| i.pointer.latest_pos());
            let screen = ui.ctx().content_rect();
            // Far enough below the bar, or outside the window.
            let torn = |p: egui::Pos2| p.y > bar.bottom() + 40.0 || !screen.contains(p);
            if let (Some(i), Some(p)) = (dragged, pointer) {
                if torn(p) {
                    if app.tabs.len() > 1 {
                        egui::Tooltip::always_open(
                            ui.ctx().clone(),
                            ui.layer_id(),
                            egui::Id::new("tab-tear"),
                            egui::PopupAnchor::Pointer,
                        )
                        .show(|ui| ui.label(format!("Open \"{}\" in a new window", app.tab_title(i))));
                    }
                } else if let Some(j) = rects.iter().position(|r| r.x_range().contains(p.x)).filter(|&j| j != i) {
                    app.move_tab(i, j);
                }
            }
            if let (Some(i), Some(p)) = (released, pointer)
                && torn(p)
                && app.tabs.len() > 1
            {
                app.tear_off_tab(i);
            } else if let Some(i) = switch {
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
            if ui_common::tool_button(ui, Icon::Back, app.history.can_back(), "Back (Alt+Left)").clicked() {
                app.back();
            }
            if ui_common::tool_button(ui, Icon::Forward, app.history.can_forward(), "Forward (Alt+Right)").clicked() {
                app.forward();
            }
            // Recent locations.
            let recent = app.history.recent(&app.cur);
            ui.add_enabled_ui(recent.len() > 1, |ui| {
                ui_common::tool_menu(ui, Icon::ChevronDown, "Recent locations", |ui| {
                    for (steps, p) in recent {
                        let label = app.crumbs(&p).last().map_or_else(|| p.display().to_string(), |c| c.0.clone());
                        let text = if steps == 0 { RichText::new(label).strong() } else { RichText::new(label) };
                        if ui.button(text).on_hover_text(p.display().to_string()).clicked() {
                            app.jump_history(steps);
                            ui.close();
                        }
                    }
                });
            });
            if ui_common::tool_button(ui, Icon::Up, parent_path(&app.cur).is_some(), "Up (Alt+Up)").clicked() {
                app.up();
            }
            if ui_common::tool_button(ui, Icon::Refresh, true, "Refresh (F5)").clicked() {
                app.refresh();
                app.load_drives();
            }
            if ui_common::tool_button(ui, Icon::Share, true, "Share with a nearby device (LocalSend)").clicked() {
                app.nearby.queue = app.selected_paths();
                app.nearby.open = true;
            }
            ui_common::tool_menu(ui, Icon::More, "See more", |ui| {
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
                    let undo = app.undo_stack.last().map(|u| u.label());
                    if ui
                        .add_enabled(undo.is_some(), egui::Button::new(format!("Undo {}", undo.unwrap_or_default())))
                        .on_hover_text("Ctrl+Z")
                        .clicked()
                    {
                        app.undo();
                        ui.close();
                    }
                    let redo = app.redo_stack.last().map(|u| u.label());
                    if ui
                        .add_enabled(redo.is_some(), egui::Button::new(format!("Redo {}", redo.unwrap_or_default())))
                        .on_hover_text("Ctrl+Y")
                        .clicked()
                    {
                        app.redo();
                        ui.close();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Select all").on_hover_text("Ctrl+A").clicked() {
                        app.selected = app.view.iter().map(|&i| app.entries[i].name.clone()).collect();
                        ui.close();
                    }
                    if ui.button("Select none").clicked() {
                        app.selected.clear();
                        ui.close();
                    }
                    if ui.button("Invert selection").clicked() {
                        app.invert_selection();
                        ui.close();
                    }
                });
                if !app.in_virtual_place() {
                    let cur = app.cur.clone();
                    let pinned = app.is_pinned(&cur);
                    if ui.button(if pinned { "Unpin from Quick access" } else { "Pin to Quick access" }).clicked() {
                        if pinned {
                            app.unpin(&cur);
                        } else {
                            app.pin(cur);
                        }
                        ui.close();
                    }
                }
                if ui.button("New window").on_hover_text("Ctrl+N").clicked() {
                    app.new_window();
                    ui.close();
                }
                if cfg!(windows) {
                    let mut on = explorer_sys::is_default_file_manager();
                    let r = ui
                        .checkbox(&mut on, "Open folders and Win+E here")
                        .on_hover_text("For your account only. Untick to go back to the built-in File Explorer.");
                    if r.clicked() {
                        let exe = std::env::current_exe().ok();
                        let res = explorer_sys::set_default_file_manager(if on { exe.as_deref() } else { None });
                        app.status = Some(match res {
                            Ok(()) if on => "Folders and Win+E now open in oxi-explorer".into(),
                            Ok(()) => "Folders open in File Explorer again".into(),
                            Err(e) => format!("Could not change the default file manager: {e}"),
                        });
                        ui.close();
                    }
                }
                ui.separator();
                ui.label("View:");
                for (m, label, keys) in ViewMode::ALL {
                    if ui.selectable_label(app.view_mode == m, label).on_hover_text(keys).clicked() {
                        app.view_mode = m;
                    }
                }
                ui.menu_button("Group by", |ui| {
                    use explorer_core::GroupBy;
                    for (g, label) in [
                        (GroupBy::Name, "Name"),
                        (GroupBy::Modified, "Date modified"),
                        (GroupBy::Type, "Type"),
                        (GroupBy::Size, "Size"),
                        (GroupBy::None, "(None)"),
                    ] {
                        if ui.selectable_label(app.group_by == g, label).clicked() {
                            app.group_by = g;
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.selectable_value(&mut app.group_ascending, true, "Ascending");
                    ui.selectable_value(&mut app.group_ascending, false, "Descending");
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
                ui.horizontal(|ui| {
                    use crate::settings::Theme;
                    ui.label("Theme:");
                    let before = app.persist.settings.theme;
                    for (t, label) in Theme::ALL {
                        let label = if t == Theme::System { "System" } else { label };
                        ui.selectable_value(&mut app.persist.settings.theme, t, label);
                    }
                    if app.persist.settings.theme != before {
                        app.persist.settings.apply_theme(ui.ctx());
                    }
                });
                ui.checkbox(&mut app.show_hidden, "Hidden items");
                ui.add_enabled(
                    app.show_hidden,
                    egui::Checkbox::new(&mut app.show_protected, "Protected operating system files"),
                );
                ui.checkbox(&mut app.show_extensions, "File name extensions");
                ui.checkbox(&mut app.item_checkboxes, "Item check boxes");
                ui.checkbox(&mut app.compact, "Compact view");
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
                        .hint_text(Icon::Search.with("Search (Enter: subfolders)"))
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
    if let Some(typed) = app.address_edit.clone() {
        // Autocomplete: subfolders of the typed folder that start with the typed name.
        let cut = typed.rfind(['\\', '/']).map(|i| i + 1).unwrap_or(0);
        let (dir, prefix) = (PathBuf::from(&typed[..cut]), typed[cut..].to_lowercase());
        let mut suggestions: Vec<PathBuf> = Vec::new();
        if cut > 0 && dir.is_dir() {
            match app.tree.get(&dir) {
                Some(names) => {
                    suggestions = names
                        .iter()
                        .filter(|n| n.to_lowercase().starts_with(&prefix) && !n.eq_ignore_ascii_case(&typed[cut..]))
                        .take(12)
                        .map(|n| dir.join(n))
                        .collect()
                }
                None => app.load_tree(dir.clone()),
            }
        }
        let edit_id = ui.make_persistent_id("address-edit");
        // Tab completes the first suggestion; taken before the text box would move focus.
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let text = app.address_edit.as_mut().expect("editing");
        if focused && !suggestions.is_empty() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
        {
            *text = format!("{}{}", suggestions[0].display(), std::path::MAIN_SEPARATOR);
        }
        let mut edit = egui::TextEdit::singleline(text).id(edit_id).desired_width(width - 8.0);
        if focused {
            edit = edit.lock_focus(true);
        }
        let r = ui.add(edit);
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.has_focus() && !suggestions.is_empty() {
            let mut pick = None;
            egui::Area::new(egui::Id::new("address-complete"))
                .order(egui::Order::Foreground)
                .fixed_pos(r.rect.left_bottom())
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(r.rect.width() - 12.0);
                        for s in &suggestions {
                            if ui.add(egui::Button::new(s.display().to_string()).frame(false)).clicked() {
                                pick = Some(s.clone());
                            }
                        }
                    });
                });
            if let Some(p) = pick {
                app.address_edit = None;
                app.navigate(p);
                return;
            }
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
        // The empty part of the bar, registered first so the segments above it get the pointer.
        let bar = ui.interact(
            egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(width - 16.0, ui.spacing().interact_size.y)),
            ui.id().with("address-bg"),
            Sense::click(),
        );
        ui.spacing_mut().item_spacing.x = 0.0;
        let crumbs = app.crumbs(&app.cur);
        // Long paths: show the last segments that fit, like Explorer.
        let skip = crumbs.len().saturating_sub(((width / 90.0) as usize).max(2));
        let mut go = None;
        let mut load = None;
        let mut over_child = false;
        if skip > 0 {
            ui.label(" … ");
        }
        let local = classify(&app.cur) == explorer_core::Place::Local;
        for (i, (label, path)) in crumbs.iter().enumerate().skip(skip) {
            let b = ui.add(egui::Button::new(label).frame(false));
            over_child |= b.hovered();
            if b.clicked() {
                go = Some(path.clone());
            }
            // The arrow after a segment lists its subfolders (the one on the path in bold).
            if local && classify(path) == explorer_core::Place::Local {
                let next = crumbs.get(i + 1).map(|c| &c.1);
                let (arrow, _) = egui::containers::menu::MenuButton::from_button(
                    egui::Button::new(RichText::new(" › ").weak()).frame(false),
                )
                .ui(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| match app.tree.get(path) {
                        Some(names) if names.is_empty() => {
                            ui.label(RichText::new("(empty)").weak());
                        }
                        Some(names) => {
                            for n in names {
                                let child = path.join(n);
                                let t = if Some(&child) == next { RichText::new(n).strong() } else { RichText::new(n) };
                                if ui.button(t).clicked() {
                                    go = Some(child);
                                    ui.close();
                                }
                            }
                        }
                        None => {
                            load = Some(path.clone());
                            ui.spinner();
                        }
                    });
                });
                over_child |= arrow.hovered();
            } else if i + 1 < crumbs.len() {
                ui.label(RichText::new(" › ").weak());
            }
        }
        (go, load, bar.clicked() && !over_child)
    });
    let (go, load, edit) = resp.inner;
    if let Some(p) = load {
        app.load_tree(p);
    }
    if let Some(p) = go {
        app.navigate(p);
    } else if edit {
        app.address_edit = Some(app.address_text());
    }
}

/// Requests and clicks collected while drawing the folder tree (it can't borrow `App` mutably).
#[derive(Default)]
struct TreeOut {
    go: Option<PathBuf>,
    load: Vec<PathBuf>,
    /// A drag is in progress: folders under the pointer are drop targets.
    dragging: bool,
    drop: Option<PathBuf>,
    /// Pinned folders (keys from `settings::folder_key`), and pin/unpin requests.
    pinned: std::collections::HashSet<String>,
    pin: Option<(PathBuf, bool)>,
    new_tab: Option<PathBuf>,
}

/// Marks `resp` as the drop target when a drag is over it.
pub fn drop_hover(resp: &egui::Response, dragging: bool, path: &Path, out: &mut Option<PathBuf>) {
    if dragging && resp.contains_pointer() {
        *out = Some(path.to_path_buf());
        let stroke = egui::Stroke::new(1.5, resp.ctx.global_style().visuals.selection.stroke.color);
        resp.ctx.layer_painter(resp.layer_id).rect_stroke(resp.rect, 3.0, stroke, egui::StrokeKind::Inside);
    }
}

/// Expandable node: arrow + clickable label; children are listed lazily the first time it opens.
#[allow(clippy::too_many_arguments)]
fn tree_node(
    ui: &mut Ui,
    tree: &std::collections::HashMap<PathBuf, Vec<String>>,
    labels: &std::collections::HashMap<PathBuf, String>,
    cur: &PathBuf,
    path: &PathBuf,
    (icon, label): (Icon, &str),
    out: &mut TreeOut,
    depth: usize,
) -> egui::Response {
    let id = ui.make_persistent_id(("tree", path));
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let open = state.is_open();
    let (_, header, _) =
        state.show_header(ui, |ui| ui_common::nav_item(ui, Some(icon), label, path == cur)).body(|ui| {
            match tree.get(path) {
                Some(children) if depth < 32 => {
                    for name in children {
                        let child = path.join(name);
                        // A phone's folders are named by ID; show their display names.
                        let shown = labels.get(&child).map_or(name.as_str(), String::as_str);
                        tree_node(ui, tree, labels, cur, &child, (Icon::FolderFilled, shown), out, depth + 1);
                    }
                    if children.is_empty() {
                        ui.label(RichText::new("(empty)").small().weak());
                    }
                }
                Some(_) => {}
                None => {
                    ui.spinner();
                }
            }
        });
    if open && !tree.contains_key(path) {
        out.load.push(path.clone());
    }
    if header.inner.clicked() {
        out.go = Some(path.clone());
    }
    drop_hover(&header.inner, out.dragging, path, &mut out.drop);
    // Drive roots keep their own menu (Disconnect); folders can be pinned (not a phone's,
    // which have no path to keep).
    if path.parent().is_some() && !path.starts_with(explorer_core::SHELL) {
        let pinned = out.pinned.contains(&crate::settings::folder_key(path));
        let mut req = None;
        header.inner.context_menu(|ui| {
            if ui.button("Open in new tab").clicked() {
                req = Some(None);
                ui.close();
            }
            let label = if pinned { "Unpin from Quick access" } else { "Pin to Quick access" };
            if ui.button(label).clicked() {
                req = Some(Some(!pinned));
                ui.close();
            }
        });
        match req {
            Some(Some(pin)) => out.pin = Some((path.clone(), pin)),
            Some(None) => out.new_tab = Some(path.clone()),
            None => {}
        }
    }
    header.inner
}

/// "Windows (C:)", "Local Disk (D:)", "USB Drive (E:)", as Explorer names drives.
pub fn drive_label(d: &explorer_sys::Drive) -> String {
    let root = d.root.to_string_lossy();
    let letter = root.trim_end_matches('\\');
    match (d.label.is_empty(), d.kind) {
        (false, _) => format!("{} ({letter})", d.label),
        (true, DriveKind::Fixed) => format!("Local Disk ({letter})"),
        (true, DriveKind::Removable) => format!("USB Drive ({letter})"),
        (true, DriveKind::Network) => format!("Network ({letter})"),
        (true, DriveKind::Optical) => format!("DVD Drive ({letter})"),
        (true, DriveKind::Other) => letter.to_string(),
    }
}

/// The used-space bar and "X free of Y" under a drive or phone in the navigation pane.
fn space_bar(ui: &mut Ui, total: u64, free: u64) {
    let used = 1.0 - free as f32 / total as f32;
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.add(egui::ProgressBar::new(used).desired_width(ui.available_width() - 4.0).desired_height(5.0).fill({
            let p = ui_common::Palette::of(ui);
            if used > 0.9 { p.danger } else { p.accent }
        }));
    });
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(format!("{} free of {}", format_bytes(free), format_bytes(total))).small().weak());
    });
}

pub fn sidebar(app: &mut App, ui: &mut Ui) {
    egui::Panel::left("sidebar").resizable(true).default_size(190.0).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(6.0);
            let mut go: Option<PathBuf> = None;
            let mut unmap: Option<char> = None;
            let dragging = app.drag.is_some();
            let pinned = app.persist.settings.pinned.iter().map(|p| crate::settings::folder_key(p)).collect();
            let mut tree_out = TreeOut { dragging, pinned, ..Default::default() };
            let mut place_drop: Option<PathBuf> = None;
            let mut place = |ui: &mut Ui, icon: Icon, name: &str, path: &PathBuf, cur: &PathBuf| {
                let r = ui_common::nav_item(ui, Some(icon), name, path == cur);
                if path.as_os_str() != explorer_core::NETWORK {
                    drop_hover(&r, dragging, path, &mut place_drop);
                }
                r.clicked()
            };
            let home_page = PathBuf::from(explorer_core::HOME);
            if place(ui, Icon::Home, "Home", &home_page, &app.cur) {
                go = Some(home_page);
            }
            let gallery = PathBuf::from(explorer_core::GALLERY);
            if place(ui, Icon::Gallery, "Gallery", &gallery, &app.cur) {
                go = Some(gallery);
            }
            for (name, path) in &app.places {
                let icon = match *name {
                    "Desktop" => Icon::Desktop,
                    "Downloads" => Icon::Downloads,
                    "Documents" => Icon::Document,
                    "Pictures" => Icon::Pictures,
                    "Music" => Icon::Music,
                    "Videos" => Icon::Videos,
                    _ => Icon::Folder,
                };
                tree_node(ui, &app.tree, &app.shell_labels, &app.cur, path, (icon, name), &mut tree_out, 0);
            }
            // Quick access pins, after the standard folders (which are always there).
            for path in &app.persist.settings.pinned {
                let name =
                    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                tree_node(ui, &app.tree, &app.shell_labels, &app.cur, path, (Icon::Pin, &name), &mut tree_out, 0);
            }
            ui.add_space(6.0);
            ui.separator();
            let this_pc = PathBuf::from(explorer_core::THIS_PC);
            if place(ui, Icon::ThisPc, "This PC", &this_pc, &app.cur) {
                go = Some(this_pc);
            }
            for d in &app.drives {
                let root = d.root.to_string_lossy();
                let letter = root.trim_end_matches('\\');
                let label = drive_label(d);
                let icon = match d.kind {
                    DriveKind::Network => Icon::NetworkDrive,
                    DriveKind::Optical => Icon::Disc,
                    DriveKind::Removable => Icon::Usb,
                    _ => Icon::Drive,
                };
                let r =
                    tree_node(ui, &app.tree, &app.shell_labels, &app.cur, &d.root, (icon, &label), &mut tree_out, 0);
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
                    space_bar(ui, d.total, d.free);
                }
            }
            // Phones and cameras, as Explorer lists them with the drives.
            for d in &app.devices {
                tree_node(ui, &app.tree, &app.shell_labels, &app.cur, &d.path, (Icon::Usb, &d.label), &mut tree_out, 0);
                if d.total > 0 {
                    space_bar(ui, d.total, d.free);
                }
            }
            ui.add_space(6.0);
            let libraries = PathBuf::from(explorer_core::LIBRARIES);
            if place(ui, Icon::Libraries, "Libraries", &libraries, &app.cur) {
                go = Some(libraries);
            }
            let network = PathBuf::from(explorer_core::NETWORK);
            if place(ui, Icon::Network, "Network", &network, &app.cur) {
                go = Some(network);
            }
            if ui_common::nav_item(ui, Some(Icon::Share), "Nearby sharing", app.nearby.open)
                .on_hover_text("Send and receive with phones (LocalSend)")
                .clicked()
            {
                app.nearby.open = !app.nearby.open;
            }
            let bin = PathBuf::from(explorer_core::RECYCLE_BIN);
            if place(ui, Icon::RecycleBin, "Recycle Bin", &bin, &app.cur) {
                go = Some(bin);
            }
            if let Some(l) = unmap {
                app.unmap(l);
            }
            if go.is_none() {
                go = tree_out.go;
            }
            if let Some(t) = place_drop.or(tree_out.drop) {
                app.drop_target = Some(t);
            }
            for p in tree_out.load {
                app.load_tree(p);
            }
            match tree_out.pin {
                Some((p, true)) => app.pin(p),
                Some((p, false)) => app.unpin(&p),
                None => {}
            }
            if let Some(p) = tree_out.new_tab {
                app.new_tab(p, false);
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
            let n = app.view.len();
            ui.label(if n == 1 { "1 item".to_string() } else { format!("{n} items") });
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
pub enum Action {
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
    let place = explorer_core::classify(&app.cur);
    let shell_menus = app.hwnd != 0
        && (!place.is_page() || matches!(place, explorer_core::Place::Server(_) | explorer_core::Place::Shell(_)));
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

    // Compact view, as in Explorer: tighter rows.
    let row_h = if app.compact { 18.0 } else { 22.0 };
    let mut sort_click = None;
    // Right-clicking any header opens the column chooser, as in Explorer.
    let mut toggle_col: Option<SortKey> = None;
    let (col_created, col_attrs) = (app.col_created, app.col_attrs);
    let prop_cols = app.prop_cols.clone();
    // Inside a OneDrive folder, ordinary files are "available on this device".
    let in_sync_root = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|root| !root.is_empty() && app.cur.starts_with(&root)));
    let mut toggle_prop: Option<explorer_core::Prop> = None;
    let mut wanted_props: Vec<PathBuf> = Vec::new();
    // `key`: None for the property columns, which don't sort.
    let mut header =
        |h: &mut egui_extras::TableRow, key: Option<SortKey>, title: &str, app: &App, out: &mut Option<SortKey>| {
            h.col(|ui| {
                let sorted = key.is_some_and(|k| app.sort == k);
                let arrow = if sorted { if app.ascending { " ⏶" } else { " ⏷" } } else { "" };
                let resp = ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false));
                if resp.clicked() && key.is_some() {
                    *out = key;
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
                    for p in explorer_core::Prop::ALL {
                        let mut on = app.prop_cols.contains(&p);
                        if ui.checkbox(&mut on, p.label()).clicked() {
                            toggle_prop = Some(p);
                            ui.close();
                        }
                    }
                });
            });
        };
    let n = app.view.len();
    let weak = ui.visuals().weak_text_color();
    let mut wanted_icons: Vec<IconKey> = Vec::new();
    // Drag and drop, collected while drawing (the rows only borrow `app`).
    let dragging = app.drag.is_some();
    let mut drag_start: Option<String> = None;
    let mut hover_target: Option<PathBuf> = None;
    if app.view_mode != ViewMode::Details {
        crate::grid::show(app, ui, &mut action, &mut wanted_icons, shell_menus, in_bin);
    } else {
        let band_active = ui.data(|d| d.get_temp::<[f32; 2]>(egui::Id::new(DETAILS_BAND)).is_some());
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click_and_drag())
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
        for _ in &prop_cols {
            table = table.column(Column::initial(120.0).at_least(50.0).clip(true));
        }
        // Column widths are remembered per column set.
        table = table.id_salt(("details", col_created, col_attrs, app.pane, &prop_cols));
        // With Group by on, a heading row (negative: -1 - group index) starts each group.
        let rows_map: Vec<isize> = app
            .groups
            .iter()
            .enumerate()
            .flat_map(|(k, g)| std::iter::once(-1 - k as isize).chain((g.start..g.start + g.len).map(|p| p as isize)))
            .collect();
        let grouped = !rows_map.is_empty();
        let n_rows = if grouped { rows_map.len() } else { n };
        let n_cols = 4 + usize::from(col_created) + usize::from(col_attrs) + prop_cols.len();
        if let Some(pos) = app.scroll_to.take().filter(|&p| p < n) {
            let row = if grouped { rows_map.iter().position(|&r| r == pos as isize).unwrap_or(pos) } else { pos };
            table = table.scroll_to_row(row, None);
        }
        // Rows drawn this frame (table row, screen rect), and a rubber band started on one.
        let mut drawn: Vec<(usize, egui::Rect)> = Vec::new();
        let mut band_start: Option<[f32; 2]> = None;
        table
            .header(row_h, |mut h| {
                header(&mut h, Some(SortKey::Name), "Name", app, &mut sort_click);
                let bin = app.in_recycle_bin();
                header(
                    &mut h,
                    Some(SortKey::Modified),
                    if bin { "Date deleted" } else { "Date modified" },
                    app,
                    &mut sort_click,
                );
                header(
                    &mut h,
                    Some(SortKey::Type),
                    if bin { "Original location" } else { "Type" },
                    app,
                    &mut sort_click,
                );
                header(&mut h, Some(SortKey::Size), "Size", app, &mut sort_click);
                if col_created {
                    header(&mut h, Some(SortKey::Created), "Date created", app, &mut sort_click);
                }
                if col_attrs {
                    header(&mut h, Some(SortKey::Attributes), "Attributes", app, &mut sort_click);
                }
                for p in &prop_cols {
                    header(&mut h, None, p.label(), app, &mut sort_click);
                }
            })
            .body(|body| {
                body.rows(row_h, n_rows, |mut row| {
                    let pos = if grouped {
                        match rows_map[row.index()] {
                            h if h < 0 => {
                                let g = &app.groups[(-1 - h) as usize];
                                row.col(|ui| {
                                    ui.label(RichText::new(&g.label).strong().color(ui.visuals().hyperlink_color));
                                    ui.label(RichText::new(format!("  ({})", g.len)).weak());
                                });
                                for _ in 1..n_cols {
                                    row.col(|_| {});
                                }
                                drawn.push((row.index(), row.response().rect));
                                return;
                            }
                            p => p as usize,
                        }
                    } else {
                        row.index()
                    };
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
                    let mut name_rect = egui::Rect::NOTHING;
                    row.col(|ui| {
                        if app.item_checkboxes {
                            let mut on = app.selected.contains(&e.name);
                            if ui.checkbox(&mut on, "").changed() {
                                action = Some(Action::Click(pos, egui::Modifiers::COMMAND | egui::Modifiers::CTRL));
                            }
                        }
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
                                ui.label(if e.is_dir { icon.color(ui_common::Palette::of(ui).folder) } else { icon });
                            }
                        }
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(name_text).selectable(false).truncate());
                        if e.link {
                            ui.label(RichText::new(" ⤴").weak());
                        }
                        name_rect = ui.min_rect();
                        // Keep the band's end in view: the table scrolls while it is dragged.
                        if band_active
                            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
                            && !ui.clip_rect().y_range().contains(p.y)
                        {
                            ui.scroll_to_rect(egui::Rect::from_center_size(p, egui::vec2(1.0, 1.0)), None);
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
                    if !prop_cols.is_empty() {
                        let path = app.child(&e.name);
                        let only_status = prop_cols.iter().all(|&p| p == explorer_core::Prop::Status);
                        let values = match app.props.get(&path) {
                            Some(v) => v.as_ref(),
                            None => {
                                if !only_status {
                                    wanted_props.push(path);
                                }
                                None
                            }
                        };
                        for p in &prop_cols {
                            let k = explorer_core::Prop::ALL.iter().position(|q| q == p).unwrap_or(0);
                            let text = if *p == explorer_core::Prop::Status {
                                explorer_core::cloud_status(e.attrs, in_sync_root)
                            } else {
                                values.and_then(|v| v.get(k)).map_or("", String::as_str)
                            };
                            row.col(|ui| {
                                ui.add(egui::Label::new(RichText::new(text).color(weak)).selectable(false));
                            });
                        }
                    }
                    let resp = row.response();
                    drawn.push((row.index(), resp.rect));
                    if resp.drag_started() {
                        // As in Explorer: dragging the name moves the item, dragging anywhere
                        // else on the row draws the selection rectangle.
                        let origin = resp.ctx.input(|i| i.pointer.press_origin());
                        match origin {
                            Some(p) if !name_rect.contains(p) => {
                                let frac = (p.y - resp.rect.top()) / resp.rect.height().max(1.0);
                                band_start = Some([p.x, row.index() as f32 + frac]);
                            }
                            _ if !in_bin => drag_start = Some(e.name.clone()),
                            _ => {}
                        }
                    }
                    if e.is_dir && !app.is_dragged(&e.name) {
                        drop_hover(&resp, dragging, &app.child(&e.name), &mut hover_target);
                    }
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
        details_band(app, ui, band_start, &drawn, &rows_map);
    }

    app.request_icons(wanted_icons);
    app.request_thumbs();
    if let Some(n) = drag_start {
        app.drag_source(&n);
    }
    if let Some(t) = hover_target {
        app.drop_target = Some(t);
    }
    if let Some(p) = toggle_prop {
        if app.prop_cols.contains(&p) {
            app.prop_cols.retain(|&q| q != p);
        } else {
            app.prop_cols.push(p);
            let order = |q: &explorer_core::Prop| explorer_core::Prop::ALL.iter().position(|x| x == q);
            app.prop_cols.sort_by_key(order);
        }
    }
    app.props_wanted.extend(wanted_props);
    app.request_props();
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
        Dialog::Rename { text, error, opened, others, .. } => {
            (if others.is_empty() { "Rename" } else { "Rename all selected" }, text, error, opened)
        }
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
