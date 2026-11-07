//! The right-hand pane: Preview (Alt+P) hosts the file type's Windows preview handler (Office,
//! PDF, mail, fonts, …) when one is registered, and otherwise shows a thumbnail or the start of
//! a text file; Details (Alt+Shift+P) shows the selection's properties.

use std::io::Read;
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use explorer_core::{
    Entry, decode_text, format_attributes, format_bytes, format_size_kb, format_time, icon_for, is_text_ext,
};

use crate::{App, IconKey, IconState, Msg};

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
                if !is_text && self.hwnd != 0 && explorer_sys::has_preview_handler(&ext) {
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
    egui::Panel::right("pane").resizable(true).default_size(250.0).show(ui, |ui| match app.pane {
        Pane::Preview => preview(app, ui),
        Pane::Details => details(app, ui),
        Pane::None => {}
    });
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
                Err(_) => {
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
    let selected: Vec<&Entry> = app.entries.iter().filter(|e| app.selected.contains(&e.name)).collect();
    let rows: Vec<(&str, String)> = match selected.as_slice() {
        [] => {
            let files = app.view.iter().filter(|&&i| !app.entries[i].is_dir).count();
            ui.heading(
                app.cur.file_name().map_or_else(|| app.cur.display().to_string(), |n| n.to_string_lossy().into_owned()),
            );
            vec![("Items", app.view.len().to_string()), ("Files", files.to_string())]
        }
        [e] => {
            ui.horizontal(|ui| {
                let key = IconKey::for_entry(e, &app.cur);
                match app.icons.get(&key) {
                    Some(IconState::Ready(tex)) => {
                        ui.add(egui::Image::new((tex.id(), egui::vec2(32.0, 32.0))));
                    }
                    _ => {
                        ui.label(RichText::new(icon_for(e)).size(28.0));
                    }
                }
                ui.add(egui::Label::new(RichText::new(e.display()).heading()).wrap());
            });
            let mut r = vec![("Type", e.type_name())];
            if !e.is_dir {
                let exact = format!("{} bytes", explorer_core::group_thousands(e.size));
                r.push(("Size", if e.size < 1024 { exact } else { format!("{} ({exact})", format_bytes(e.size)) }));
            }
            r.push(("Date modified", format_time(e.modified, app.utc_offset)));
            r.push(("Date created", format_time(e.created, app.utc_offset)));
            r.push(("Attributes", format_attributes(e.attrs)));
            if let Some(n) = &e.note {
                r.push(("Original location", n.clone()));
            }
            r
        }
        many => {
            let size: u64 = many.iter().map(|e| e.size).sum();
            ui.heading(format!("{} items selected", many.len()));
            let newest = many.iter().map(|e| e.modified).max().unwrap_or(0);
            let oldest = many.iter().map(|e| e.modified).filter(|&t| t > 0).min().unwrap_or(0);
            vec![
                ("Total size", format_size_kb(size)),
                (
                    "Date modified",
                    format!("{} – {}", format_time(oldest, app.utc_offset), format_time(newest, app.utc_offset)),
                ),
            ]
        }
    };
    ui.separator();
    egui::Grid::new("details-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(RichText::new(k).weak());
            ui.add(egui::Label::new(v).wrap());
            ui.end_row();
        }
    });
}
