//! Settings page, saved as `key=value` lines in %APPDATA%\oxidows\taskmgr.ini
//! (`$XDG_CONFIG_HOME/oxidows/taskmgr.ini` elsewhere).

use std::path::PathBuf;

use egui::Ui;

use crate::Status;

pub use ui_common::Theme;

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
    pub group_by_type: bool,
    /// Memory, Disk and Network on the Processes page as percentages.
    pub values_pct: bool,
    /// Visible Details columns, as keys (see `details::Col::key`).
    pub details_columns: String,
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
            group_by_type: true,
            values_pct: false,
            details_columns: crate::details::columns_text(&crate::details::Col::DEFAULT),
        }
    }
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
                "theme" => s.theme = Theme::parse(v),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let theme = self.theme.key();
        format!(
            "default_tab={}\ninterval_ms={}\nalways_on_top={}\nhide_when_minimized={}\nminimize_on_use={}\ntheme={theme}\ngroup_by_type={}\nvalues_pct={}\ndetails_columns={}\n",
            self.default_tab,
            self.interval_ms,
            self.always_on_top,
            self.hide_when_minimized,
            self.minimize_on_use,
            self.group_by_type,
            self.values_pct,
            self.details_columns
        )
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
        *s = Settings::default();
    }
    *s != before
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
            group_by_type: false,
            values_pct: true,
            details_columns: "name,pid,cmdline".into(),
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("junk\ninterval_ms=abc"), Settings::default());
    }
}
