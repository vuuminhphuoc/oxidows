//! Settings page, saved as `key=value` lines in %APPDATA%\oxidows\taskmgr.ini
//! (`$XDG_CONFIG_HOME/oxidows/taskmgr.ini` elsewhere).

use std::path::PathBuf;

use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};

use crate::Status;

pub use ui_common::Theme;

/// Settings > Run as administrator: how Task Manager starts when it is not elevated already.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AdminMode {
    /// As Windows' own: elevated only through "Run as administrator".
    #[default]
    Off,
    /// Starts itself again through the UAC prompt every time.
    Ask,
    /// Starts itself again through a scheduled task that has the user's full token: no prompt.
    Task,
}

impl AdminMode {
    pub const ALL: [AdminMode; 3] = [AdminMode::Off, AdminMode::Ask, AdminMode::Task];

    pub fn key(self) -> &'static str {
        match self {
            AdminMode::Off => "off",
            AdminMode::Ask => "ask",
            AdminMode::Task => "task",
        }
    }

    pub fn parse(v: &str) -> Self {
        Self::ALL.into_iter().find(|m| m.key() == v).unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            AdminMode::Off => "Only when asked",
            AdminMode::Ask => "Always, with a UAC prompt",
            AdminMode::Task => "Always, without a prompt",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            AdminMode::Off => "Use \"Run as administrator\" at the top of the window when you need it",
            AdminMode::Ask => "Every start asks for administrator rights through UAC",
            AdminMode::Task => {
                "A scheduled task (Task Scheduler > oxidows) starts Task Manager with your full rights, so no \
                 prompt appears. Setting it up asks once. Any program running as you can then start this exe \
                 as administrator too."
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    /// Label of the tab to open on start.
    pub default_tab: String,
    pub interval_ms: u64,
    pub always_on_top: bool,
    /// Hide the window when minimized; the notification-area icon brings it back.
    pub hide_when_minimized: bool,
    /// Minimize after "Switch to".
    pub minimize_on_use: bool,
    pub theme: Theme,
    pub run_as_admin: AdminMode,
    pub group_by_type: bool,
    /// Memory, Disk and Network on the Processes page as percentages.
    pub values_pct: bool,
    /// Visible Details columns, as keys (see `details::Col::key`).
    pub details_columns: String,
    /// Sorted column per table, as `column:asc` / `column:desc`; empty for the table's default.
    /// Task Manager keeps each page's sort order the same way.
    pub sort_processes: String,
    pub sort_history: String,
    pub sort_startup: String,
    pub sort_users: String,
    pub sort_details: String,
    pub sort_services: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_tab: "Processes".into(),
            interval_ms: 1000,
            always_on_top: false,
            hide_when_minimized: false,
            minimize_on_use: false,
            theme: Theme::System,
            run_as_admin: AdminMode::Off,
            group_by_type: true,
            values_pct: false,
            details_columns: crate::details::columns_text(&crate::details::Col::DEFAULT),
            sort_processes: String::new(),
            sort_history: String::new(),
            sort_startup: String::new(),
            sort_users: String::new(),
            sort_details: String::new(),
            sort_services: String::new(),
        }
    }
}

/// Where a start that hands over to the elevated task leaves its command line (the task can't
/// pass one on): `/startup` from Settings > Apps > Startup, for example.
pub fn handoff_path() -> Option<PathBuf> {
    Some(path()?.with_file_name("taskmgr-launch.txt"))
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("oxidows").join("taskmgr.ini"))
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "default_tab" => s.default_tab = v.to_string(),
                "interval_ms" => s.interval_ms = v.parse().unwrap_or(s.interval_ms),
                "always_on_top" => s.always_on_top = v == "true",
                "hide_when_minimized" => s.hide_when_minimized = v == "true",
                "minimize_on_use" => s.minimize_on_use = v == "true",
                "group_by_type" => s.group_by_type = v != "false",
                "values_pct" => s.values_pct = v == "true",
                "details_columns" => s.details_columns = v.to_string(),
                "sort_processes" => s.sort_processes = v.to_string(),
                "sort_history" => s.sort_history = v.to_string(),
                "sort_startup" => s.sort_startup = v.to_string(),
                "sort_users" => s.sort_users = v.to_string(),
                "sort_details" => s.sort_details = v.to_string(),
                "sort_services" => s.sort_services = v.to_string(),
                "theme" => s.theme = Theme::parse(v),
                "run_as_admin" => s.run_as_admin = AdminMode::parse(v),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let theme = self.theme.key();
        let mut t = format!(
            "default_tab={}\ninterval_ms={}\nalways_on_top={}\nhide_when_minimized={}\nminimize_on_use={}\ntheme={theme}\ngroup_by_type={}\nvalues_pct={}\ndetails_columns={}\n",
            self.default_tab,
            self.interval_ms,
            self.always_on_top,
            self.hide_when_minimized,
            self.minimize_on_use,
            self.group_by_type,
            self.values_pct,
            self.details_columns
        );
        for (k, v) in [
            ("sort_processes", &self.sort_processes),
            ("sort_history", &self.sort_history),
            ("sort_startup", &self.sort_startup),
            ("sort_users", &self.sort_users),
            ("sort_details", &self.sort_details),
            ("sort_services", &self.sort_services),
            ("run_as_admin", &self.run_as_admin.key().to_string()),
        ] {
            if !v.is_empty() {
                t.push_str(&format!("{k}={v}\n"));
            }
        }
        t
    }

    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path().ok_or_else(|| std::io::Error::other("no settings folder"))?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, self.to_text())
    }

    pub fn apply_theme(&self, ctx: &egui::Context) {
        ui_common::theme::apply(ctx, self.theme);
    }

    pub fn apply_on_top(&self, ctx: &egui::Context) {
        let level = if self.always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
    }
}

/// Draws the page. Returns true when something changed (the caller saves and applies it).
pub fn show(
    ui: &mut Ui,
    s: &mut Settings,
    tabs: &[&str],
    status: &Status,
    default_app: &mut ui_common::default_app::DefaultApp,
) -> bool {
    let before = s.clone();
    egui::Grid::new("settings").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
        ui.label("Default start page");
        egui::ComboBox::from_id_salt("default_tab").selected_text(s.default_tab.clone()).show_ui(ui, |ui| {
            for t in tabs {
                ui.selectable_value(&mut s.default_tab, (*t).to_string(), *t);
            }
        });
        ui.end_row();

        ui.label("Real time update speed");
        egui::ComboBox::from_id_salt("speed")
            .selected_text(match s.interval_ms {
                0 => "Paused",
                500 => "High (0.5 s)",
                1000 => "Normal (1 s)",
                _ => "Low (2 s)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut s.interval_ms, 500, "High (0.5 s)");
                ui.selectable_value(&mut s.interval_ms, 1000, "Normal (1 s)");
                ui.selectable_value(&mut s.interval_ms, 2000, "Low (2 s)");
                ui.selectable_value(&mut s.interval_ms, 0, "Paused");
            });
        ui.end_row();

        ui.label("App theme");
        ui.horizontal(|ui| {
            for (t, label) in Theme::ALL {
                ui.selectable_value(&mut s.theme, t, label);
            }
        });
        ui.end_row();

        ui.label("Window management");
        ui.vertical(|ui| {
            ui.checkbox(&mut s.always_on_top, "Always on top");
            ui.checkbox(&mut s.minimize_on_use, "Minimize on use")
                .on_hover_text("Minimize Task Manager after \"Switch to\" brings another app forward");
            ui.add_enabled(cfg!(windows), egui::Checkbox::new(&mut s.hide_when_minimized, "Hide when minimized"))
                .on_hover_text("The CPU icon in the notification area brings the window back");
        });
        ui.end_row();

        ui.label("Processes page");
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.group_by_type, "Group by type");
            ui.checkbox(&mut s.values_pct, "Resource values as percentages")
                .on_hover_text("Memory, Disk and Network as a share of the machine's total");
        });
        ui.end_row();

        ui.label("Run as administrator");
        if let Some(msg) = admin_mode(ui, s) {
            status.set(msg);
        }
        ui.end_row();

        ui.label("Default app");
        if let Some(msg) = default_app.show(
            ui,
            &crate::DEFAULT_APP,
            crate::taskmgr_handler,
            taskmgr_sys::set_replaces_taskmgr_as_admin,
        ) {
            status.set(msg);
        }
        ui.end_row();
    });
    ui.add_space(12.0);
    if ui.button("Reset to defaults").clicked() {
        // Like the default app, this one changes Windows (a scheduled task): it stays as it is.
        *s = Settings { run_as_admin: s.run_as_admin, ..Settings::default() };
    }
    *s != before
}

/// A change of [`AdminMode`] that is adding or removing the task (it may be waiting on the UAC
/// prompt, so it runs on its own thread): the mode asked for, and the result once it is done.
type AdminChange = Arc<Mutex<(AdminMode, Option<std::io::Result<()>>)>>;

/// The "Run as administrator" choice. Going to or from [`AdminMode::Task`] adds or removes the
/// task first, and the setting only changes when that worked. Returns a status line, if any.
fn admin_mode(ui: &mut Ui, s: &mut Settings) -> Option<String> {
    let id = egui::Id::new("admin_mode_change");
    let mut status = None;
    let running = ui.data(|d| d.get_temp::<AdminChange>(id));
    if let Some(change) = &running {
        let done = change.lock().ok().and_then(|mut c| c.1.take().map(|r| (c.0, r)));
        if let Some((mode, r)) = done {
            ui.data_mut(|d| d.remove::<AdminChange>(id));
            match r {
                Ok(()) => {
                    s.run_as_admin = mode;
                    status = Some(format!("Run as administrator: {}", mode.label().to_lowercase()));
                }
                Err(e) => status = Some(format!("Could not change how Task Manager starts: {e}")),
            }
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
    let busy = ui.data(|d| d.get_temp::<AdminChange>(id)).is_some();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let mut want = s.run_as_admin;
            ui.add_enabled_ui(cfg!(windows) && !busy, |ui| {
                egui::ComboBox::from_id_salt("run_as_admin").selected_text(want.label()).show_ui(ui, |ui| {
                    for m in AdminMode::ALL {
                        ui.selectable_value(&mut want, m, m.label()).on_hover_text(m.hint());
                    }
                });
            });
            if busy {
                ui.spinner();
                ui.label(
                    RichText::new("Waiting for the administrator prompt…")
                        .color(ui_common::Palette::of(ui).text_secondary),
                );
            }
            if want != s.run_as_admin {
                if (want == AdminMode::Task) == (s.run_as_admin == AdminMode::Task) {
                    s.run_as_admin = want; // no task to add or remove
                } else {
                    let change: AdminChange = Arc::new(Mutex::new((want, None)));
                    let c = change.clone();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let r = taskmgr_sys::set_admin_task_as_admin(want == AdminMode::Task);
                        if let Ok(mut c) = c.lock() {
                            c.1 = Some(r);
                        }
                        ctx.request_repaint();
                    });
                    ui.data_mut(|d| d.insert_temp(id, change));
                }
            }
        });
        ui.label(RichText::new(s.run_as_admin.hint()).color(ui_common::Palette::of(ui).text_secondary));
    });
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            default_tab: "Performance".into(),
            interval_ms: 500,
            always_on_top: true,
            hide_when_minimized: true,
            minimize_on_use: true,
            theme: Theme::Dark,
            run_as_admin: AdminMode::Task,
            group_by_type: false,
            values_pct: true,
            details_columns: "name,pid,cmdline".into(),
            sort_processes: "memory:desc".into(),
            sort_history: "name:asc".into(),
            sort_startup: "impact:desc".into(),
            sort_users: "cpu:desc".into(),
            sort_details: "pid:asc".into(),
            sort_services: "status:desc".into(),
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("junk\ninterval_ms=abc"), Settings::default());
    }
}
