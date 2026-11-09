//! "Set as default" for the apps: the settings section that shows which program a Windows entry
//! point (Ctrl+Shift+Esc, Win+E, opening a folder) starts, the buttons that take it over or give
//! it back, and the `--set-default` / `--restore-default` command line both apps accept, so the
//! change can also be undone without opening the app.
//!
//! The registry work itself lives in each app's `*-sys` crate; this module only draws and runs it.

use std::io;
use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};

use crate::Palette;

/// Where an entry point goes now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handler {
    /// The program Windows ships (Task Manager, File Explorer), or what this user had before.
    BuiltIn,
    /// This exe.
    ThisApp,
    /// Another copy of this app (moved, or a second download), by its path.
    OtherCopy(String),
    /// A different program took it over (Process Explorer, another file manager), by its path.
    Other(String),
}

/// What the command line asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    SetDefault,
    RestoreDefault,
}

/// `--set-default` / `--restore-default` anywhere on the command line, and whether `--quiet`
/// (no message box at the end) was given too.
pub fn command_from_args<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Option<(Command, bool)> {
    let mut cmd = None;
    let mut quiet = false;
    for a in args {
        match a.as_ref().trim_start_matches(['-', '/']).to_ascii_lowercase().as_str() {
            "set-default" => cmd = Some(Command::SetDefault),
            "restore-default" | "unset-default" => cmd = Some(Command::RestoreDefault),
            "quiet" => quiet = true,
            _ => {}
        }
    }
    cmd.map(|c| (c, quiet))
}

/// Runs a command-line request: applies it, tells the user how it went (a message box on
/// Windows, stderr elsewhere) unless `quiet`, and returns the process exit code.
pub fn run_command(
    title: &str,
    cmd: Command,
    quiet: bool,
    apply: impl FnOnce(bool) -> io::Result<()>,
    texts: &Texts,
) -> i32 {
    let on = cmd == Command::SetDefault;
    let (ok, text) = match apply(on) {
        Ok(()) => (true, if on { texts.now_default } else { texts.now_builtin }.to_string()),
        Err(e) => (false, format!("{}: {e}", texts.failed)),
    };
    if !quiet {
        message_box(title, &text, !ok);
    }
    eprintln!("{text}");
    if ok { 0 } else { 1 }
}

/// A plain OK message box (stderr off Windows).
pub fn message_box(title: &str, text: &str, error: bool) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (t, m) = (w(title), w(text));
        let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
        // SAFETY: NUL-terminated strings that outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | icon) };
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        eprintln!("{title}: {text}");
    }
}

/// The words of one app's section.
pub struct Texts {
    /// What the section is about, e.g. "Ctrl+Shift+Esc and taskmgr.exe".
    pub what: &'static str,
    /// The program that has it otherwise, e.g. "Windows Task Manager".
    pub builtin: &'static str,
    /// Every way in that the change covers, shown under the buttons.
    pub covers: &'static str,
    /// Set when the change needs administrator rights (a UAC prompt appears).
    pub needs_admin: bool,
    /// The exe name, for the recovery hint (`oxi-taskmgr.exe --restore-default`).
    pub exe: &'static str,
    pub now_default: &'static str,
    pub now_builtin: &'static str,
    pub failed: &'static str,
}

type Done = Arc<Mutex<Option<io::Result<()>>>>;

/// State of the section: what the registry said last, and a change still running (it may be
/// waiting on the administrator prompt, so it runs on its own thread).
#[derive(Default)]
pub struct DefaultApp {
    handler: Option<Handler>,
    running: Option<(bool, Done)>,
}

impl DefaultApp {
    /// `handler` is `None` where the feature does not exist (off Windows).
    pub fn new(handler: Option<Handler>) -> Self {
        Self { handler, running: None }
    }

    pub fn handler(&self) -> Option<&Handler> {
        self.handler.as_ref()
    }

    /// Draws the section. `read` re-reads the registry after a change; `apply(true)` makes this
    /// exe the default and `apply(false)` gives it back. Returns a status line to show, if any.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        texts: &Texts,
        read: impl Fn() -> Handler,
        apply: impl Fn(bool) -> io::Result<()> + Send + 'static,
    ) -> Option<String> {
        let mut status = None;
        if let Some((on, done)) = &self.running {
            let result = done.lock().ok().and_then(|mut d| d.take());
            if let Some(r) = result {
                let on = *on;
                self.running = None;
                self.handler = Some(read());
                status = Some(match r {
                    Ok(()) => (if on { texts.now_default } else { texts.now_builtin }).to_string(),
                    Err(e) => format!("{}: {e}", texts.failed),
                });
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        let p = Palette::of(ui);
        let Some(handler) = self.handler.clone() else {
            ui.label(RichText::new("Only on Windows.").color(p.text_secondary));
            return status;
        };
        ui.vertical(|ui| {
            let now = match &handler {
                Handler::BuiltIn => texts.builtin.to_string(),
                Handler::ThisApp => "this app (oxidows)".to_string(),
                Handler::OtherCopy(path) => format!("another copy of this app: {path}"),
                Handler::Other(path) => format!("another program: {path}"),
            };
            ui.label(format!("{} opens {now}", texts.what));
            ui.horizontal(|ui| {
                let busy = self.running.is_some();
                let set = ui
                    .add_enabled(!busy && handler != Handler::ThisApp, egui::Button::new("Set as default"))
                    .on_hover_text(texts.covers);
                // Undo only takes out what this app put in; another program's change stays.
                let ours = matches!(handler, Handler::ThisApp | Handler::OtherCopy(_));
                let restore = ui.add_enabled(
                    !busy && ours,
                    egui::Button::new(format!("Restore {}", texts.builtin)),
                );
                let want = if set.clicked() {
                    Some(true)
                } else if restore.clicked() {
                    Some(false)
                } else {
                    None
                };
                if let Some(on) = want {
                    let done: Done = Arc::default();
                    let d = done.clone();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let r = apply(on);
                        if let Ok(mut d) = d.lock() {
                            *d = Some(r);
                        }
                        ctx.request_repaint();
                    });
                    self.running = Some((on, done));
                }
                if busy {
                    ui.spinner();
                    if texts.needs_admin {
                        ui.label(RichText::new("Waiting for the administrator prompt…").color(p.text_secondary));
                    }
                }
            });
            ui.label(RichText::new(texts.covers).color(p.text_secondary));
            if texts.needs_admin {
                ui.label(RichText::new("Needs administrator rights: Windows asks for them.").color(p.text_secondary));
            }
            ui.label(
                RichText::new(format!(
                    "Restore the default before moving or deleting this exe. To undo without the app: {} --restore-default",
                    texts.exe
                ))
                .color(p.text_secondary),
            );
        });
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(command_from_args(["--set-default"]), Some((Command::SetDefault, false)));
        assert_eq!(command_from_args(["/restore-default", "--quiet"]), Some((Command::RestoreDefault, true)));
        assert_eq!(command_from_args(["C:\\Windows\\System32\\Taskmgr.exe", "/0", "/startup"]), None);
        assert_eq!(command_from_args(Vec::<String>::new()), None);
    }
}
