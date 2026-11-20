//! Window frame without the Windows title bar: own bar with menu, status,
//! project name, export and the window buttons (minimize, maximize, close),
//! move by dragging, double-click maximizes, resize via the window edges.

use eframe::egui::{
    self, pos2, vec2, Color32, CursorIcon, Rect, ResizeDirection, RichText, Sense, ViewportCommand,
};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};

const RESIZE_PX: f32 = 5.0;

/// Smallest area in points that the main window and dialogs (export ~760×690,
/// settings ~720×520) fit into.
const MIN_UI: egui::Vec2 = vec2(900.0, 720.0);

/// Effective UI scale: the setting, but only so large that the window (`base` =
/// size in points at 100 %) still offers `MIN_UI` points. Never below 100 % or the
/// setting itself if that is lower.
pub fn effective_ui_scale(setting: f32, base: egui::Vec2) -> f32 {
    let fit = (base.x / MIN_UI.x).min(base.y / MIN_UI.y);
    setting.min(fit.max(setting.min(1.0)))
}

pub fn load_ui_scale() -> f32 {
    if let Some(v) = std::env::var("CUTLINE_UI_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        return v.clamp(0.6, 2.0);
    }
    std::fs::read_to_string(paths::data_dir().join("ui_scale"))
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.6, 2.0))
        .unwrap_or(1.0)
}

/// Display of running background tasks in the status bar: name (with several a counter) and
/// progress (`None` = unknown → running line; with several only if all report one: the mean).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskSummary {
    pub label: String,
    pub progress: Option<f32>,
    pub count: usize,
}

/// Summarizes running tasks (name, reported progress 0…1; 0 = none yet).
pub fn task_summary(tasks: &[(&str, f32)]) -> Option<TaskSummary> {
    let (first, _) = tasks.first()?;
    let known = tasks.iter().all(|t| t.1 > 0.0);
    Some(TaskSummary {
        label: if tasks.len() == 1 {
            first.trim_end_matches(" …").trim_end_matches('…').trim_end().to_string()
        } else {
            format!("{} Aufgaben", tasks.len())
        },
        progress: known.then(|| tasks.iter().map(|t| t.1.clamp(0.0, 1.0)).sum::<f32>() / tasks.len() as f32),
        count: tasks.len(),
    })
}

impl App {
    /// Running background tasks (proxy, analysis, captions, import …) in the status bar:
    /// name, progress or running line, cancel; several = counter with an expandable list.
    /// Appears only after 150 ms (short tasks do not flash).
    /// Rows of the task display: tasks of the same kind as one row („Freistellen ×3“), proxies
    /// with overall progress. Between two proxy jobs (and while paused) the round stays
    /// as a row without a task (empty group), so pause/resume does not disappear.
    fn task_rows(&self) -> Vec<(String, f32, Vec<usize>)> {
        let keys: Vec<&str> = self.tasks.list.iter().map(|t| t.key.as_str()).collect();
        let groups = crate::tasks::groups(&keys);
        let (pd, pt) = (self.proxies.done, self.proxies.total);
        let mut rows: Vec<(String, f32, Vec<usize>)> = groups
            .into_iter()
            .map(|g| {
                let t = &self.tasks.list[g[0]];
                let mut name = crate::tr(&t.label);
                let mut p = g.iter().map(|i| self.tasks.list[*i].progress()).sum::<f32>() / g.len() as f32;
                if g.len() > 1 {
                    name = format!("{}  ×{}", name.trim_end_matches(" …"), g.len());
                }
                if t.key == "proxy" && pt > 0 {
                    p = ((pd as f32 + p) / pt as f32).max(1e-3);
                }
                (name, p, g)
            })
            .collect();
        if pt > 0 && pd < pt && !self.proxies.stopped && self.settings.use_proxies && self.tasks.running("proxy").is_none() {
            rows.push((crate::tr(format!("Proxys {} / {}", pd + 1, pt)), (pd as f32 / pt as f32).max(1e-3), Vec::new()));
        }
        rows
    }

    pub fn tasks_badge(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::new("tasks-badge");
        let rows = self.task_rows();
        let list: Vec<(&str, f32)> = rows.iter().map(|(n, p, _)| (n.as_str(), *p)).collect();
        let now = task_summary(&list);
        if !theme::loading(ui.ctx(), id, now.is_some()) {
            return;
        }
        // just finished: the last display stays briefly, with a full line
        let s = match now {
            Some(s) => {
                ui.data_mut(|d| d.insert_temp(id, s.clone()));
                s
            }
            None => match ui.data(|d| d.get_temp::<TaskSummary>(id)) {
                Some(s) => TaskSummary { progress: Some(1.0), ..s },
                None => return,
            },
        };
        let single = s.count == 1;
        if single {
            if let Some((_, _, g)) = rows.first() {
                let g = g.clone();
                self.task_buttons(ui, &g);
            }
        }
        let (lr, _) = ui.allocate_exact_size(vec2(56.0, 3.0), Sense::hover());
        crate::widgets::task_line(ui.painter(), id, lr, s.progress.unwrap_or(0.0));
        let text = if single { s.label.clone() } else { format!("{}  {}", s.label, icon::CARET_DOWN) };
        let font = egui::FontId::proportional(theme::FS_UI);
        let shown = theme::elide(&crate::tr(&text), 220.0, |t| theme::text_width(ui.ctx(), t, font.clone()));
        let r = ui.add(egui::Label::new(RichText::new(&shown).size(theme::FS_UI).color(theme::TEXT_DIM())).sense(Sense::click()));
        if single {
            if shown != s.label {
                r.on_hover_text(&s.label);
            }
            return;
        }
        let pop = id.with("list");
        if r.clicked() {
            ui.memory_mut(|m| m.toggle_popup(pop));
        }
        egui::popup::popup_below_widget(ui, pop, &r, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(260.0);
            for (name, p, g) in &rows {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name.trim_end_matches(" …")).size(theme::FS_UI));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.task_buttons(ui, g);
                    });
                });
                let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
                crate::widgets::task_line(ui.painter(), egui::Id::new(("task", g.first().map(|i| self.tasks.list[*i].id))), lr, *p);
                ui.add_space(6.0);
            }
        });
    }

    /// Cancel (all of the group) and for proxies pause/resume; cancelling the proxies
    /// ends proxy building for this project.
    fn task_buttons(&mut self, ui: &mut egui::Ui, group: &[usize]) {
        // empty group: proxy round between two jobs (see `task_rows`)
        let proxy = group.is_empty() || group.iter().any(|i| self.tasks.list.get(*i).is_some_and(|t| t.key == "proxy"));
        if theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked() {
            if proxy {
                self.stop_proxies();
            }
            for i in group {
                if let Some(t) = self.tasks.list.get(*i) {
                    t.cancel();
                }
            }
        }
        if proxy {
            let paused = crate::proxy::ProxyState::paused();
            let (g, tip) = if paused { (icon::PLAY, "Fortsetzen") } else { (icon::PAUSE, "Pausieren") };
            if theme::icon_button(ui, g, tip, true, false).clicked() {
                crate::proxy::ProxyState::set_paused(!paused);
            }
        }
    }

    pub fn set_ui_scale(&mut self, ctx: &egui::Context, v: f32) {
        self.ui_scale = ((v * 10.0).round() / 10.0).clamp(0.6, 2.0);
        ctx.set_zoom_factor(self.ui_scale);
        self.settings.ui_scale = self.ui_scale;
        self.settings.save();
        self.toast(format!(
            "{} Oberfläche {:.0} %",
            icon::MAGNIFYING_GLASS,
            self.ui_scale * 100.0
        ));
    }

    /// Invisible edges for resizing the window (only when not maximized).
    pub fn window_frame(&mut self, ctx: &egui::Context) {
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized {
            return;
        }
        let r = ctx.screen_rect();
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let (l, rt, t, b) = (
            p.x - r.left() < RESIZE_PX,
            r.right() - p.x < RESIZE_PX,
            p.y - r.top() < RESIZE_PX,
            r.bottom() - p.y < RESIZE_PX,
        );
        let dir = match (l, rt, t, b) {
            (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
            (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
            (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
            (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
            (true, ..) => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
            (_, true, ..) => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
            (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
            (.., true) => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
            _ => None,
        };
        if let Some((d, c)) = dir {
            ctx.set_cursor_icon(c);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
            }
        }
    }

    pub fn title_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let bar = ui.max_rect();

        // The whole bar is a drag area; widgets on top take precedence.
        // without keyboard focus: Tab should not outline the whole bar
        let drag = ui.interact(bar, ui.id().with("title-drag"), Sense::CLICK | Sense::DRAG);
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        // Right: window buttons (fixed width), export
        let ctrl_w = 46.0 * 3.0;
        let ctrl_rect = Rect::from_min_max(pos2(bar.right() - ctrl_w, bar.top()), bar.max);
        let btn = |ui: &mut egui::Ui, r: Rect, glyph: &str, danger: bool, id: &str| -> bool {
            let resp = ui.interact(r, ui.id().with(id), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(
                    r,
                    0.0,
                    if danger {
                        Color32::from_rgb(0xC4, 0x2B, 0x1C)
                    } else {
                        theme::BG_RAISED()
                    },
                );
            }
            let col = if resp.hovered() && danger {
                Color32::WHITE
            } else {
                theme::TEXT_DIM()
            };
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                crate::tr(glyph),
                // icon glyph of the window buttons
                egui::FontId::proportional(13.0),
                col,
            );
            resp.clicked()
        };
        let cell = |i: f32| {
            Rect::from_min_size(
                pos2(ctrl_rect.left() + i * 46.0, bar.top()),
                vec2(46.0, bar.height()),
            )
        };
        if btn(ui, cell(0.0), icon::MINUS, false, "win-min") {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if btn(
            ui,
            cell(1.0),
            if maximized {
                icon::COPY_SIMPLE
            } else {
                icon::SQUARE
            },
            false,
            "win-max",
        ) {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        if btn(ui, cell(2.0), icon::X, true, "win-close") {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // Content left/right (without the window buttons)
        let content = Rect::from_min_max(bar.min, pos2(ctrl_rect.left() - 8.0, bar.bottom()));
        let home = self.screen == crate::app::Screen::Home;
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(content)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (lr, lresp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                theme::logo(ui.painter(), lr);
                // Start page: the name already stands as a word mark on the left, only the logo here
                if home {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            icon::GEAR_SIX,
                            "Einstellungen (Strg+,)",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.settings_open = true;
                        }
                        self.update_badge(ui);
                        self.tasks_badge(ui);
                    });
                    return;
                }
                if lresp
                    .on_hover_text(crate::tr("Zur Projektübersicht"))
                    .clicked()
                {
                    self.go_home();
                    return;
                }
                if theme::icon_button(ui, icon::HOUSE, "Projektübersicht", true, false).clicked() {
                    self.go_home();
                    return;
                }
                self.main_menu(ui);
                let (glyph, status, col) =
                    if self.revision == self.saved_revision && self.project_path.is_some() {
                        let when = self.last_saved.map(|t| t.elapsed().as_secs()).map(|s| {
                            if s < 60 {
                                crate::tr("gerade eben")
                            } else {
                                crate::tr(format!("vor {} Min.", s / 60))
                            }
                        });
                        (
                            icon::CHECK_CIRCLE,
                            when.map(|w| format!("Gespeichert {w}"))
                                .unwrap_or_else(|| "Gespeichert".into()),
                            theme::TEXT_DIM(),
                        )
                    } else if self.project_path.is_none() && self.revision == self.saved_revision {
                        (
                            icon::CIRCLE_DASHED,
                            "Noch nicht gespeichert".to_string(),
                            theme::TEXT_DIM(),
                        )
                    } else {
                        (
                            icon::CIRCLE_DASHED,
                            "Wird gespeichert …".to_string(),
                            theme::TEXT_DIM(),
                        )
                    };
                let narrow = ui.available_width() < 560.0;
                let resp = ui.label(
                    RichText::new(crate::tr(if narrow {
                        glyph.to_string()
                    } else {
                        format!("{glyph} {status}")
                    }))
                    .size(theme::FS_UI)
                    .color(col),
                );
                resp.on_hover_text(crate::tr(
                    self.project_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or(status),
                ));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let ex = ui.add(theme::primary(format!(
                        "{}  {}",
                        icon::EXPORT,
                        self.t("export")
                    )));
                    self.tour_rects.insert("export", ex.rect);
                    if ex.on_hover_text(crate::tr("Strg+E")).clicked() {
                        self.commit_live();
                        self.export
                            .open_for(&self.project, self.project_path.as_deref(), self.revision, self.range_in.zip(self.range_out));
                    }
                    if theme::icon_button(ui, icon::GEAR_SIX, "Einstellungen (Strg+,)", true, false)
                        .clicked()
                    {
                        self.settings_open = true;
                    }
                    self.update_badge(ui);
                    self.tasks_badge(ui);
                    if self.mcp.is_some() {
                        ui.label(
                            // icon glyph as a subtle status indicator
                            RichText::new(icon::PLUGS_CONNECTED)
                                .size(12.0)
                                .color(theme::TEXT_DIM()),
                        )
                        .on_hover_text(crate::tr(
                            "MCP-Server aktiv (Einstellungen → MCP-Schnittstelle)",
                        ));
                    }
                });
            },
        );
        // Project is loading (UI is already there, the first frame is still coming): running line under the title
        let key = egui::Id::new("project-loading");
        let proj = egui::Id::new(&self.project_path).value();
        let (last, pending, n) = ctx.data(|d| d.get_temp::<(u64, bool, u32)>(key)).unwrap_or((!proj, true, 0));
        let (pending, n) = if last != proj || home { (true, 0) } else { (pending, n.saturating_add(1)) };
        // done with the first frame – or none is needed (after two frames without a request)
        let pending = pending && self.frames.is_empty() && !(n > 2 && !self.core.has_requests());
        ctx.data_mut(|d| d.insert_temp(key, (proj, pending, n)));
        if theme::loading(&ctx, key, pending && !home) {
            crate::widgets::busy_line(ui.painter(), Rect::from_min_max(pos2(bar.left(), bar.bottom() - 2.0), bar.max));
        }
        // Project name centered
        if !home {
            let name = self.project.name.clone();
            let galley =
                ui.painter()
                    .layout_no_wrap(name, egui::FontId::proportional(theme::FS_TEXT), theme::TEXT());
            let nr = Rect::from_center_size(bar.center(), galley.size());
            if nr.left() > bar.left() + 330.0 && nr.right() < ctrl_rect.left() - 170.0 {
                ui.painter().galley(nr.min, galley, theme::TEXT());
            }
        }
    }

    fn main_menu(&mut self, ui: &mut egui::Ui) {
        let menu = ui.menu_button(
            RichText::new(crate::tr(format!(
                "{} {}",
                self.t("menu"),
                icon::CARET_DOWN
            )))
            .size(theme::FS_TEXT),
            |ui| {
                ui.set_min_width(230.0);
                let ctx = ui.ctx().clone();
                // Entries are actions from `keymap::ACTIONS` and run via `run_action` (like the
                // command search); text as before in the menu, shortcut from the current bindings
                let mut act: Option<&str> = None;
                let mut item = |app: &Self, ui: &mut egui::Ui, id: &'static str, t: &str| {
                    let g = crate::keymap::icon(id);
                    let r = ui.add_enabled(
                        app.action_enabled(id),
                        theme::menu_row(crate::tr(format!("{g}   {t}"))).keys(&app.key_of(id)),
                    );
                    if r.clicked() {
                        act = Some(id);
                    }
                    r
                };
                item(self, ui, "new_project", "Neues Projekt");
                item(self, ui, "open", "Öffnen …");
                item(self, ui, "save", "Speichern");
                item(self, ui, "save_as", "Speichern unter …");
                ui.separator();
                item(self, ui, "import", &self.t("import-media"));
                // same dialog as on the start page; off when all formats are switched off
                item(self, ui, "import_project", "Projekt aus anderem Programm importieren …")
                    .on_hover_text(crate::tr("Aus CapCut, Final Cut Pro oder DaVinci Resolve"));
                item(self, ui, "import_fonts", &self.t("import-fonts"));
                item(self, ui, "export", &self.t("export"));
                item(self, ui, "exports", "Exporte");
                item(self, ui, "history", "Bearbeitungsverlauf");
                item(self, ui, "layout_edit", "Layout bearbeiten …");
                ui.separator();
                item(self, ui, "template_save", "Als Vorlage speichern …");
                item(self, ui, "template_insert", "Aus Vorlage einfügen …");
                item(self, ui, "archive", "Projekt archivieren …").on_hover_text(crate::tr(
                    "Projekt mit allen Medien und Schriften in einen Ordner kopieren",
                ));
                item(self, ui, "relink", "Medien neu verknüpfen …");
                item(self, ui, "backups", "Sicherungen …");
                ui.separator();
                theme::submenu(
                    ui,
                    icon::MAGNIFYING_GLASS,
                    &format!("Oberflächengröße  ({:.0} %)", self.ui_scale * 100.0),
                    |ui| {
                        for v in [0.8, 0.9, 1.0, 1.1, 1.25, 1.5] {
                            if ui
                                .selectable_label(
                                    (self.ui_scale - v).abs() < 0.01,
                                    crate::tr(format!("{:.0} %", v * 100.0)),
                                )
                                .clicked()
                            {
                                let ctx = ui.ctx().clone();
                                self.set_ui_scale(&ctx, v);
                                ui.close_menu();
                            }
                        }
                        ui.label(
                            RichText::new(crate::tr("Strg + / Strg − / Strg 0"))
                                .small()
                                .color(theme::TEXT_DIM()),
                        );
                    },
                );
                ui.separator();
                item(self, ui, "settings", "Einstellungen …");
                item(self, ui, "go_home", "Projektübersicht");
                ui.separator();
                item(self, ui, "tour", "Rundgang");
                item(self, ui, "onboarding", "Einführung wiederholen");
                if let Some(a) = act {
                    ui.close_menu();
                    self.run_action(&ctx, a);
                }
            },
        );
        self.tour_rects.insert("menu", menu.response.rect);
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn tasks_are_summarized_for_the_status_bar() {
        assert_eq!(task_summary(&[]), None);
        // one task: its name without „…“, progress only once one is reported
        let one = task_summary(&[("Proxy wird erstellt: a.mp4 …", 0.0)]).unwrap();
        assert_eq!((one.label.as_str(), one.progress, one.count), ("Proxy wird erstellt: a.mp4", None, 1));
        assert_eq!(task_summary(&[("Analyse", 0.4)]).unwrap().progress, Some(0.4));
        // several: counter; mean only if all report progress, otherwise running line
        let two = task_summary(&[("A", 0.2), ("B", 0.6)]).unwrap();
        assert_eq!(two.label, "2 Aufgaben");
        assert!((two.progress.unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(task_summary(&[("A", 0.2), ("B", 0.0), ("C", 1.0)]).unwrap().progress, None);
        assert_eq!(task_summary(&[("A", 0.2), ("B", 0.0), ("C", 1.0)]).unwrap().count, 3);
    }

    /// Pausing proxies: between two jobs the button disappeared (click into nothing), and
    /// despite the pause the next job started.
    #[test]
    fn proxy_pause_stays_between_builds_and_holds_the_next() {
        let (mut app, _home, _g) = crate::saving::tests::test_app();
        app.settings.use_proxies = true;
        (app.proxies.done, app.proxies.total) = (3, 10);
        let rows = app.task_rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].2.is_empty() && rows[0].0.contains("4 / 10"), "{rows:?}");
        app.proxies.stopped = true;
        assert!(app.task_rows().is_empty(), "cancelled: no row");
        assert!(crate::proxy::start_next(5, false, false));
        assert!(!crate::proxy::start_next(5, false, true), "paused: no new job");
        assert!(!crate::proxy::start_next(5, true, false) && !crate::proxy::start_next(0, false, false));
    }

    #[test]
    fn first_tab_does_not_focus_the_title_bar_drag_area() {
        let (mut app, _home, _g) = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let mut bar = egui::Rect::NOTHING;
        for tab in [false, true, false] {
            let events = if tab {
                vec![egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]
            } else {
                Vec::new()
            };
            let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| {
                egui::TopBottomPanel::top("t").exact_height(40.0).show(ctx, |ui| {
                    bar = ui.max_rect();
                    app.title_bar(ui);
                });
            });
        }
        let focused = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id));
        assert!(focused.is_none_or(|r| r.rect != bar), "focus on the drag area");
    }

    #[test]
    fn ui_scale_keeps_minimum_area() {
        // 1440×900 at 200 %: only so large that 900×720 points remain
        let s = effective_ui_scale(2.0, vec2(1440.0, 900.0));
        assert!((s - 1.25).abs() < 1e-4, "{s}");
        assert!(1440.0 / s >= MIN_UI.x - 0.01 && 900.0 / s >= MIN_UI.y - 0.01);
        // large window: the setting applies fully
        assert_eq!(effective_ui_scale(2.0, vec2(3840.0, 2160.0)), 2.0);
        // small window: never below 100 % or the smaller setting
        assert_eq!(effective_ui_scale(1.5, vec2(800.0, 500.0)), 1.0);
        assert_eq!(effective_ui_scale(0.6, vec2(800.0, 500.0)), 0.6);
    }
}
