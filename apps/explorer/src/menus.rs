//! Context menus for items and folder background matching Windows 11's layout: an icon row for
//! primary actions (Cut, Copy, Rename, Share, Delete), common verbs, and "Show more options" to
//! open the classic shell menu.

use std::path::PathBuf;

use egui::Ui;
use ui_common::{Icon, MenuItem};

use crate::App;
use crate::view::Action;

/// Information about a right-clicked item needed to render its context menu.
pub struct ItemInfo {
    pub pos: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub ext: String,
    pub pinned: bool,
    pub shell: bool,
}

impl ItemInfo {
    pub fn new(app: &App, e: &explorer_core::Entry, pos: usize, shell: bool) -> Self {
        let path = app.child(&e.name);
        let pinned = e.is_dir && app.is_pinned(&path);
        Self { pos, is_dir: e.is_dir, ext: e.extension(), pinned, shell, path }
    }
}

/// Context menu for a file or folder item.
pub fn item(ui: &mut Ui, info: &ItemInfo, action: &mut Option<Action>) {
    // (1) Icon row for primary commands: Cut, Copy, Rename, Share, Delete
    ui.horizontal(|ui| {
        if ui_common::command_button(ui, Icon::Cut, "", true, "Cut (Ctrl+X)").clicked() {
            *action = Some(Action::Copy(true));
            ui.close();
        }
        if ui_common::command_button(ui, Icon::Copy, "", true, "Copy (Ctrl+C)").clicked() {
            *action = Some(Action::Copy(false));
            ui.close();
        }
        if ui_common::command_button(ui, Icon::Rename, "", true, "Rename (F2)").clicked() {
            *action = Some(Action::Rename);
            ui.close();
        }
        if ui_common::command_button(ui, Icon::Share, "", true, "Share with a nearby device (LocalSend)").clicked() {
            *action = Some(Action::Share);
            ui.close();
        }
        if ui_common::command_button(ui, Icon::Delete, "", true, "Delete (Delete, Ctrl+D)").clicked() {
            *action = Some(Action::Delete(false));
            ui.close();
        }
    });
    ui.separator();

    // (2) Open
    if ui_common::menu_item(ui, Icon::OpenFile, "Open", Some("Enter")).clicked() {
        *action = Some(Action::Open(info.pos));
        ui.close();
    }

    // (3) Files, Windows only: Open with…
    if !info.is_dir && info.shell && ui_common::menu_item(ui, Icon::OpenWith, "Open with…", None).clicked() {
        *action = Some(Action::Verb("openas"));
        ui.close();
    }

    // (4) Folders: Open in new tab, Open in new window, Pin/Unpin
    if info.is_dir {
        if ui_common::menu_item(ui, Icon::Add, "Open in new tab", None).clicked() {
            *action = Some(Action::OpenInNewTab(info.pos));
            ui.close();
        }
        if ui_common::menu_item(ui, Icon::NewWindow, "Open in new window", None).clicked() {
            *action = Some(Action::NewWindow(info.path.clone()));
            ui.close();
        }
        let (icon, label, pin) = if info.pinned {
            (Icon::Unpin, "Unpin from Quick access", false)
        } else {
            (Icon::Pin, "Pin to Quick access", true)
        };
        if ui_common::menu_item(ui, icon, label, None).clicked() {
            *action = Some(Action::Pin(info.path.clone(), pin));
            ui.close();
        }
    }

    // (5) Windows only by extension
    if info.shell {
        if matches!(info.ext.as_str(), "exe" | "msi" | "bat" | "cmd") {
            if ui_common::menu_item(ui, Icon::Shield, "Run as administrator", None).clicked() {
                *action = Some(Action::Verb("runas"));
                ui.close();
            }
        } else if matches!(info.ext.as_str(), "jpg" | "jpeg" | "png" | "bmp" | "gif" | "tif" | "tiff" | "webp" | "heic")
            && ui_common::menu_item(ui, Icon::Pictures, "Set as desktop background", None).clicked()
        {
            *action = Some(Action::Verb("setdesktopwallpaper"));
            ui.close();
        }
    }

    // (6) Copy as path
    if ui_common::menu_item(ui, Icon::Link, "Copy as path", Some("Ctrl+Shift+C")).clicked() {
        *action = Some(Action::CopyPath);
        ui.close();
    }

    // (7) Properties
    if ui_common::menu_item(ui, Icon::Properties, "Properties", Some("Alt+Enter")).clicked() {
        *action = Some(Action::Properties(info.path.clone()));
        ui.close();
    }

    // (8) Folders: separator, Open in Terminal
    if info.is_dir {
        ui.separator();
        if ui_common::menu_item(ui, Icon::Terminal, "Open in Terminal", None).clicked() {
            *action = Some(Action::TerminalAt(info.path.clone()));
            ui.close();
        }
    }

    // (9) Windows only: separator, Show more options
    if info.shell {
        ui.separator();
        if ui_common::menu_item(ui, Icon::More, "Show more options", Some("Shift+F10")).clicked() {
            *action = Some(Action::ShellMenu(Some(info.pos)));
            ui.close();
        }
    }
}

/// Context menu for items in the Recycle Bin.
pub fn recycle_bin_item(ui: &mut Ui, action: &mut Option<Action>) {
    if ui_common::menu_item(ui, Icon::Restore, "Restore", None).clicked() {
        *action = Some(Action::Restore);
        ui.close();
    }
    if ui_common::menu_item(ui, Icon::Delete, "Delete permanently", Some("Del")).clicked() {
        *action = Some(Action::Delete(true));
        ui.close();
    }
}

/// Context menu for the folder background.
pub fn background(app: &mut App, ui: &mut Ui, action: &mut Option<Action>) {
    if app.in_recycle_bin() {
        MenuItem::new("View").icon(Icon::View).submenu(ui, |ui| crate::view::view_menu(app, ui));
        MenuItem::new("Sort by").icon(Icon::Sort).submenu(ui, |ui| crate::view::sort_keys_menu(app, ui));
        MenuItem::new("Group by").icon(Icon::GroupBy).submenu(ui, |ui| crate::view::group_menu(app, ui));
        if ui_common::menu_item(ui, Icon::Refresh, "Refresh", Some("F5")).clicked() {
            *action = Some(Action::Refresh);
            ui.close();
        }
        ui.separator();
        if ui_common::menu_item(ui, Icon::RecycleBin, "Empty Recycle Bin", None).clicked() {
            *action = Some(Action::EmptyBin);
            ui.close();
        }
        return;
    }

    if app.can_paste() {
        ui.horizontal(|ui| {
            if ui_common::command_button(ui, Icon::Paste, "", true, "Paste (Ctrl+V)").clicked() {
                *action = Some(Action::Paste(app.cur.clone()));
                ui.close();
            }
        });
        ui.separator();
    }

    MenuItem::new("View").icon(Icon::View).submenu(ui, |ui| crate::view::view_menu(app, ui));
    MenuItem::new("Sort by").icon(Icon::Sort).submenu(ui, |ui| crate::view::sort_keys_menu(app, ui));
    MenuItem::new("Group by").icon(Icon::GroupBy).submenu(ui, |ui| crate::view::group_menu(app, ui));
    ui.separator();

    if ui_common::menu_item(ui, Icon::Refresh, "Refresh", Some("F5")).clicked() {
        *action = Some(Action::Refresh);
        ui.close();
    }

    if let Some(u) = app.undo_stack.last() {
        let label = format!("Undo {}", u.label());
        if ui_common::menu_item(ui, Icon::Undo, &label, Some("Ctrl+Z")).clicked() {
            app.undo();
            ui.close();
        }
    }
    ui.separator();

    if !app.in_virtual_place() {
        MenuItem::new("New").icon(Icon::Add).submenu(ui, |ui| crate::view::new_menu(app, ui));
        ui.separator();
    }

    if ui_common::menu_item(ui, Icon::Properties, "Properties", None).clicked() {
        *action = Some(Action::Properties(app.cur.clone()));
        ui.close();
    }
    ui.separator();

    if ui_common::menu_item(ui, Icon::Terminal, "Open in Terminal", None).clicked() {
        *action = Some(Action::Terminal);
        ui.close();
    }

    let place = explorer_core::classify(&app.cur);
    let shell = app.hwnd != 0
        && (!place.is_page() || matches!(place, explorer_core::Place::Server(_) | explorer_core::Place::Shell(_)));
    if shell {
        ui.separator();
        if ui_common::menu_item(ui, Icon::More, "Show more options", Some("Shift+F10")).clicked() {
            *action = Some(Action::ShellMenu(None));
            ui.close();
        }
    }
}
