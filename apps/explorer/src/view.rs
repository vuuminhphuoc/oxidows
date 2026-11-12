//! Drawing: toolbar with address bar, sidebar, virtualised file table, status bar, dialogs.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, RichText, Sense, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use explorer_core::{
    DateModifiedBucket, FilterColumn, HeaderSizeBucket, NameBucket, SortKey, classify, format_attributes, format_bytes,
    format_size_kb, format_time, icon_for, parent_path,
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
            enum TabAction {
                New,
                Duplicate(usize),
                MoveToNewWindow(usize),
                Close(usize),
                CloseOthers(usize),
                CloseRight(usize),
            }
            let (mut switch, mut close, mut tab_choice) = (None, None, None);
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
                let bg_slot = ui.painter().add(egui::Shape::Noop);
                let r = ui
                    .add(
                        egui::Button::selectable(active, Icon::FolderFilled.with(short))
                            .frame_when_inactive(false)
                            .fill(egui::Color32::TRANSPARENT)
                            .min_size(egui::vec2(120.0, 28.0))
                            .sense(Sense::click_and_drag()),
                    )
                    .on_hover_text(&title);
                if ui.is_rect_visible(r.rect) {
                    let p = ui_common::Palette::of(ui);
                    if active {
                        ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, p.hover));
                    } else {
                        let t = ui_common::motion::fade(ui.ctx(), r.id, r.hovered());
                        if t > 0.0 {
                            let fill = ui_common::motion::lerp_color(egui::Color32::TRANSPARENT, p.hover, t);
                            ui.painter().set(bg_slot, egui::epaint::RectShape::filled(r.rect, 4.0, fill));
                        }
                    }
                }
                rects.push(r.rect);
                if app.drag.is_some() && !active && r.contains_pointer() {
                    app.frame_hover = Some((crate::dnd::HoverTarget::Tab(i), None));
                }
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
                r.context_menu(|ui| {
                    if ui_common::MenuItem::new("New tab").icon(Icon::Add).shortcut("Ctrl+T").ui(ui).clicked() {
                        tab_choice = Some(TabAction::New);
                        ui.close();
                    }
                    if ui_common::MenuItem::new("Duplicate tab").icon(Icon::Copy).ui(ui).clicked() {
                        tab_choice = Some(TabAction::Duplicate(i));
                        ui.close();
                    }
                    if ui_common::MenuItem::new("Move tab to new window")
                        .icon(Icon::NewWindow)
                        .enabled(app.tabs.len() > 1)
                        .ui(ui)
                        .clicked()
                    {
                        tab_choice = Some(TabAction::MoveToNewWindow(i));
                        ui.close();
                    }
                    ui.separator();
                    if ui_common::MenuItem::new("Close tab").icon(Icon::Close).shortcut("Ctrl+W").ui(ui).clicked() {
                        tab_choice = Some(TabAction::Close(i));
                        ui.close();
                    }
                    if ui_common::MenuItem::new("Close other tabs")
                        .icon(Icon::ClosePane)
                        .enabled(app.tabs.len() > 1)
                        .ui(ui)
                        .clicked()
                    {
                        tab_choice = Some(TabAction::CloseOthers(i));
                        ui.close();
                    }
                    if ui_common::MenuItem::new("Close tabs to the right")
                        .icon(Icon::ClosePaneMirrored)
                        .enabled(i + 1 < app.tabs.len())
                        .ui(ui)
                        .clicked()
                    {
                        tab_choice = Some(TabAction::CloseRight(i));
                        ui.close();
                    }
                });
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
            if let Some(choice) = tab_choice {
                match choice {
                    TabAction::New => app.new_tab(PathBuf::from(explorer_core::HOME), true),
                    TabAction::Duplicate(i) => app.duplicate_tab(i),
                    TabAction::MoveToNewWindow(i) => app.tear_off_tab(i),
                    TabAction::Close(i) => app.close_tab(i),
                    TabAction::CloseOthers(i) => app.close_other_tabs(i),
                    TabAction::CloseRight(i) => app.close_tabs_right(i),
                }
            } else if let (Some(i), Some(p)) = (released, pointer)
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

/// The "See more" (…) menu: each row has an icon, and shortcut keys sit at the right of the row
/// as in Windows 11's menus. The View rows show a picture of each layout.
fn more_menu(app: &mut App, ui: &mut Ui) {
    use ui_common::{MenuItem, menu_item};
    if menu_item(ui, Icon::NetworkDrive, "Map network drive…", None).clicked() {
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
        MenuItem::new("Disconnect network drive").icon(Icon::DisconnectDrive).submenu(ui, |ui| {
            for l in mapped {
                if menu_item(ui, Icon::NetworkDrive, &format!("{l}:"), None).clicked() {
                    app.unmap(l);
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    let undo = app.undo_stack.last().map(|u| format!("Undo {}", u.label()));
    let undo_item = MenuItem::new(undo.as_deref().unwrap_or("Undo")).icon(Icon::Undo).shortcut("Ctrl+Z");
    if undo_item.enabled(undo.is_some()).ui(ui).clicked() {
        app.undo();
        ui.close();
    }
    let redo = app.redo_stack.last().map(|u| format!("Redo {}", u.label()));
    let redo_item = MenuItem::new(redo.as_deref().unwrap_or("Redo")).icon(Icon::Redo).shortcut("Ctrl+Y");
    if redo_item.enabled(redo.is_some()).ui(ui).clicked() {
        app.redo();
        ui.close();
    }
    if menu_item(ui, Icon::SelectAll, "Select all", Some("Ctrl+A")).clicked() {
        app.selected = app.view.iter().map(|&i| app.entries[i].name.clone()).collect();
        ui.close();
    }
    if menu_item(ui, Icon::SelectNone, "Select none", None).clicked() {
        app.selected.clear();
        ui.close();
    }
    if menu_item(ui, Icon::InvertSelection, "Invert selection", None).clicked() {
        app.invert_selection();
        ui.close();
    }
    ui.separator();
    if !app.in_virtual_place() {
        let cur = app.cur.clone();
        let pinned = app.is_pinned(&cur);
        let (icon, label) =
            if pinned { (Icon::Unpin, "Unpin from Quick access") } else { (Icon::Pin, "Pin to Quick access") };
        if menu_item(ui, icon, label, None).clicked() {
            if pinned {
                app.unpin(&cur);
            } else {
                app.pin(cur);
            }
            ui.close();
        }
    }
    if menu_item(ui, Icon::NewWindow, "New window", Some("Ctrl+N")).clicked() {
        app.new_window();
        ui.close();
    }
    if menu_item(ui, Icon::Settings, "Options", None)
        .on_hover_text("Default file manager (Win+E, opening folders)")
        .clicked()
    {
        app.options_open = true;
        ui.close();
    }
    ui.separator();
    MenuItem::new("Theme").icon(Icon::Theme).submenu(ui, |ui| {
        use crate::settings::Theme;
        for (t, label) in Theme::ALL {
            let label = if t == Theme::System { "Use system setting" } else { label };
            if MenuItem::new(label).selected(app.persist.settings.theme == t).ui(ui).clicked() {
                app.persist.settings.theme = t;
                app.persist.settings.apply_theme(ui.ctx());
                ui.close();
            }
        }
    });
    ui.separator();
    if menu_item(ui, Icon::Terminal, "Open in Terminal", None).clicked() {
        if let Err(e) = explorer_sys::open_terminal(&app.cur) {
            app.status = Some(format!("Terminal: {e}"));
        }
        ui.close();
    }
    if menu_item(ui, Icon::Properties, "Properties", Some("Alt+Enter")).clicked() {
        let c = app.cur.clone();
        app.show_properties(&c);
        ui.close();
    }
    ui.separator();
    if menu_item(ui, Icon::Info, "About and updates", None).clicked() {
        app.about_open = true;
        ui.close();
    }
}

/// The command bar's New menu: Folder, then the file types Windows registers for New.
pub(crate) fn new_menu(app: &mut App, ui: &mut Ui) {
    use ui_common::menu_item;
    if menu_item(ui, Icon::NewFolder, "Folder", Some("Ctrl+Shift+N")).clicked() {
        app.begin_new_folder();
        ui.close();
    }
    let items = app.new_items.get_or_insert_with(explorer_sys::new_items).clone();
    // Shortcut sits with Folder, the other types follow under a line, as in Explorer.
    let (links, others): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| i.ext.eq_ignore_ascii_case(".lnk"));
    for (k, it) in links.iter().chain(&others).enumerate() {
        if k == links.len() && !others.is_empty() {
            ui.separator();
        }
        if menu_item(ui, Icon::Document, &it.name, None).clicked() {
            app.create_new_item(it);
            ui.close();
        }
    }
}

/// The sort keys and direction part of the Sort menu.
pub(crate) fn sort_keys_menu(app: &mut App, ui: &mut Ui) {
    use ui_common::MenuItem;
    let bin = app.in_recycle_bin();
    for (k, label) in [
        (SortKey::Name, "Name"),
        (SortKey::Modified, if bin { "Date deleted" } else { "Date modified" }),
        (SortKey::Type, if bin { "Original location" } else { "Type" }),
        (SortKey::Size, "Size"),
    ] {
        if MenuItem::new(label).selected(app.sort == k).ui(ui).clicked() {
            if app.sort != k {
                app.set_sort(k);
            }
            ui.close();
        }
    }
    MenuItem::new("More").submenu(ui, |ui| {
        for (k, label) in [(SortKey::Created, "Date created"), (SortKey::Attributes, "Attributes")] {
            if MenuItem::new(label).selected(app.sort == k).ui(ui).clicked() {
                if app.sort != k {
                    app.set_sort(k);
                }
                ui.close();
            }
        }
    });
    ui.separator();
    for (asc, label) in [(true, "Ascending"), (false, "Descending")] {
        if MenuItem::new(label).selected(app.ascending == asc).ui(ui).clicked() {
            app.ascending = asc;
            ui.close();
        }
    }
}

/// The grouping options part of the Sort menu.
pub(crate) fn group_menu(app: &mut App, ui: &mut Ui) {
    use explorer_core::GroupBy;
    use ui_common::MenuItem;
    for (g, label) in [
        (GroupBy::Name, "Name"),
        (GroupBy::Modified, "Date modified"),
        (GroupBy::Type, "Type"),
        (GroupBy::Size, "Size"),
        (GroupBy::None, "(None)"),
    ] {
        if MenuItem::new(label).selected(app.group_by == g).ui(ui).clicked() {
            app.group_by = g;
            ui.close();
        }
    }
    ui.separator();
    for (asc, label) in [(true, "Ascending"), (false, "Descending")] {
        if MenuItem::new(label).selected(app.group_ascending == asc).ui(ui).clicked() {
            app.group_ascending = asc;
            ui.close();
        }
    }
}

/// The command bar's Sort menu: sort key, direction, and Group by.
fn sort_menu(app: &mut App, ui: &mut Ui) {
    use ui_common::MenuItem;
    sort_keys_menu(app, ui);
    ui.separator();
    MenuItem::new("Group by").icon(Icon::GroupBy).submenu(ui, |ui| {
        group_menu(app, ui);
    });
}

/// The command bar's View menu: layouts, Compact view, and Show.
pub(crate) fn view_menu(app: &mut App, ui: &mut Ui) {
    use ui_common::{MenuItem, ViewIcon, menu_check};
    for (m, label, keys) in ViewMode::ALL {
        if MenuItem::new(label).icon(m.icon()).shortcut(keys).selected(app.view_mode == m).ui(ui).clicked() {
            app.view_mode = m;
            ui.close();
        }
    }
    if menu_check(ui, ViewIcon::Compact, "Compact view", &mut app.compact).clicked() {
        ui.close();
    }
    MenuItem::new("Show").icon(Icon::Show).submenu(ui, |ui| {
        use crate::pane::Pane;
        for (p, icon, label, keys) in [
            (Pane::Details, ViewIcon::DetailsPane, "Details pane", "Alt+Shift+P"),
            (Pane::Preview, ViewIcon::PreviewPane, "Preview pane", "Alt+P"),
        ] {
            if MenuItem::new(label).icon(icon).shortcut(keys).checked(app.pane == p).ui(ui).clicked() {
                app.toggle_pane(p);
                ui.close();
            }
        }
        ui.separator();
        let none = ui_common::MenuIcon::None;
        menu_check(ui, none, "Item check boxes", &mut app.item_checkboxes);
        menu_check(ui, none, "File name extensions", &mut app.show_extensions);
        menu_check(ui, none, "Hidden items", &mut app.show_hidden);
        let protected = MenuItem::new("Protected operating system files").checked(app.show_protected);
        if protected.enabled(app.show_hidden).ui(ui).clicked() {
            app.show_protected = !app.show_protected;
        }
    });
}

fn search_options_menu(app: &mut App, ui: &mut Ui) {
    use explorer_core::{DateFilter, Kind, SizeFilter};
    use ui_common::MenuItem;
    MenuItem::new("Date modified").submenu(ui, |ui| {
        for (d, label) in DateFilter::ALL {
            if MenuItem::new(label).selected(app.search_filters.date == Some(d)).ui(ui).clicked() {
                app.search_filters.date = if app.search_filters.date == Some(d) { None } else { Some(d) };
                ui.close();
            }
        }
    });
    MenuItem::new("Kind").submenu(ui, |ui| {
        for (k, label) in Kind::SEARCH_OPTIONS {
            if MenuItem::new(label).selected(app.search_filters.kind == Some(k)).ui(ui).clicked() {
                app.search_filters.kind = if app.search_filters.kind == Some(k) { None } else { Some(k) };
                ui.close();
            }
        }
    });
    MenuItem::new("Size").submenu(ui, |ui| {
        for (s, label) in SizeFilter::ALL {
            if MenuItem::new(label).selected(app.search_filters.size == Some(s)).ui(ui).clicked() {
                app.search_filters.size = if app.search_filters.size == Some(s) { None } else { Some(s) };
                ui.close();
            }
        }
    });
    ui.separator();
    if MenuItem::new("Clear filters").enabled(!app.search_filters.is_empty()).ui(ui).clicked() {
        app.search_filters.clear();
        ui.close();
    }
}

pub fn toolbar(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // Right-click Back or Forward for the recent locations, as in Explorer.
            let recent = app.history.recent(&app.cur);
            let mut jump = None;
            let history_menu = |r: &egui::Response, jump: &mut Option<isize>| {
                r.context_menu(|ui| {
                    for (steps, p) in &recent {
                        let label = app.crumbs(p).last().map_or_else(|| p.display().to_string(), |c| c.0.clone());
                        let text = if *steps == 0 { RichText::new(label).strong() } else { RichText::new(label) };
                        if ui.button(text).on_hover_text(p.display().to_string()).clicked() {
                            *jump = Some(*steps);
                            ui.close();
                        }
                    }
                });
            };
            let back = ui_common::tool_button(ui, Icon::Back, app.history.can_back(), "Back (Alt+Left Arrow)");
            history_menu(&back, &mut jump);
            let forward =
                ui_common::tool_button(ui, Icon::Forward, app.history.can_forward(), "Forward (Alt+Right Arrow)");
            history_menu(&forward, &mut jump);
            if back.clicked() {
                app.back();
            } else if forward.clicked() {
                app.forward();
            } else if let Some(steps) = jump {
                app.jump_history(steps);
            }
            if ui_common::tool_button(
                ui,
                Icon::Up,
                parent_path(&app.cur).is_some(),
                "Up to the parent folder (Alt+Up Arrow)",
            )
            .clicked()
            {
                app.up();
            }
            if ui_common::tool_button(ui, Icon::Refresh, true, "Refresh (F5)").clicked() {
                app.refresh();
                app.load_drives();
            }

            // Search box on the right, address bar takes the rest.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // "Search Downloads", as Explorer names the box after the folder.
                let hint = format!("Search {}", app.tab_title(app.active_tab));
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.filter).hint_text(Icon::Search.with(hint)).desired_width(200.0),
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

/// The command bar under the address bar, as in Windows 11: New, clipboard and file commands,
/// Sort, View, "See more", and the Details pane switch on the right.
pub fn command_bar(app: &mut App, ui: &mut Ui) {
    use ui_common::{command_button, command_menu, command_separator};
    egui::Panel::top("commands").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let virt = app.in_virtual_place();
            let in_bin = app.in_recycle_bin();
            let has_sel = !app.selected.is_empty();
            if in_bin {
                if command_button(ui, Icon::RecycleBin, "Empty Recycle Bin", true, "").clicked() {
                    app.empty_bin();
                }
                if command_button(ui, Icon::Restore, "Restore the selected items", has_sel, "").clicked() {
                    app.restore_selection();
                }
            } else {
                ui.add_enabled_ui(!virt, |ui| {
                    command_menu(ui, Icon::Add, "New", "Create a new item in the current location", |ui| {
                        new_menu(app, ui)
                    });
                });
            }
            command_separator(ui);
            let files = has_sel && !in_bin;
            if command_button(ui, Icon::Cut, "", files, "Cut (Ctrl+X)").clicked() {
                app.copy_selection(true);
            }
            if command_button(ui, Icon::Copy, "", files, "Copy (Ctrl+C)").clicked() {
                app.copy_selection(false);
            }
            let paste = !virt && !in_bin && app.can_paste();
            if command_button(ui, Icon::Paste, "", paste, "Paste (Ctrl+V)").clicked() {
                let c = app.cur.clone();
                app.paste_into(c);
            }
            if command_button(ui, Icon::Rename, "", has_sel && !virt, "Rename (F2)").clicked() {
                app.begin_rename();
            }
            if command_button(ui, Icon::Share, "", !in_bin, "Share with a nearby device (LocalSend)").clicked() {
                app.nearby.queue = app.selected_paths();
                app.nearby.open = true;
            }
            if command_button(ui, Icon::Delete, "", has_sel, "Delete (Delete, Ctrl+D)").clicked() {
                app.delete_selection(false);
            }
            if classify(&app.cur) == explorer_core::Place::Gallery {
                command_menu(ui, Icon::Folder, "Collection", "Gallery collection folders", |ui| {
                    collection_menu(app, ui);
                });
            }
            command_separator(ui);
            command_menu(ui, Icon::Sort, "Sort", "Sort and group the items", |ui| sort_menu(app, ui));
            command_menu(ui, Icon::View, "View", "Layout and which items and panes to show", |ui| view_menu(app, ui));
            if !app.filter.trim().is_empty() || app.deep_search.is_some() {
                let n = app.search_filters.count();
                let label = if n > 0 { format!("Search options ({n})") } else { "Search options".to_string() };
                command_menu(ui, Icon::Filter, &label, "Filter the search results", |ui| {
                    search_options_menu(app, ui);
                });
            }
            command_separator(ui);
            ui_common::tool_menu(ui, Icon::More, "See more", |ui| more_menu(app, ui));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                use crate::pane::Pane;
                let on = app.pane == Pane::Details;
                let r = ui_common::view_toggle(
                    ui,
                    ui_common::ViewIcon::DetailsPane,
                    "Details",
                    on,
                    "Show the details pane (Alt+Shift+P)",
                );
                if r.clicked() {
                    app.toggle_pane(Pane::Details);
                }
            });
        });
        ui.add_space(2.0);
    });
}

fn collection_menu(app: &mut App, ui: &mut Ui) {
    ui.label(RichText::new("Collection folders").strong());
    ui.separator();
    let is_default = app.persist.settings.gallery_folders.is_empty();
    if is_default {
        ui.label("Pictures (default)");
    } else {
        let mut remove_idx = None;
        for (i, p) in app.persist.settings.gallery_folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(p.display().to_string());
                if ui.small_button("✕").clicked() {
                    remove_idx = Some(i);
                }
            });
        }
        if let Some(i) = remove_idx {
            app.persist.settings.gallery_folders.remove(i);
            app.refresh();
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Add folder…").clicked() {
        app.dialog = Some(crate::Dialog::AddGalleryFolder(String::new()));
        ui.close();
    }
    if !is_default && ui.button("Reset to default").clicked() {
        app.persist.settings.gallery_folders.clear();
        app.refresh();
        ui.close();
    }
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
        let text_len = text.chars().count();
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if std::mem::take(&mut app.address_select_all)
            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id)
        {
            let range = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text_len));
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), r.id);
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
        // Nothing may spill over the search box, however long the path.
        ui.shrink_clip_rect(ui.max_rect());
        let crumbs = app.crumbs(&app.cur);
        let local = classify(&app.cur) == explorer_core::Place::Local;
        // Long paths: show the last segments that fit, like Explorer; the hidden ones are in
        // the "…" menu at the left.
        let pad = ui.spacing().button_padding.x * 2.0;
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_w = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), egui::Color32::WHITE).size().x;
        let arrow_w = text_w(" › ") + pad;
        let widths: Vec<f32> = crumbs.iter().map(|(label, _)| text_w(label) + pad + arrow_w).collect();
        let room = width - 16.0 - 8.0 - (text_w(" … ") + pad);
        let mut skip = crumbs.len().saturating_sub(1);
        let mut used = widths.last().copied().unwrap_or(0.0);
        while skip > 0 && used + widths[skip - 1] <= room {
            skip -= 1;
            used += widths[skip];
        }
        let mut go = None;
        let mut load = None;
        let mut over_child = false;
        if skip > 0 {
            let (r, _) = egui::containers::menu::MenuButton::from_button(
                egui::Button::new(RichText::new(" … ").weak()).frame_when_inactive(false),
            )
            .ui(ui, |ui| {
                for (label, path) in crumbs[..skip].iter().rev() {
                    if ui.button(label).clicked() {
                        go = Some(path.clone());
                        ui.close();
                    }
                }
            });
            over_child |= r.hovered();
        }
        for (i, (label, path)) in crumbs.iter().enumerate().skip(skip) {
            // Each segment and each arrow lights up on its own under the pointer, as in Explorer.
            let b = ui.add(egui::Button::new(label).frame_when_inactive(false));
            over_child |= b.hovered();
            if b.clicked() {
                go = Some(path.clone());
            }
            // The arrow after a segment lists its subfolders (the one on the path in bold).
            if local && classify(path) == explorer_core::Place::Local {
                let next = crumbs.get(i + 1).map(|c| &c.1);
                let (arrow, _) = egui::containers::menu::MenuButton::from_button(
                    egui::Button::new(RichText::new(" › ").weak()).frame_when_inactive(false),
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
        app.open_address_edit();
    }
}

/// The expand arrow of a navigation-pane folder: a thin chevron that turns from › to ⌄ as the
/// folder opens.
fn tree_chevron(ui: &mut Ui, openness: f32, r: &egui::Response, color: egui::Color32) {
    let c = r.rect.center();
    let angle = openness * std::f32::consts::FRAC_PI_2;
    let rot = egui::emath::Rot2::from_angle(angle);
    let pts = [egui::vec2(-1.5, -4.0), egui::vec2(2.5, 0.0), egui::vec2(-1.5, 4.0)].map(|v| c + rot * v);
    let color = if r.hovered() { ui.visuals().strong_text_color() } else { color };
    ui.painter().add(egui::Shape::line(pts.to_vec(), egui::Stroke::new(1.2, color)));
}

/// Requests and clicks collected while drawing the folder tree (it can't borrow `App` mutably).
#[derive(Default)]
struct TreeOut {
    /// The pointer is over the navigation pane (the expand chevrons show).
    pane_hovered: bool,
    go: Option<PathBuf>,
    load: Vec<PathBuf>,
    /// A drag is in progress: folders under the pointer are drop targets.
    dragging: bool,
    drop: Option<PathBuf>,
    hover_tree: Option<(egui::Id, PathBuf)>,
    /// Pinned folders (keys from `settings::folder_key`), and pin/unpin requests.
    pinned: std::collections::HashSet<String>,
    pin: Option<(PathBuf, bool)>,
    new_tab: Option<PathBuf>,
    reveal_done: bool,
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
    reveal: Option<&Path>,
    parent_animating: bool,
) -> egui::Response {
    let id = ui.make_persistent_id(("tree", path));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if let Some(target) = reveal
        && explorer_core::path_is_ancestor(path, target)
        && !state.is_open()
    {
        state.set_open(true);
    }
    let open = state.is_open();
    let openness_now = state.openness(ui.ctx());
    // Windows 11 shows the expand chevrons only while the pointer is over the pane.
    let (show, color) = (out.pane_hovered, ui_common::Palette::of(ui).text_secondary);
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        state.show_toggle_button(ui, move |ui, openness, r| {
            if show || r.hovered() {
                tree_chevron(ui, openness, r, color);
            }
        });
        ui_common::nav_item(ui, Some(icon), label, explorer_core::path_eq(path, cur))
    });
    let header = egui::InnerResponse { inner: row.inner, response: row.response.clone() };
    if let Some(target) = reveal
        && explorer_core::path_eq(path, target)
        && !parent_animating
    {
        header.inner.scroll_to_me(Some(Align::Center));
        out.reveal_done = true;
    }
    if out.dragging && !open && row.response.contains_pointer() {
        out.hover_tree = Some((id, path.clone()));
    }
    let branch_animating = parent_animating || (open && openness_now < 1.0);
    state.show_body_indented(&row.response, ui, |ui| {
        match tree.get(path) {
            Some(children) if depth < 32 => {
                let mut any_reveal_child = false;
                for name in children {
                    let child = path.join(name);
                    // A phone's folders are named by ID; show their display names.
                    let shown = labels.get(&child).map_or(name.as_str(), String::as_str);
                    let child_reveal = reveal.filter(|t| {
                        let m = explorer_core::path_eq(&child, t) || explorer_core::path_is_ancestor(&child, t);
                        any_reveal_child |= m;
                        m
                    });
                    tree_node(
                        ui,
                        tree,
                        labels,
                        cur,
                        &child,
                        (Icon::FolderFilled, shown),
                        out,
                        depth + 1,
                        child_reveal,
                        branch_animating,
                    );
                }
                if reveal.is_some_and(|t| explorer_core::path_is_ancestor(path, t)) && !any_reveal_child {
                    out.reveal_done = true;
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
pub fn drive_label(d: &explorer_sys::Drive, letters_before: bool) -> String {
    let root = d.root.to_string_lossy();
    let letter = root.trim_end_matches('\\');
    if letters_before {
        match (d.label.is_empty(), d.kind) {
            (false, _) => format!("({letter}) {}", d.label),
            (true, DriveKind::Fixed) => format!("({letter}) Local Disk"),
            (true, DriveKind::Removable) => format!("({letter}) USB Drive"),
            (true, DriveKind::Network) => format!("({letter}) Network"),
            (true, DriveKind::Optical) => format!("({letter}) DVD Drive"),
            (true, DriveKind::Other) => letter.to_string(),
        }
    } else {
        match (d.label.is_empty(), d.kind) {
            (false, _) => format!("{} ({letter})", d.label),
            (true, DriveKind::Fixed) => format!("Local Disk ({letter})"),
            (true, DriveKind::Removable) => format!("USB Drive ({letter})"),
            (true, DriveKind::Network) => format!("Network ({letter})"),
            (true, DriveKind::Optical) => format!("DVD Drive ({letter})"),
            (true, DriveKind::Other) => letter.to_string(),
        }
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
            let pane_hovered = ui.rect_contains_pointer(ui.clip_rect());
            let mut tree_out = TreeOut { dragging, pinned, pane_hovered, ..Default::default() };
            let reveal = app.reveal_tree.clone();
            // Pick the deepest matching root (broken by sidebar order) so Ctrl+Shift+E expands one branch.
            let reveal_root: Option<PathBuf> = reveal.as_ref().and_then(|target| {
                let roots = app
                    .places
                    .iter()
                    .map(|(_, p)| p)
                    .chain(app.onedrive.iter().map(|(_, p)| p))
                    .chain(app.persist.settings.pinned.iter())
                    .chain(app.drives.iter().map(|d| &d.root))
                    .chain(app.devices.iter().map(|d| &d.path));
                let mut best: Option<(usize, PathBuf)> = None;
                for r in roots {
                    if explorer_core::path_eq(r, target) || explorer_core::path_is_ancestor(r, target) {
                        let depth = r.components().count();
                        if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                            best = Some((depth, r.clone()));
                        }
                    }
                }
                best.map(|(_, p)| p)
            });
            let mut reveal_used = false;
            let mut take_reveal = |path: &Path| -> Option<&Path> {
                if !reveal_used
                    && let (Some(root), Some(target)) = (reveal_root.as_deref(), reveal.as_deref())
                    && explorer_core::path_eq(root, path)
                {
                    reveal_used = true;
                    return Some(target);
                }
                None
            };
            let mut place_drop: Option<PathBuf> = None;
            let mut place_revealed = false;
            let mut place = |ui: &mut Ui, icon: Icon, name: &str, path: &PathBuf, cur: &PathBuf| {
                let r = ui_common::nav_item(ui, Some(icon), name, explorer_core::path_eq(path, cur));
                if reveal.as_ref().is_some_and(|t| explorer_core::path_eq(path, t)) {
                    r.scroll_to_me(Some(Align::Center));
                    place_revealed = true;
                }
                if path.as_os_str() != explorer_core::NETWORK && path.as_os_str() != r"\\wsl.localhost" {
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
                let rev = take_reveal(path);
                tree_node(ui, &app.tree, &app.shell_labels, &app.cur, path, (icon, name), &mut tree_out, 0, rev, false);
            }
            // Quick access pins, after the standard folders (which are always there).
            for path in &app.persist.settings.pinned {
                let name =
                    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let rev = take_reveal(path);
                tree_node(
                    ui,
                    &app.tree,
                    &app.shell_labels,
                    &app.cur,
                    path,
                    (Icon::Pin, &name),
                    &mut tree_out,
                    0,
                    rev,
                    false,
                );
            }
            // OneDrive personal/commercial roots (Windows only).
            for (name, path) in &app.onedrive {
                let rev = take_reveal(path);
                tree_node(
                    ui,
                    &app.tree,
                    &app.shell_labels,
                    &app.cur,
                    path,
                    (Icon::Cloud, name),
                    &mut tree_out,
                    0,
                    rev,
                    false,
                );
            }
            ui.add_space(6.0);
            ui.separator();
            let this_pc = PathBuf::from(explorer_core::THIS_PC);
            if place(ui, Icon::ThisPc, "This PC", &this_pc, &app.cur) {
                go = Some(this_pc);
            }
            for d in &app.drives {
                if app.persist.settings.hide_empty_drives
                    && d.total == 0
                    && matches!(d.kind, DriveKind::Removable | DriveKind::Optical)
                {
                    continue;
                }
                let root = d.root.to_string_lossy();
                let letter = root.trim_end_matches('\\');
                let label = drive_label(d, app.persist.settings.drive_letters_before);
                let icon = match d.kind {
                    DriveKind::Network => Icon::NetworkDrive,
                    DriveKind::Optical => Icon::Disc,
                    DriveKind::Removable => Icon::Usb,
                    _ => Icon::Drive,
                };
                let rev = take_reveal(&d.root);
                let r = tree_node(
                    ui,
                    &app.tree,
                    &app.shell_labels,
                    &app.cur,
                    &d.root,
                    (icon, &label),
                    &mut tree_out,
                    0,
                    rev,
                    false,
                );
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
                let rev = take_reveal(&d.path);
                tree_node(
                    ui,
                    &app.tree,
                    &app.shell_labels,
                    &app.cur,
                    &d.path,
                    (Icon::Phone, &d.label),
                    &mut tree_out,
                    0,
                    rev,
                    false,
                );
                if d.total > 0 {
                    space_bar(ui, d.total, d.free);
                }
            }
            if app.persist.settings.show_libraries {
                ui.add_space(6.0);
                let libraries = PathBuf::from(explorer_core::LIBRARIES);
                if place(ui, Icon::Libraries, "Libraries", &libraries, &app.cur) {
                    go = Some(libraries);
                }
            }
            let network = PathBuf::from(explorer_core::NETWORK);
            if place(ui, Icon::Network, "Network", &network, &app.cur) {
                go = Some(network);
            }
            if app.has_wsl {
                let wsl = PathBuf::from(r"\\wsl.localhost");
                if place(ui, Icon::Network, "Linux", &wsl, &app.cur) {
                    go = Some(wsl);
                }
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
            if place_revealed || tree_out.reveal_done || (reveal.is_some() && !reveal_used) {
                app.reveal_tree = None;
            }
            if let Some((id, path)) = tree_out.hover_tree {
                app.frame_hover = Some((crate::dnd::HoverTarget::Tree(id), Some(path)));
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
                let what = if sel.len() == 1 {
                    "1 item selected".to_string()
                } else {
                    format!("{} items selected", sel.len())
                };
                if sel.iter().all(|e| !e.is_dir) {
                    ui.label(format!("{what}  {}", format_bytes(bytes)));
                } else {
                    ui.label(what);
                }
            }
            if let Some(q) = &app.deep_search {
                ui.separator();
                ui.label(format!("Results for \"{q}\" in subfolders"));
            }
            if !app.search_filters.is_empty() {
                ui.separator();
                ui.label(RichText::new(format!("Filter: {}", app.search_filters.summary())).weak());
            }
            if app.loading {
                ui.separator();
                ui.spinner();
                ui.label(if app.deep_search.is_some() { "Searching…" } else { "Loading…" });
            } else if let Some(t) = app.list_time.filter(|_| std::env::var_os("OXI_EXPLORER_TIMING").is_some()) {
                // Listing time, for measuring only (Explorer shows nothing like it).
                ui.separator();
                let text = match app.search_backend {
                    Some(b) => format!("{b} in {:.1} ms", t.as_secs_f64() * 1000.0),
                    None => format!("read in {:.1} ms", t.as_secs_f64() * 1000.0),
                };
                ui.label(RichText::new(text).weak());
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
            // The two layout switches at the right end, as in Explorer.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                use ui_common::ViewIcon;
                ui.spacing_mut().item_spacing.x = 2.0;
                for (mode, icon, tip) in [
                    (ViewMode::Large, ViewIcon::Large, "Display items by using large thumbnails"),
                    (ViewMode::Details, ViewIcon::Details, "Display information about each item in the window"),
                ] {
                    if ui_common::view_toggle(ui, icon, "", app.view_mode == mode, tip).clicked() {
                        app.view_mode = mode;
                    }
                }
            });
        });
    });
}

/// The tip Explorer shows when the pointer rests on an item: type, size and date for a file,
/// the creation date for a folder. Empty when there is nothing to say.
pub fn info_tip(e: &explorer_core::Entry, utc_offset: i64) -> String {
    if e.is_dir {
        return if e.created > 0 {
            format!("Date created: {}", format_time(e.created, utc_offset))
        } else {
            String::new()
        };
    }
    let mut tip = format!("Type: {}", e.type_name());
    tip += &format!("\nSize: {}", format_bytes(e.size));
    if e.modified > 0 {
        tip += &format!("\nDate modified: {}", format_time(e.modified, utc_offset));
    }
    tip
}

/// Adds [`info_tip`] to an item's response; it shows once the pointer stays still on it.
pub fn with_info_tip(resp: egui::Response, e: &explorer_core::Entry, utc_offset: i64) -> egui::Response {
    if e.is_dir && e.created <= 0 {
        return resp;
    }
    resp.on_hover_ui(|ui| {
        ui.label(info_tip(e, utc_offset));
    })
}

/// What a row click or menu asked for; applied after the table so `app` is not borrowed twice.
pub enum Action {
    Click(usize, egui::Modifiers),
    Open(usize),
    OpenPath(PathBuf),
    OpenInNewTabPath(PathBuf),
    Favorite(PathBuf, bool),
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
    Refresh,
    Terminal,
    ClearSelection,
    Verb(&'static str),
    NewWindow(PathBuf),
    Pin(PathBuf, bool),
    TerminalAt(PathBuf),
    Share,
}

pub fn file_list(app: &mut App, ui: &mut Ui) {
    // The sidebar or toolbar may have navigated earlier in this frame.
    app.update_view();
    let over_list = ui.rect_contains_pointer(ui.max_rect());
    let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
    if over_list && app.dialog.is_none() && ctrl_held {
        let mut steps = 0i32;
        ui.input_mut(|i| {
            // Do not scroll the list while Ctrl+wheel changes the view.
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            for ev in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = ev
                    && (modifiers.ctrl || modifiers.command)
                {
                    match unit {
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            if delta.y > 0.0 {
                                steps += 1;
                            } else if delta.y < 0.0 {
                                steps -= 1;
                            }
                        }
                        egui::MouseWheelUnit::Point => {
                            app.wheel_zoom_accum += delta.y;
                            while app.wheel_zoom_accum >= 20.0 {
                                app.wheel_zoom_accum -= 20.0;
                                steps += 1;
                            }
                            while app.wheel_zoom_accum <= -20.0 {
                                app.wheel_zoom_accum += 20.0;
                                steps -= 1;
                            }
                        }
                    }
                }
            }
        });
        if steps != 0 {
            let up = steps > 0;
            for _ in 0..steps.abs() {
                app.view_mode = app.view_mode.step_wheel(up);
            }
        }
    } else {
        app.wheel_zoom_accum = 0.0;
    }
    let mut action: Option<Action> = None;

    // Background (empty space below the rows): clear selection, folder context menu.
    let bg = ui.interact(ui.max_rect(), ui.id().with("bg"), Sense::click());
    if bg.clicked() {
        action = Some(Action::ClearSelection);
    }
    let place = explorer_core::classify(&app.cur);
    let shell_menus = app.hwnd != 0
        && (!place.is_page() || matches!(place, explorer_core::Place::Server(_) | explorer_core::Place::Shell(_)));
    let in_bin = app.in_recycle_bin();
    let shift = ui.input(|i| i.modifiers.shift);
    if shell_menus && shift && bg.secondary_clicked() {
        action = Some(Action::ShellMenu(None));
    }
    if !(shell_menus && shift) {
        bg.context_menu(|ui| crate::menus::background(app, ui, &mut action));
    }

    let on_home_page =
        app.cur.as_os_str() == explorer_core::HOME && app.filter.trim().is_empty() && app.deep_search.is_none();
    if app.view.is_empty() && !app.loading && !on_home_page {
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
    let mut header = |h: &mut egui_extras::TableRow,
                      key: Option<SortKey>,
                      filter_kind: Option<FilterColumn>,
                      title: &str,
                      app: &App,
                      out: &mut Option<SortKey>,
                      popup_out: &mut Option<(FilterColumn, egui::Pos2)>,
                      choose_cols_out: &mut bool| {
        h.col(|ui| {
            // The whole cell sorts, with the chevron above the title, as in Explorer; a filter
            // chevron appears at the right edge while the pointer is over a filterable header.
            let sorted = key.filter(|&k| app.sort == k).map(|_| !app.ascending);
            let resp = ui_common::sort_header(ui, title, sorted);
            let rect = resp.rect;
            let p = ui_common::Palette::of(ui);
            let btn_rect = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 20.0, rect.top() + 3.0),
                egui::pos2(rect.right() - 2.0, rect.bottom() - 3.0),
            );
            let is_open = filter_kind.is_some() && app.active_filter_popup.as_ref().map(|(k, _)| *k) == filter_kind;
            if let Some(kind) = filter_kind {
                let painter = ui.painter_at(rect);
                if app.header_filters.is_filtered(kind) {
                    let x = if resp.hovered() { btn_rect.left() - 14.0 } else { rect.right() - 16.0 };
                    let font = egui::FontId::proportional(12.0);
                    painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, "☑", font, p.text);
                }
                if resp.hovered() || is_open {
                    let on_btn = resp.hover_pos().is_some_and(|pos| btn_rect.contains(pos));
                    if on_btn {
                        painter.rect_filled(btn_rect, 2.0, p.hover);
                    }
                    let (cx, cy) = btn_rect.center().into();
                    let points =
                        vec![egui::pos2(cx - 3.5, cy - 1.5), egui::pos2(cx, cy + 2.0), egui::pos2(cx + 3.5, cy - 1.5)];
                    painter.line(points, Stroke::new(1.2, p.text));
                }
            }
            if resp.clicked() {
                let on_btn = resp.interact_pointer_pos().is_some_and(|pos| btn_rect.contains(pos));
                if let Some(kind) = filter_kind.filter(|_| on_btn) {
                    *popup_out = Some((kind, rect.left_bottom()));
                } else if key.is_some() {
                    *out = key;
                }
            }
            resp.context_menu(|ui| {
                ui.add_enabled(false, egui::Checkbox::new(&mut true, "Name"));
                for (k, label, on) in
                    [(SortKey::Created, "Date created", col_created), (SortKey::Attributes, "Attributes", col_attrs)]
                {
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
                ui.separator();
                if ui.button("More…").clicked() {
                    *choose_cols_out = true;
                    ui.close();
                }
            });
        });
    };
    let n = app.view.len();
    let weak = ui.visuals().weak_text_color();
    let text_color = ui.visuals().text_color();
    let mut wanted_icons: Vec<IconKey> = Vec::new();
    // Drag and drop, collected while drawing (the rows only borrow `app`).
    let dragging = app.drag.is_some();
    let mut drag_start: Option<String> = None;
    let mut hover_target: Option<PathBuf> = None;
    // A name being edited in place: drawn by its row or tile, committed after the list.
    let mut renaming = app.renaming.take();
    if on_home_page {
        home_page_ui(app, ui, &mut action, &mut wanted_icons);
    } else if classify(&app.cur) == explorer_core::Place::Gallery {
        crate::gallery::show(app, ui, &mut action, shell_menus);
    } else if app.view_mode != ViewMode::Details {
        crate::grid::show(app, ui, &mut action, &mut wanted_icons, shell_menus, in_bin, &mut renaming);
    } else {
        let mut popup_toggle: Option<(FilterColumn, egui::Pos2)> = None;
        let mut open_choose_cols = false;
        let band_active = ui.data(|d| d.get_temp::<[f32; 2]>(egui::Id::new(DETAILS_BAND)).is_some());
        let mut table = TableBuilder::new(ui)
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
            .header(ui_common::SORT_HEADER_HEIGHT, |mut h| {
                header(
                    &mut h,
                    Some(SortKey::Name),
                    Some(FilterColumn::Name),
                    "Name",
                    app,
                    &mut sort_click,
                    &mut popup_toggle,
                    &mut open_choose_cols,
                );
                let bin = app.in_recycle_bin();
                header(
                    &mut h,
                    Some(SortKey::Modified),
                    if bin { None } else { Some(FilterColumn::Modified) },
                    if bin { "Date deleted" } else { "Date modified" },
                    app,
                    &mut sort_click,
                    &mut popup_toggle,
                    &mut open_choose_cols,
                );
                header(
                    &mut h,
                    Some(SortKey::Type),
                    if bin { None } else { Some(FilterColumn::Type) },
                    if bin { "Original location" } else { "Type" },
                    app,
                    &mut sort_click,
                    &mut popup_toggle,
                    &mut open_choose_cols,
                );
                header(
                    &mut h,
                    Some(SortKey::Size),
                    Some(FilterColumn::Size),
                    "Size",
                    app,
                    &mut sort_click,
                    &mut popup_toggle,
                    &mut open_choose_cols,
                );
                if col_created {
                    header(
                        &mut h,
                        Some(SortKey::Created),
                        None,
                        "Date created",
                        app,
                        &mut sort_click,
                        &mut popup_toggle,
                        &mut open_choose_cols,
                    );
                }
                if col_attrs {
                    header(
                        &mut h,
                        Some(SortKey::Attributes),
                        None,
                        "Attributes",
                        app,
                        &mut sort_click,
                        &mut popup_toggle,
                        &mut open_choose_cols,
                    );
                }
                for p in &prop_cols {
                    header(
                        &mut h,
                        Some(SortKey::Prop(*p)),
                        None,
                        p.label(),
                        app,
                        &mut sort_click,
                        &mut popup_toggle,
                        &mut open_choose_cols,
                    );
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
                    let cut = app.is_cut(&e.name);
                    let row_weak = if cut { weak.gamma_multiply(0.5) } else { weak };
                    let img_tint = if cut { egui::Color32::WHITE.gamma_multiply(0.5) } else { egui::Color32::WHITE };
                    let shown = if app.show_extensions || e.is_dir {
                        e.display()
                    } else {
                        let d = e.display();
                        d.rfind('.').filter(|&i| i > 0).map_or(d, |i| &d[..i])
                    };
                    let mut name_color = if dim { weak } else { text_color };
                    if cut {
                        name_color = name_color.gamma_multiply(0.5);
                    }
                    let name_text = RichText::new(shown).color(name_color);
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
                                ui.add(egui::Image::new((tex.id(), egui::vec2(16.0, 16.0))).tint(img_tint));
                            }
                            state => {
                                if state.is_none() {
                                    wanted_icons.push(key);
                                }
                                let icon = RichText::new(icon_for(e));
                                let mut c = if e.is_dir {
                                    ui_common::Palette::of(ui).folder
                                } else {
                                    ui.visuals().text_color()
                                };
                                if cut {
                                    c = c.gamma_multiply(0.5);
                                }
                                ui.label(icon.color(c));
                            }
                        }
                        ui.add_space(4.0);
                        match renaming.as_mut().filter(|r| r.original == e.name) {
                            Some(r) => rename_field(ui, r, None),
                            None => {
                                ui.add(egui::Label::new(name_text).selectable(false).truncate());
                            }
                        }
                        if e.link {
                            ui.label(RichText::new(" ⤴").color(row_weak));
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
                            egui::Label::new(RichText::new(format_time(e.modified, app.utc_offset)).color(row_weak))
                                .selectable(false),
                        );
                    });
                    row.col(|ui| {
                        let third = match &e.note {
                            Some(n) => n.clone(),
                            None => e.type_name(),
                        };
                        ui.add(egui::Label::new(RichText::new(third).color(row_weak)).selectable(false));
                    });
                    row.col(|ui| {
                        if !e.is_dir {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(format_size_kb(e.size)).color(row_weak))
                                        .selectable(false),
                                );
                            });
                        }
                    });
                    if col_created {
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format_time(e.created, app.utc_offset)).color(row_weak))
                                    .selectable(false),
                            );
                        });
                    }
                    if col_attrs {
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format_attributes(e.attrs)).color(row_weak))
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
                                ui.add(egui::Label::new(RichText::new(text).color(row_weak)).selectable(false));
                            });
                        }
                    }
                    let resp = row.response();
                    let resp = if dragging || band_active { resp } else { with_info_tip(resp, e, app.utc_offset) };
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
                    let is_renaming = renaming.as_ref().is_some_and(|r| r.original == e.name);
                    if resp.middle_clicked() && e.is_dir {
                        action = Some(Action::OpenInNewTab(pos));
                    } else if app.persist.settings.single_click {
                        if resp.clicked() {
                            action = Some(Action::Open(pos));
                        }
                    } else if resp.double_clicked() {
                        action = Some(Action::Open(pos));
                    } else if resp.clicked() {
                        action = Some(Action::Click(pos, resp.ctx.input(|i| i.modifiers)));
                    } else if !is_renaming && resp.secondary_clicked() {
                        let shift = resp.ctx.input(|i| i.modifiers.shift);
                        action = Some(if shell_menus && shift {
                            Action::ShellMenu(Some(pos))
                        } else {
                            Action::ContextSelect(pos)
                        });
                    }
                    let shift = resp.ctx.input(|i| i.modifiers.shift);
                    if is_renaming || (shell_menus && shift) {
                        return;
                    }
                    if in_bin {
                        resp.context_menu(|ui| crate::menus::recycle_bin_item(ui, &mut action));
                        return;
                    }
                    // Built only while the menu is open, not for every row every frame.
                    resp.context_menu(|ui| {
                        let info = crate::menus::ItemInfo::new(app, e, pos, shell_menus);
                        crate::menus::item(ui, &info, &mut action);
                    });
                });
            });
        details_band(app, ui, band_start, &drawn, &rows_map);
        if let Some((kind, pos)) = popup_toggle {
            if app.active_filter_popup.as_ref().map(|(k, _)| *k) == Some(kind) {
                app.active_filter_popup = None;
            } else {
                app.active_filter_popup = Some((kind, pos));
            }
        }
        if open_choose_cols {
            app.choose_columns_open = true;
            app.choose_columns_search = String::new();
            app.choose_columns_selected = app.prop_cols.clone();
        }

        if let Some((popup, pos)) = app.active_filter_popup {
            let just_opened = popup_toggle.is_some();
            let mut close = false;
            let area = egui::Area::new(egui::Id::new("header-filter-popup-area"))
                .fixed_pos(pos)
                .order(egui::Order::Foreground);
            let resp = area.show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(170.0);
                    let f = &mut app.header_filters;
                    match popup {
                        FilterColumn::Name => {
                            let items: Vec<_> = NameBucket::ALL.iter().map(|&b| (b, b.label())).collect();
                            bucket_checks(ui, &items, &mut f.names);
                        }
                        FilterColumn::Modified => {
                            let items: Vec<_> = DateModifiedBucket::ALL.iter().map(|&b| (b, b.label())).collect();
                            bucket_checks(ui, &items, &mut f.dates);
                        }
                        FilterColumn::Type => {
                            let (top_types, has_others) = explorer_core::folder_type_buckets(&app.entries);
                            let mut items: Vec<_> = top_types.into_iter().map(|t| (t.clone(), t)).collect();
                            if has_others {
                                items.push(("Others".to_string(), "Others".to_string()));
                            }
                            let items: Vec<_> = items.iter().map(|(k, l)| (k.clone(), l.as_str())).collect();
                            bucket_checks(ui, &items, &mut f.types);
                        }
                        FilterColumn::Size => {
                            let items: Vec<_> = HeaderSizeBucket::ALL.iter().map(|&b| (b, b.label())).collect();
                            bucket_checks(ui, &items, &mut f.sizes);
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Clear").clicked() {
                            let f = &mut app.header_filters;
                            match popup {
                                FilterColumn::Name => f.names.clear(),
                                FilterColumn::Modified => f.dates.clear(),
                                FilterColumn::Type => f.types.clear(),
                                FilterColumn::Size => f.sizes.clear(),
                            }
                            close = true;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if close {
                app.active_filter_popup = None;
            } else if !just_opened
                && ui.input(|i| i.pointer.any_click())
                && let Some(click_pos) = ui.input(|i| i.pointer.interact_pos())
                && !resp.response.rect.contains(click_pos)
            {
                app.active_filter_popup = None;
            }
        }
    }

    if let Some(mut r) = renaming {
        match r.outcome.take() {
            Some(true) => {
                if let Some(e) = app.finish_rename(&r) {
                    // Stay in the field with the message, as Explorer does.
                    r.error = Some(e);
                    r.opened = false;
                    app.renaming = Some(r);
                }
            }
            Some(false) => {}
            None => app.renaming = Some(r),
        }
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
                if app.persist.settings.browse_new_window && e.is_dir {
                    let p = app.child(&e.name);
                    app.open_window(&p);
                } else {
                    app.open_entry(&e);
                }
            }
        }
        Some(Action::OpenPath(p)) => {
            if p.is_dir() {
                if app.persist.settings.browse_new_window {
                    app.open_window(&p);
                } else {
                    app.navigate(p);
                }
            } else if let Err(e) = explorer_sys::open(&p) {
                app.status = Some(format!("Could not open {}: {e}", p.display()));
            }
        }
        Some(Action::OpenInNewTabPath(p)) => {
            app.new_tab(p, false);
        }
        Some(Action::Favorite(p, add)) => {
            if add {
                app.add_favorite(p);
            } else {
                app.remove_favorite(&p);
            }
            home_data_stale(ui.ctx());
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
        Some(Action::CopyPath) => app.copy_as_path(ui.ctx()),
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
        Some(Action::Verb(verb)) => {
            if let Err(e) = explorer_sys::shell_verb(app.hwnd, &app.cur, &app.selected_paths(), verb) {
                app.status = Some(format!("{verb}: {e}"));
            }
        }
        Some(Action::NewWindow(path)) => {
            app.open_window(&path);
        }
        Some(Action::Pin(path, pin)) => {
            if pin {
                app.pin(path);
            } else {
                app.unpin(&path);
            }
        }
        Some(Action::TerminalAt(path)) => {
            if let Err(e) = explorer_sys::open_terminal(&path) {
                app.status = Some(format!("Terminal: {e}"));
            }
        }
        Some(Action::Share) => {
            app.nearby.queue = app.selected_paths();
            app.nearby.open = true;
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

/// A column-filter checkbox list: one box per bucket, toggling its key in `set`.
fn bucket_checks<T: Clone + Eq + std::hash::Hash>(
    ui: &mut Ui,
    items: &[(T, &str)],
    set: &mut std::collections::HashSet<T>,
) {
    for (key, label) in items {
        let mut on = set.contains(key);
        if ui.checkbox(&mut on, *label).changed() {
            if on {
                set.insert(key.clone());
            } else {
                set.remove(key);
            }
        }
    }
}

pub fn dialog(app: &mut App, ui: &mut Ui) {
    if app.choose_columns_open {
        let mut save = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("choose-columns")).show(ui.ctx(), |ui| {
            ui.set_width(320.0);
            ui.heading("Choose Details");
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(&mut app.choose_columns_search).hint_text("Search properties…"));
            ui.separator();
            let q = app.choose_columns_search.trim().to_lowercase();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for p in explorer_core::Prop::ALL {
                    if q.is_empty() || p.label().to_lowercase().contains(&q) {
                        let mut on = app.choose_columns_selected.contains(&p);
                        if ui.checkbox(&mut on, p.label()).changed() {
                            if on {
                                app.choose_columns_selected.push(p);
                            } else {
                                app.choose_columns_selected.retain(|&x| x != p);
                            }
                        }
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                save = ui.button("OK").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if save {
            app.prop_cols = app.choose_columns_selected.clone();
            app.choose_columns_open = false;
        } else if cancel || modal.should_close() {
            app.choose_columns_open = false;
        }
    }

    let Some(mut d) = app.dialog.take() else { return };
    match &mut d {
        Dialog::MapDrive(_) => {
            let Dialog::MapDrive(m) = d else { unreachable!() };
            map_drive_dialog(app, ui, m)
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
        }
        Dialog::AddGalleryFolder(path_str) => {
            let (mut ok, mut cancel) = (false, false);
            let modal = egui::Modal::new(egui::Id::new("add-gallery-folder")).show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                ui.heading("Add collection folder");
                ui.add_space(6.0);
                ui.label("Folder path:");
                ui.add(egui::TextEdit::singleline(path_str).desired_width(340.0));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ok = ui.button("Add").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if ok {
                let p = PathBuf::from(path_str.trim());
                if p.is_dir() {
                    if !app.persist.settings.gallery_folders.contains(&p) {
                        app.persist.settings.gallery_folders.push(p);
                        app.refresh();
                    }
                } else {
                    app.status = Some(format!("Folder does not exist: {}", p.display()));
                }
            } else if !cancel && !modal.should_close() {
                app.dialog = Some(d);
            }
        }
    }
}

/// The in-place name editor over an item's label: focused with the name's stem selected when it
/// opens; Enter or clicking elsewhere commits, Esc cancels. An error shows under the field.
pub fn rename_field(ui: &mut Ui, r: &mut crate::InlineRename, rect: Option<egui::Rect>) {
    // As wide as the name (Explorer's box hugs the text), within the column.
    let text_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(r.text.clone(), egui::FontId::proportional(ui_common::theme::BODY), egui::Color32::WHITE)
            .size()
            .x
    });
    // egui drops focus on Esc before widgets see the key, so read it from the raw events.
    let esc = ui.input(|i| {
        i.raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, .. }))
    });
    let edit = egui::TextEdit::singleline(&mut r.text).margin(egui::vec2(3.0, 1.0));
    let resp = match rect {
        Some(rect) => ui.put(rect, edit.desired_width(rect.width())),
        None => ui.add(edit.desired_width((text_w + 24.0).clamp(60.0, ui.available_width().max(60.0)))),
    };
    if !r.opened {
        r.opened = true;
        resp.request_focus();
        // Select the name without its extension, as Explorer does (folders: all of it).
        let n = r.text.chars().count();
        let stem = match r.text.rfind('.') {
            Some(i) if i > 0 && !r.dir.join(&r.original).is_dir() => r.text[..i].chars().count(),
            _ => n,
        };
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
            let range = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem));
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), resp.id);
        }
    } else if resp.lost_focus() {
        r.outcome = Some(!esc);
    }
    if resp.changed() {
        r.error = None;
    }
    if let Some(e) = r.error {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), resp.id.with("err"), &resp)
            .show(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
    }
}

type HomeCache = (std::time::Instant, Vec<(PathBuf, String, bool)>, Vec<explorer_core::Entry>);

/// Quick access tiles and Favorites, re-read at most once a second: they stat every path, and a pinned
/// network folder that is offline would stall each frame.
fn home_data(app: &App, ui: &Ui) -> (Vec<(PathBuf, String, bool)>, Vec<explorer_core::Entry>) {
    let id = egui::Id::new("oxi_home_cache");
    if let Some((at, tiles, favs)) = ui.data(|d| d.get_temp::<HomeCache>(id))
        && at.elapsed() < std::time::Duration::from_secs(1)
    {
        return (tiles, favs);
    }
    let (tiles, favs) = (app.quick_access_tiles(), app.favorite_items());
    ui.data_mut(|d| d.insert_temp::<HomeCache>(id, (std::time::Instant::now(), tiles.clone(), favs.clone())));
    (tiles, favs)
}

/// Drops the cached Home data after a pin, unpin or favorite change.
fn home_data_stale(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<HomeCache>(egui::Id::new("oxi_home_cache")));
}

/// A label inside `rect`, left-aligned and vertically centred (`Ui::put` centres it).
fn put_left(ui: &mut Ui, rect: egui::Rect, label: egui::Label) {
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(layout), |ui| {
        ui.add(label);
    });
}

fn section_header(ui: &mut Ui, title: &str, count: usize, collapsed: bool) -> egui::Response {
    let chevron = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
    let resp = ui
        .horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(chevron.sized(13.0));
            ui.add_space(4.0);
            ui.label(RichText::new(title).strong().size(14.0));
            ui.label(RichText::new(format!(" ({count})")).weak().size(13.0));
        })
        .response;
    ui.interact(resp.rect, resp.id.with(title), Sense::click())
}

fn home_page_ui(app: &mut App, ui: &mut Ui, action: &mut Option<Action>, wanted_icons: &mut Vec<IconKey>) {
    let weak_color = ui.visuals().weak_text_color();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(8.0);

        // 1. Quick access
        let qa_collapsed = app.persist.settings.home_collapsed.contains("quick_access");
        let (tiles, favorites) = home_data(app, ui);
        let qa_resp = section_header(ui, "Quick access", tiles.len(), qa_collapsed);
        if qa_resp.clicked() {
            if qa_collapsed {
                app.persist.settings.home_collapsed.remove("quick_access");
            } else {
                app.persist.settings.home_collapsed.insert("quick_access".into());
            }
        }
        ui.add_space(6.0);

        if !qa_collapsed {
            let avail_w = ui.available_width() - 24.0;
            let tile_w = 230.0f32;
            let cols = ((avail_w + 8.0) / (tile_w + 8.0)).floor().max(1.0) as usize;

            for chunk in tiles.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    for (path, label, pinned) in chunk {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(tile_w, 56.0), Sense::click());
                        let hover = resp.hovered();
                        let bg = if hover {
                            ui.visuals().widgets.hovered.bg_fill
                        } else {
                            ui.visuals().widgets.inactive.bg_fill.gamma_multiply(0.3)
                        };
                        let stroke = if hover {
                            egui::Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color)
                        } else {
                            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
                        };
                        ui.painter().rect_filled(rect, 4.0, bg);
                        ui.painter().rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

                        // Icon on left
                        let icon_center = egui::pos2(rect.left() + 24.0, rect.center().y);
                        ui.painter().text(
                            icon_center,
                            egui::Align2::CENTER_CENTER,
                            Icon::FolderFilled.glyph(),
                            Icon::font(24.0),
                            ui_common::Palette::of(ui).folder,
                        );

                        // Labels on right
                        let text_left = rect.left() + 48.0;
                        let text_w = (rect.right() - text_left - 8.0).max(10.0);
                        let line1_rect = egui::Rect::from_min_size(
                            egui::pos2(text_left, rect.top() + 8.0),
                            egui::vec2(text_w, 18.0),
                        );
                        let line2_rect = egui::Rect::from_min_size(
                            egui::pos2(text_left, rect.top() + 28.0),
                            egui::vec2(text_w, 16.0),
                        );

                        let name_rich = if app.persist.settings.single_click && hover {
                            RichText::new(label).strong().underline()
                        } else {
                            RichText::new(label).strong()
                        };
                        put_left(ui, line1_rect, egui::Label::new(name_rich).truncate());
                        if *pinned {
                            let pin_pos = egui::pos2(rect.right() - 16.0, rect.top() + 14.0);
                            ui.painter().text(
                                pin_pos,
                                egui::Align2::CENTER_CENTER,
                                Icon::Pin.glyph(),
                                Icon::font(11.0),
                                weak_color,
                            );
                        }

                        let parent_str = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                        put_left(ui, line2_rect, egui::Label::new(RichText::new(parent_str).small().weak()).truncate());

                        if resp.middle_clicked() {
                            *action = Some(Action::OpenInNewTabPath(path.clone()));
                        } else if app.persist.settings.single_click {
                            if resp.clicked() {
                                *action = Some(Action::OpenPath(path.clone()));
                            }
                        } else if resp.double_clicked() {
                            *action = Some(Action::OpenPath(path.clone()));
                        }

                        resp.context_menu(|ui| {
                            if ui_common::MenuItem::new("Open").icon(Icon::OpenFile).ui(ui).clicked() {
                                *action = Some(Action::OpenPath(path.clone()));
                                ui.close();
                            }
                            if ui_common::MenuItem::new("Open in new tab").icon(Icon::Add).ui(ui).clicked() {
                                *action = Some(Action::OpenInNewTabPath(path.clone()));
                                ui.close();
                            }
                            if ui_common::MenuItem::new("Open in new window").icon(Icon::NewWindow).ui(ui).clicked() {
                                *action = Some(Action::NewWindow(path.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if *pinned {
                                if ui_common::MenuItem::new("Unpin from Quick access")
                                    .icon(Icon::Unpin)
                                    .ui(ui)
                                    .clicked()
                                {
                                    app.unpin(path);
                                    home_data_stale(ui.ctx());
                                    ui.close();
                                }
                            } else {
                                if ui_common::MenuItem::new("Pin to Quick access").icon(Icon::Pin).ui(ui).clicked() {
                                    app.pin(path.clone());
                                    home_data_stale(ui.ctx());
                                    ui.close();
                                }
                                if ui_common::MenuItem::new("Remove from Quick access")
                                    .icon(Icon::Delete)
                                    .ui(ui)
                                    .clicked()
                                {
                                    app.remove_frequent_folder(path);
                                    home_data_stale(ui.ctx());
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                ui.add_space(6.0);
            }
        }
        ui.add_space(16.0);

        // 2. Favorites
        let fav_collapsed = app.persist.settings.home_collapsed.contains("favorites");
        let fav_resp = section_header(ui, "Favorites", favorites.len(), fav_collapsed);
        if fav_resp.clicked() {
            if fav_collapsed {
                app.persist.settings.home_collapsed.remove("favorites");
            } else {
                app.persist.settings.home_collapsed.insert("favorites".into());
            }
        }
        ui.add_space(6.0);

        if !fav_collapsed {
            if favorites.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new("Files you favorite appear here. Right-click any file to add it.").weak());
                });
            } else {
                home_file_table(app, ui, &favorites, action, wanted_icons, true);
            }
        }
        ui.add_space(16.0);

        // 3. Recent
        let rec_collapsed = app.persist.settings.home_collapsed.contains("recent");
        let recent_items = if app.persist.settings.privacy_recent_files { app.entries.clone() } else { Vec::new() };
        let rec_resp = section_header(ui, "Recent", recent_items.len(), rec_collapsed);
        if rec_resp.clicked() {
            if rec_collapsed {
                app.persist.settings.home_collapsed.remove("recent");
            } else {
                app.persist.settings.home_collapsed.insert("recent".into());
            }
        }
        ui.add_space(6.0);

        if !rec_collapsed {
            if !app.persist.settings.privacy_recent_files {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new("Recent files are hidden (Folder Options ▸ General ▸ Privacy).").weak());
                });
            } else if recent_items.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new("No recent files.").weak());
                });
            } else {
                home_file_table(app, ui, &recent_items, action, wanted_icons, false);
            }
        }
        ui.add_space(16.0);
    });
}

fn home_file_table(
    app: &mut App,
    ui: &mut Ui,
    items: &[explorer_core::Entry],
    action: &mut Option<Action>,
    wanted_icons: &mut Vec<IconKey>,
    is_favorites_section: bool,
) {
    let row_h = 24.0;

    // Header
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let avail_w = ui.available_width() - 32.0;
        let col1_w = (avail_w * 0.45).max(150.0);
        let col2_w = 160.0f32;
        let col3_w = (avail_w - col1_w - col2_w).max(100.0);

        let (r1, _) = ui.allocate_exact_size(egui::vec2(col1_w, 20.0), Sense::hover());
        put_left(ui, r1, egui::Label::new(RichText::new("Name").strong().weak()));
        let (r2, _) = ui.allocate_exact_size(egui::vec2(col2_w, 20.0), Sense::hover());
        put_left(ui, r2, egui::Label::new(RichText::new("Date modified").strong().weak()));
        let (r3, _) = ui.allocate_exact_size(egui::vec2(col3_w, 20.0), Sense::hover());
        put_left(ui, r3, egui::Label::new(RichText::new("Location").strong().weak()));
    });
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);

    // Rows
    for e in items {
        let (row_rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 16.0, row_h), Sense::click());
        let hover = resp.hovered();
        if hover {
            ui.painter().rect_filled(row_rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
        }

        let avail_w = row_rect.width() - 32.0;
        let col1_w = (avail_w * 0.45).max(150.0);
        let col2_w = 160.0f32;
        let col3_w = (avail_w - col1_w - col2_w).max(100.0);

        // Col 1: Icon + Name
        let name_x = row_rect.left() + 16.0;
        let icon_rect =
            egui::Rect::from_center_size(egui::pos2(name_x + 8.0, row_rect.center().y), egui::vec2(16.0, 16.0));
        let path = PathBuf::from(&e.name);
        let key = IconKey::for_entry(e, &app.cur);
        match app.icons.get(&key) {
            Some(IconState::Ready(tex)) => {
                ui.painter().image(
                    tex.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            state => {
                if state.is_none() {
                    wanted_icons.push(key);
                }
                let icon_text = RichText::new(icon_for(e));
                put_left(ui, icon_rect, egui::Label::new(icon_text));
            }
        }

        let text_rect = egui::Rect::from_min_size(
            egui::pos2(name_x + 22.0, row_rect.top() + 3.0),
            egui::vec2(col1_w - 22.0, row_h - 6.0),
        );
        let name_rich = if app.persist.settings.single_click && hover {
            RichText::new(e.display()).underline()
        } else {
            RichText::new(e.display())
        };
        put_left(ui, text_rect, egui::Label::new(name_rich).truncate());

        // Col 2: Date modified
        let col2_x = name_x + col1_w;
        let col2_rect =
            egui::Rect::from_min_size(egui::pos2(col2_x, row_rect.top() + 3.0), egui::vec2(col2_w, row_h - 6.0));
        let time_str = format_time(e.modified, app.utc_offset);
        put_left(ui, col2_rect, egui::Label::new(RichText::new(time_str).weak()).truncate());

        // Col 3: Location
        let col3_x = col2_x + col2_w;
        let col3_rect =
            egui::Rect::from_min_size(egui::pos2(col3_x, row_rect.top() + 3.0), egui::vec2(col3_w, row_h - 6.0));
        let loc_str = e.note.as_deref().unwrap_or("");
        put_left(ui, col3_rect, egui::Label::new(RichText::new(loc_str).weak()).truncate());

        // Click actions
        if resp.middle_clicked() {
            *action = Some(Action::OpenInNewTabPath(path.clone()));
        } else if app.persist.settings.single_click {
            if resp.clicked() {
                *action = Some(Action::OpenPath(path.clone()));
            }
        } else if resp.double_clicked() {
            *action = Some(Action::OpenPath(path.clone()));
        }

        // Context menu
        resp.context_menu(|ui| {
            if ui_common::MenuItem::new("Open").icon(Icon::OpenFile).ui(ui).clicked() {
                *action = Some(Action::OpenPath(path.clone()));
                ui.close();
            }
            ui.separator();
            if is_favorites_section || app.is_favorite(&path) {
                if ui_common::MenuItem::new("Remove from Favorites").icon(Icon::FavoriteStar).ui(ui).clicked() {
                    app.remove_favorite(&path);
                    home_data_stale(ui.ctx());
                    ui.close();
                }
            } else {
                if ui_common::MenuItem::new("Add to Favorites").icon(Icon::FavoriteStar).ui(ui).clicked() {
                    app.add_favorite(path.clone());
                    home_data_stale(ui.ctx());
                    ui.close();
                }
            }
            if ui_common::menu_item(ui, Icon::Link, "Copy as path", Some("Ctrl+Shift+C")).clicked() {
                ui.ctx().copy_text(path.display().to_string());
                ui.close();
            }
            if ui_common::menu_item(ui, Icon::Properties, "Properties", Some("Alt+Enter")).clicked() {
                *action = Some(Action::Properties(path.clone()));
                ui.close();
            }
        });
    }
}
