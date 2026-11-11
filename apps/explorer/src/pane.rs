//! The right-hand pane: Preview (Alt+P) hosts the file type's Windows preview handler (Office,
//! PDF, mail, fonts, …) when one is registered, and otherwise shows a thumbnail or the start of
//! a text file; Details (Alt+Shift+P) shows the selection's properties.

use std::io::Read;
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui, vec2};
use explorer_core::query::Kind;
use explorer_core::{Entry, Prop, decode_text, format_attributes, format_bytes, format_time, icon_for, is_text_ext};

use crate::{App, IconKey, IconState, Msg};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropEditState {
    pub path: PathBuf,
    pub prop: Prop,
    pub text: String,
}

/// Bytes of a text file the preview reads.
const TEXT_BYTES: usize = 64 * 1024;
/// Thumbnail size for the preview, in pixels.
const PREVIEW_PX: u32 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pane {
    #[default]
    None,
    Preview,
    Details,
}

pub enum PreviewData {
    Image(explorer_sys::IconImage),
    Text(String),
    Nothing,
}

pub enum PreviewState {
    Loading,
    Image(egui::TextureHandle),
    Text(String),
    Nothing,
    /// Marked as downloaded from the internet: Explorer shows a warning instead of a preview.
    Blocked,
    /// A Windows preview handler; `None` until the pane's rectangle is known.
    Handler(Option<explorer_sys::PreviewHost>),
}

/// Reads the preview of `path` off the UI thread.
fn load(path: &Path, is_text: bool) -> PreviewData {
    if is_text {
        let mut buf = Vec::with_capacity(TEXT_BYTES);
        if let Ok(f) = std::fs::File::open(path)
            && f.take(TEXT_BYTES as u64).read_to_end(&mut buf).is_ok()
            && let Some(t) = decode_text(&buf)
        {
            return PreviewData::Text(t);
        }
    }
    match explorer_sys::thumbnail(path, PREVIEW_PX) {
        Some(img) => PreviewData::Image(img),
        None => PreviewData::Nothing,
    }
}

impl App {
    /// The one selected file, when exactly one is selected.
    fn single_selection(&self) -> Option<&Entry> {
        if self.selected.len() != 1 {
            return None;
        }
        let name = self.selected.iter().next()?;
        self.entries.iter().find(|e| &e.name == name)
    }

    /// Starts loading the preview of the selection if it changed.
    pub fn update_preview(&mut self) {
        if self.pane != Pane::Preview {
            return;
        }
        let target =
            self.single_selection().filter(|e| !e.is_dir).map(|e| (self.child(&e.name), is_text_ext(&e.extension())));
        match target {
            None => self.preview = None,
            Some((path, is_text)) => {
                if self.preview.as_ref().is_some_and(|(p, _)| *p == path) {
                    return;
                }
                // Drop the old handler first: it may hold the file open.
                self.preview = None;
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                let handler = !is_text && self.hwnd != 0 && explorer_sys::has_preview_handler(&ext);
                // As Explorer: no preview handler for files downloaded from the internet.
                if handler && explorer_core::preview_blocked(explorer_sys::zone_of(&path)) {
                    self.preview = Some((path, PreviewState::Blocked));
                    return;
                }
                if handler {
                    self.preview = Some((path, PreviewState::Handler(None)));
                    return;
                }
                self.load_preview(path, is_text);
            }
        }
    }

    /// Reads a thumbnail or text preview in the background.
    fn load_preview(&mut self, path: PathBuf, is_text: bool) {
        self.preview = Some((path.clone(), PreviewState::Loading));
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let _ = std::thread::Builder::new().name("preview".into()).spawn(move || {
            let data = load(&path, is_text);
            if tx.send(Msg::Preview(path, data)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    pub fn preview_loaded(&mut self, path: PathBuf, data: PreviewData) {
        let Some((cur, state)) = &mut self.preview else { return };
        if *cur != path {
            return;
        }
        *state = match data {
            PreviewData::Image(i) => PreviewState::Image(self.ctx.load_texture(
                "preview",
                egui::ColorImage::from_rgba_premultiplied([i.width, i.height], &i.rgba),
                egui::TextureOptions::LINEAR,
            )),
            PreviewData::Text(t) => PreviewState::Text(t),
            PreviewData::Nothing => PreviewState::Nothing,
        };
    }

    pub fn toggle_pane(&mut self, pane: Pane) {
        self.pane = if self.pane == pane { Pane::None } else { pane };
        if self.pane != Pane::Preview {
            self.preview = None;
        }
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    if app.pane == Pane::None {
        return;
    }
    app.update_preview();
    let panel_resp =
        egui::Panel::right("pane").resizable(true).default_size(app.pane_width).show(ui, |ui| match app.pane {
            Pane::Preview => preview(app, ui),
            Pane::Details => details(app, ui),
            Pane::None => {}
        });
    let w = panel_resp.response.rect.width();
    if w > 50.0 && (w - app.pane_width).abs() > 0.5 {
        app.pane_width = w;
    }
}

fn centered_note(ui: &mut Ui, text: &str) {
    ui.centered_and_justified(|ui| ui.label(RichText::new(text).weak()));
}

/// Opens the handler once the pane's rectangle is known and keeps its window over it; falls
/// back to the thumbnail when the handler fails.
fn handler_preview(app: &mut App, ui: &mut Ui) {
    let r = ui.available_rect_before_wrap();
    let ppp = ui.ctx().pixels_per_point();
    let px = [
        (r.left() * ppp).round() as i32,
        (r.top() * ppp).round() as i32,
        (r.right() * ppp).round() as i32,
        (r.bottom() * ppp).round() as i32,
    ];
    // A native child window sits above everything egui draws: hide it under menus and dialogs.
    let covered = egui::Popup::is_any_open(ui.ctx()) || ui.ctx().any_popup_open() || app.dialog.is_some();
    if let Some((path, PreviewState::Handler(host))) = &mut app.preview {
        if host.is_none() && !covered && px[2] > px[0] && px[3] > px[1] {
            match explorer_sys::PreviewHost::open(app.hwnd, path, px) {
                Ok(h) => *host = Some(h),
                Err(e) => {
                    eprintln!("preview handler for {}: {e}", path.display());
                    let path = path.clone();
                    return app.load_preview(path, false);
                }
            }
        }
        if let Some(h) = host {
            h.place((!covered).then_some(px));
        }
    }
    ui.allocate_rect(r, egui::Sense::hover());
}

fn preview(app: &mut App, ui: &mut Ui) {
    if matches!(app.preview, Some((_, PreviewState::Handler(_)))) {
        return handler_preview(app, ui);
    }
    match &app.preview {
        None => centered_note(ui, "Select a file to preview."),
        Some((_, PreviewState::Loading)) => {
            ui.centered_and_justified(|ui| ui.spinner());
        }
        Some((_, PreviewState::Nothing)) => centered_note(ui, "No preview available."),
        Some((_, PreviewState::Blocked)) => centered_note(ui, explorer_core::PREVIEW_BLOCKED_TEXT),
        Some((_, PreviewState::Handler(_))) => {}
        Some((_, PreviewState::Image(tex))) => {
            let size = tex.size_vec2();
            let scale = (ui.available_width() / size.x).min(1.5);
            ui.vertical_centered(|ui| ui.add(egui::Image::new((tex.id(), size * scale))));
        }
        Some((_, PreviewState::Text(t))) => {
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(t.as_str()).monospace()).wrap_mode(egui::TextWrapMode::Extend));
            });
        }
    }
}

fn details(app: &mut App, ui: &mut Ui) {
    if app.selected.is_empty() {
        let files = app.view.iter().filter(|&&i| !app.entries[i].is_dir).count();
        ui.vertical_centered(|ui| {
            ui.heading(
                app.cur.file_name().map_or_else(|| app.cur.display().to_string(), |n| n.to_string_lossy().into_owned()),
            );
            ui.label(RichText::new("File folder").weak());
        });
        ui.separator();
        egui::Grid::new("details-folder-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Items").weak());
            ui.label(app.view.len().to_string());
            ui.end_row();
            ui.label(RichText::new("Files").weak());
            ui.label(files.to_string());
            ui.end_row();
        });
        return;
    }

    if app.selected.len() > 1 {
        let many: Vec<(u64, i64)> =
            app.entries.iter().filter(|e| app.selected.contains(&e.name)).map(|e| (e.size, e.modified)).collect();
        let size: u64 = many.iter().map(|(s, _)| *s).sum();
        ui.vertical_centered(|ui| {
            ui.heading(format!("{} items selected", many.len()));
        });
        ui.separator();
        let newest = many.iter().map(|(_, m)| *m).max().unwrap_or(0);
        let oldest = many.iter().map(|(_, m)| *m).filter(|&t| t > 0).min().unwrap_or(0);
        egui::Grid::new("details-multi-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Total size").weak());
            ui.label(format_bytes(size));
            ui.end_row();
            ui.label(RichText::new("Date modified").weak());
            ui.label(format!("{} – {}", format_time(oldest, app.utc_offset), format_time(newest, app.utc_offset)));
            ui.end_row();
        });
        return;
    }

    let Some(selected_name) = app.selected.iter().next().cloned() else { return };
    let Some(e) = app.entries.iter().find(|entry| entry.name == selected_name).cloned() else { return };

    let path = app.child(&e.name);
    if !app.props.contains_key(&path) {
        app.props_wanted.push(path.clone());
        app.request_props();
    }

    // Large thumbnail or icon (up to pane width)
    let max_w = (ui.available_width() - 16.0).max(64.0);
    let thumb = app.thumbs.get(&path);
    match thumb {
        Some(IconState::Ready(tex)) => {
            let s = tex.size_vec2();
            let scale = (max_w / s.x).min(max_w / s.y).min(1.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new((tex.id(), s * scale)));
            });
        }
        _ => {
            app.thumb_wanted.push(path.clone());
            let key = IconKey::for_entry(&e, &app.cur);
            ui.vertical_centered(|ui| match app.icons.get(&key) {
                Some(IconState::Ready(tex)) => {
                    ui.add(egui::Image::new((tex.id(), vec2(64.0, 64.0))));
                }
                _ => {
                    ui.label(RichText::new(icon_for(&e)).size(54.0));
                }
            });
        }
    }

    ui.vertical_centered(|ui| {
        ui.add(egui::Label::new(RichText::new(e.display()).heading()).wrap());
        ui.label(RichText::new(e.type_name()).weak());
    });
    ui.separator();

    let is_readonly = (e.attrs & explorer_core::attr::READONLY) != 0;
    let can_edit = !is_readonly && explorer_sys::can_write_props(&path);
    let tooltip_reason =
        if is_readonly { "This file is read-only." } else { "Properties cannot be modified for this file type." };

    let prop_map: std::collections::HashMap<Prop, String> = Prop::ALL
        .into_iter()
        .map(|p| {
            let s = app
                .props
                .get(&path)
                .and_then(|o| o.as_ref())
                .and_then(|v| {
                    let idx = Prop::ALL.iter().position(|&x| x == p)?;
                    v.get(idx).cloned()
                })
                .unwrap_or_default();
            (p, s)
        })
        .collect();

    let kind = Kind::of(&e);
    let utc_offset = app.utc_offset;
    let group_th = explorer_core::group_thousands(e.size);

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("details-single-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            // General file properties
            if !e.is_dir {
                ui.label(RichText::new("Size").weak());
                let exact = format!("{} bytes", group_th);
                ui.label(if e.size < 1024 { exact } else { format!("{} ({exact})", format_bytes(e.size)) });
                ui.end_row();
            }
            ui.label(RichText::new("Date modified").weak());
            ui.label(format_time(e.modified, utc_offset));
            ui.end_row();
            ui.label(RichText::new("Date created").weak());
            ui.label(format_time(e.created, utc_offset));
            ui.end_row();
            ui.label(RichText::new("Attributes").weak());
            ui.label(format_attributes(e.attrs));
            ui.end_row();

            // Image specific
            if matches!(kind, Some(Kind::Picture)) {
                let dim = prop_map.get(&Prop::Dimensions).map(String::as_str).unwrap_or("");
                if !dim.is_empty() {
                    ui.label(RichText::new("Dimensions").weak());
                    ui.label(dim);
                    ui.end_row();
                }
                let dt = prop_map.get(&Prop::DateTaken).map(String::as_str).unwrap_or("");
                if !dt.is_empty() {
                    ui.label(RichText::new("Date taken").weak());
                    ui.label(dt);
                    ui.end_row();
                }
                let cam = prop_map.get(&Prop::CameraModel).map(String::as_str).unwrap_or("");
                if !cam.is_empty() {
                    ui.label(RichText::new("Camera model").weak());
                    ui.label(cam);
                    ui.end_row();
                }
            }

            // Audio / Video specific
            if matches!(kind, Some(Kind::Music | Kind::Video)) {
                let len = prop_map.get(&Prop::Length).map(String::as_str).unwrap_or("");
                if !len.is_empty() {
                    ui.label(RichText::new("Length").weak());
                    ui.label(len);
                    ui.end_row();
                }
                let alb = prop_map.get(&Prop::Album).map(String::as_str).unwrap_or("");
                if !alb.is_empty() {
                    ui.label(RichText::new("Album").weak());
                    ui.label(alb);
                    ui.end_row();
                }
                let yr = prop_map.get(&Prop::Year).map(String::as_str).unwrap_or("");
                if !yr.is_empty() {
                    ui.label(RichText::new("Year").weak());
                    ui.label(yr);
                    ui.end_row();
                }
                let br = prop_map.get(&Prop::BitRate).map(String::as_str).unwrap_or("");
                if !br.is_empty() {
                    ui.label(RichText::new("Bit rate").weak());
                    ui.label(br);
                    ui.end_row();
                }
            }

            // Document specific
            if matches!(kind, Some(Kind::Document)) {
                let pages = prop_map.get(&Prop::Pages).map(String::as_str).unwrap_or("");
                if !pages.is_empty() {
                    ui.label(RichText::new("Pages").weak());
                    ui.label(pages);
                    ui.end_row();
                }
            }
        });

        ui.separator();
        ui.label(RichText::new("Properties").strong());

        // Rating row (five stars)
        ui.horizontal(|ui| {
            ui.label(RichText::new("Rating").weak());
            let r_str = prop_map.get(&Prop::Rating).map(String::as_str).unwrap_or("");
            let r: u32 = r_str.trim().parse().unwrap_or(0);
            let cur_stars = explorer_core::rating_to_stars(r);
            for s in 1..=5 {
                let star_symbol = if s <= cur_stars { "★" } else { "☆" };
                if can_edit {
                    let resp = ui.small_button(star_symbol);
                    if resp.clicked() {
                        let new_stars = if cur_stars == s { 0 } else { s };
                        let new_r = explorer_core::stars_to_rating(new_stars);
                        match explorer_sys::write_prop(&path, Prop::Rating, &new_r.to_string()) {
                            Ok(()) => {
                                if let Some(Some(v)) = app.props.get_mut(&path)
                                    && let Some(idx) = Prop::ALL.iter().position(|&p| p == Prop::Rating)
                                {
                                    v[idx] = if new_r > 0 { new_r.to_string() } else { String::new() };
                                }
                            }
                            Err(e) => {
                                app.status = Some(format!("Could not save rating: {e}"));
                            }
                        }
                    }
                } else {
                    let r = ui.label(star_symbol);
                    r.on_hover_text(tooltip_reason);
                }
            }
        });

        // Editable text rows: Title, Authors, Tags, Comments
        let p_title = prop_map.get(&Prop::Title).map(String::as_str).unwrap_or("");
        editable_prop_field(
            app,
            ui,
            PropField {
                path: &path,
                prop: Prop::Title,
                label: "Title",
                can_edit,
                tooltip: tooltip_reason,
                current_val: p_title,
            },
        );
        let p_auth = prop_map.get(&Prop::Authors).map(String::as_str).unwrap_or("");
        editable_prop_field(
            app,
            ui,
            PropField {
                path: &path,
                prop: Prop::Authors,
                label: "Authors",
                can_edit,
                tooltip: tooltip_reason,
                current_val: p_auth,
            },
        );
        let p_tags = prop_map.get(&Prop::Tags).map(String::as_str).unwrap_or("");
        editable_prop_field(
            app,
            ui,
            PropField {
                path: &path,
                prop: Prop::Tags,
                label: "Tags",
                can_edit,
                tooltip: tooltip_reason,
                current_val: p_tags,
            },
        );
        let p_comm = prop_map.get(&Prop::Comment).map(String::as_str).unwrap_or("");
        editable_prop_field(
            app,
            ui,
            PropField {
                path: &path,
                prop: Prop::Comment,
                label: "Comments",
                can_edit,
                tooltip: tooltip_reason,
                current_val: p_comm,
            },
        );
    });
}

struct PropField<'a> {
    path: &'a Path,
    prop: Prop,
    label: &'static str,
    can_edit: bool,
    tooltip: &'static str,
    current_val: &'a str,
}

fn editable_prop_field(app: &mut App, ui: &mut Ui, f: PropField<'_>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(f.label).weak());
        if app.editing_prop.as_ref().is_some_and(|s| s.path == f.path && s.prop == f.prop) {
            let mut commit = false;
            let mut cancel = false;
            let text_mut = &mut app.editing_prop.as_mut().unwrap().text;
            let resp = ui.add(egui::TextEdit::singleline(text_mut).desired_width(140.0));
            if !resp.has_focus() {
                resp.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            if commit {
                let val = app.editing_prop.as_ref().unwrap().text.clone();
                match explorer_sys::write_prop(f.path, f.prop, &val) {
                    Ok(()) => {
                        if let Some(Some(v)) = app.props.get_mut(f.path)
                            && let Some(idx) = Prop::ALL.iter().position(|&p| p == f.prop)
                        {
                            v[idx] = val;
                        }
                        app.editing_prop = None;
                    }
                    Err(e) => {
                        app.status = Some(format!("Could not save {}: {e}", f.label));
                    }
                }
            } else if cancel {
                app.editing_prop = None;
            }
        } else if f.can_edit {
            let text = if f.current_val.is_empty() { format!("Add {}", f.label) } else { f.current_val.to_string() };
            let color =
                if f.current_val.is_empty() { ui.visuals().weak_text_color() } else { ui.visuals().text_color() };
            let btn = ui.add(egui::Button::new(RichText::new(text).color(color)).frame(false));
            if btn.clicked() {
                app.editing_prop =
                    Some(PropEditState { path: f.path.to_path_buf(), prop: f.prop, text: f.current_val.to_string() });
            }
        } else {
            let resp = ui.add(egui::Label::new(if f.current_val.is_empty() { "-" } else { f.current_val }).wrap());
            resp.on_hover_text(f.tooltip);
        }
    });
}
