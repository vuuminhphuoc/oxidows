//! Program icons for the process tables, read off the UI thread and cached by exe path.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{TextureHandle, Ui, vec2};
use taskmgr_core::ProcessRow;

/// Logical size of a row icon, as Task Manager draws them.
pub const SIZE: f32 = 16.0;

enum Slot {
    Pending,
    Ready(TextureHandle),
    Missing,
}

struct Inner {
    /// Lower-case exe path ("" = no path: System, Registry, Idle...).
    map: HashMap<String, Slot>,
    tx: Option<Sender<String>>,
    rx: Receiver<(String, Option<taskmgr_sys::IconImage>)>,
}

/// Shared by every tab that lists processes; cheap to clone.
#[derive(Clone)]
pub struct Icons(Rc<RefCell<Inner>>);

impl Icons {
    pub fn new(ctx: &egui::Context) -> Self {
        let (req_tx, req_rx) = channel::<String>();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                while let Ok(path) = req_rx.recv() {
                    let img = taskmgr_sys::exe_icon(&path);
                    if tx.send((path, img)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .is_ok();
        // Off Windows every icon comes back missing; rows keep the same layout.
        let tx = spawned.then_some(req_tx);
        Self(Rc::new(RefCell::new(Inner { map: HashMap::new(), tx, rx })))
    }

    /// Turns finished icons into textures. Call once per frame before drawing rows.
    pub fn poll(&self, ctx: &egui::Context) {
        let mut inner = self.0.borrow_mut();
        while let Ok((key, img)) = inner.rx.try_recv() {
            let slot = match img {
                Some(img) if img.width * img.height * 4 == img.rgba.len() => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
                    Slot::Ready(ctx.load_texture(format!("exe-icon:{key}"), image, egui::TextureOptions::LINEAR))
                }
                _ => Slot::Missing,
            };
            inner.map.insert(key, slot);
        }
    }

    /// How many icons are loaded, and how many paths were asked for (OXI_TASKMGR_TIMING).
    pub fn counts(&self) -> (usize, usize) {
        let inner = self.0.borrow();
        (inner.map.values().filter(|s| matches!(s, Slot::Ready(_))).count(), inner.map.len())
    }

    /// Draws `p`'s icon (or keeps its place while it loads), then a small gap.
    pub fn show(&self, ui: &mut Ui, p: &ProcessRow) {
        let mut inner = self.0.borrow_mut();
        if inner.tx.is_none() {
            return;
        }
        let key = p.info.path.to_lowercase();
        let tex = match inner.map.get(&key) {
            Some(Slot::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                if let Some(tx) = &inner.tx {
                    let _ = tx.send(key.clone());
                }
                inner.map.insert(key, Slot::Pending);
                None
            }
        };
        drop(inner);
        let (rect, _) = ui.allocate_exact_size(vec2(SIZE, SIZE), egui::Sense::hover());
        if let Some(t) = tex {
            egui::Image::new((t.id(), vec2(SIZE, SIZE))).paint_at(ui, rect);
        }
        ui.add_space(2.0);
    }
}
