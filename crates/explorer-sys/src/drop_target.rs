//! The window as an OLE drop target, so other programs can drop files on it the way they can on
//! Windows Explorer.
//!
//! winit's own drop handler only reads a file list: it always answers "copy", ignores the
//! pointer position and the Ctrl/Shift/Alt keys, and cannot take anything but `CF_HDROP`. This
//! one forwards the drag to the *shell's* drop target of the folder under the pointer instead,
//! so copy, move and shortcut (Ctrl, Shift, Alt, or the right-button menu), the cursor, the
//! progress and conflict dialogs, the Recycle Bin, zip folders and virtual files (a browser's
//! images, a mail attachment) all behave exactly as in Explorer.
//!
//! Everything here runs on the UI thread: OLE calls the drop target from the window's message
//! loop, and the app reads and steers it between frames. No COM call is made while the state is
//! borrowed, because the shell may run a modal loop (a menu, a dialog) inside `Drop`.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{BHID_SFUIObject, IShellItem};
use windows::core::{Ref, implement};

fn dbg_log(args: std::fmt::Arguments) {
    if std::env::var_os("OXI_DND_DEBUG").is_some() {
        eprintln!("drop target: {args}");
    }
}

fn to_io(e: windows::core::Error) -> io::Error {
    io::Error::other(e)
}

/// The shell's drop target for `dir`, if it can take files at all (This PC, Home and the
/// Libraries list cannot).
fn shell_target(dir: &Path) -> io::Result<IDropTarget> {
    crate::windows::com_init();
    let item: IShellItem = if dir == Path::new(explorer_core::RECYCLE_BIN) {
        use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
        let name: Vec<u16> = "::{645FF040-5081-101B-9F08-00AA002F954E}".encode_utf16().chain(Some(0)).collect();
        unsafe { SHCreateItemFromParsingName(windows::core::PCWSTR(name.as_ptr()), None) }.map_err(to_io)?
    } else {
        match explorer_core::classify(dir) {
            explorer_core::Place::Local | explorer_core::Place::Shell(_) | explorer_core::Place::Unc { .. } => {}
            _ => return Err(io::Error::other("not a folder that takes files")),
        }
        crate::shell_ns::item(dir)?
    };
    unsafe { item.BindToHandler(None, &BHID_SFUIObject) }.map_err(to_io)
}

struct State {
    hwnd: HWND,
    wake: Box<dyn Fn()>,
    /// Between DragEnter and the drop or DragLeave.
    active: bool,
    /// Screen position, keys and allowed effects of the last event.
    pos: POINTL,
    keys: MODIFIERKEYS_FLAGS,
    allowed: DROPEFFECT,
    data: Option<IDataObject>,
    /// The folder the app says the pointer is over.
    wanted: Option<PathBuf>,
    /// The folder whose shell drop target has been entered.
    bound: Option<(PathBuf, IDropTarget)>,
    /// A drop finished since the app last asked.
    dropped: bool,
}

type Shared = Rc<RefCell<State>>;

/// Moves the shell's drag session to the folder the app wants, if that changed. Returns the
/// effect the (new) target answers, or `NONE` when nothing there takes the drag.
fn follow(st: &Shared, entering: bool, effect: &mut DROPEFFECT) {
    let (wanted, bound, data, keys, pos, allowed) = {
        let s = st.borrow();
        (s.wanted.clone(), s.bound.clone(), s.data.clone(), s.keys, s.pos, s.allowed)
    };
    let same = match (&wanted, &bound) {
        (Some(w), Some((b, _))) => w == b,
        (None, None) => true,
        _ => false,
    };
    if same && !entering {
        if let Some((_, t)) = bound {
            let mut e = allowed;
            if unsafe { t.DragOver(keys, pos, &mut e) }.is_ok() {
                *effect = e;
            } else {
                *effect = DROPEFFECT_NONE;
            }
        } else {
            *effect = DROPEFFECT_NONE;
        }
        return;
    }
    if let Some((_, t)) = &bound {
        let _ = unsafe { t.DragLeave() };
    }
    st.borrow_mut().bound = None;
    *effect = DROPEFFECT_NONE;
    let (Some(dir), Some(data)) = (wanted, data) else { return };
    let Ok(t) = shell_target(&dir) else { return };
    let mut e = allowed;
    if unsafe { t.DragEnter(&data, keys, pos, &mut e) }.is_ok() {
        *effect = e;
        st.borrow_mut().bound = Some((dir, t));
    }
}

#[implement(IDropTarget)]
struct Target {
    st: Shared,
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let allowed = unsafe { *pdweffect };
        {
            let mut s = self.st.borrow_mut();
            s.active = true;
            s.keys = grfkeystate;
            s.pos = *pt;
            s.allowed = allowed;
            s.data = pdataobj.as_ref().cloned();
            s.bound = None;
            // The app says where the pointer is once it has drawn a frame; until then, nowhere.
            s.wanted = None;
        }
        let mut effect = DROPEFFECT_NONE;
        follow(&self.st, true, &mut effect);
        dbg_log(format_args!("enter at {},{} effect {}", pt.x, pt.y, effect.0));
        unsafe { *pdweffect = effect };
        (self.st.borrow().wake)();
        Ok(())
    }

    fn DragOver(&self, grfkeystate: MODIFIERKEYS_FLAGS, pt: &POINTL, pdweffect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = unsafe { *pdweffect };
        {
            let mut s = self.st.borrow_mut();
            s.keys = grfkeystate;
            s.pos = *pt;
            s.allowed = allowed;
        }
        let mut effect = DROPEFFECT_NONE;
        follow(&self.st, false, &mut effect);
        unsafe { *pdweffect = effect };
        // The app reads the position on its next frame and says which folder it is over.
        (self.st.borrow().wake)();
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        let bound = {
            let mut s = self.st.borrow_mut();
            s.active = false;
            s.data = None;
            s.wanted = None;
            s.bound.take()
        };
        if let Some((_, t)) = bound {
            let _ = unsafe { t.DragLeave() };
        }
        (self.st.borrow().wake)();
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let allowed = unsafe { *pdweffect };
        {
            let mut s = self.st.borrow_mut();
            s.keys = grfkeystate;
            s.pos = *pt;
            s.allowed = allowed;
            if let Some(d) = pdataobj.as_ref() {
                s.data = Some(d.clone());
            }
        }
        // Make sure the shell target of the folder under the pointer is the one entered.
        let mut effect = DROPEFFECT_NONE;
        follow(&self.st, false, &mut effect);
        let (bound, data, keys, pos) = {
            let mut s = self.st.borrow_mut();
            s.active = false;
            s.wanted = None;
            (s.bound.take(), s.data.take(), s.keys, s.pos)
        };
        let mut result = DROPEFFECT_NONE;
        if let (Some((_, t)), Some(data)) = (bound, data) {
            let mut e = allowed;
            if unsafe { t.Drop(&data, keys, pos, &mut e) }.is_ok() {
                result = e;
            }
        }
        dbg_log(format_args!("drop -> {}", result.0));
        unsafe { *pdweffect = result };
        {
            let mut s = self.st.borrow_mut();
            s.dropped = true;
        }
        (self.st.borrow().wake)();
        Ok(())
    }
}

/// Keeps the window registered as a drop target until dropped.
pub struct DropHost {
    st: Shared,
}

impl DropHost {
    /// Registers `hwnd`; `wake` is called (on the UI thread) whenever a drag moves over it, so the
    /// app repaints and can say which folder it is over. winit's own drop handler must be off
    /// for this window (`with_drag_and_drop(false)`).
    pub fn register(hwnd: isize, wake: impl Fn() + 'static) -> io::Result<DropHost> {
        let hwnd = HWND(hwnd as *mut std::ffi::c_void);
        crate::windows::com_init();
        // OLE (not just COM) has to be up on this thread; winit only does it for its own handler.
        let _ = unsafe { OleInitialize(None) };
        let st = Rc::new(RefCell::new(State {
            hwnd,
            wake: Box::new(wake),
            active: false,
            pos: POINTL::default(),
            keys: MODIFIERKEYS_FLAGS(0),
            allowed: DROPEFFECT_NONE,
            data: None,
            wanted: None,
            bound: None,
            dropped: false,
        }));
        let target: IDropTarget = Target { st: st.clone() }.into();
        unsafe { RegisterDragDrop(hwnd, &target) }.map_err(to_io)?;
        Ok(DropHost { st })
    }

    /// Whether another program is dragging files over the window right now.
    pub fn active(&self) -> bool {
        self.st.borrow().active
    }

    /// The drag position in the window's client area, in pixels.
    pub fn client_pos(&self) -> Option<(i32, i32)> {
        let s = self.st.borrow();
        if !s.active {
            return None;
        }
        let mut p = POINT { x: s.pos.x, y: s.pos.y };
        unsafe { ScreenToClient(s.hwnd, &mut p) }.as_bool().then_some((p.x, p.y))
    }

    /// The folder a drop would go into (`None`: nowhere). Call every frame while [`active`].
    ///
    /// [`active`]: DropHost::active
    pub fn set_target(&self, target: Option<&Path>) {
        let mut s = self.st.borrow_mut();
        if s.wanted.as_deref() != target {
            s.wanted = target.map(Path::to_path_buf);
        }
    }

    /// True once after each finished drop, so the app can refresh the folder.
    pub fn take_dropped(&self) -> bool {
        std::mem::take(&mut self.st.borrow_mut().dropped)
    }
}

impl Drop for DropHost {
    fn drop(&mut self) {
        let hwnd = self.st.borrow().hwnd;
        let _ = unsafe { RevokeDragDrop(hwnd) };
    }
}
