//! Fensterrahmen ohne Windows-Titelleiste: eigene Leiste mit Menü, Status,
//! Projektname, Export und den Fensterknöpfen (Minimieren, Maximieren, Schließen),
//! Verschieben per Ziehen, Doppelklick maximiert, Größe über die Fensterränder.

use eframe::egui::{
    self, pos2, vec2, Color32, CursorIcon, Rect, ResizeDirection, RichText, Sense, ViewportCommand,
};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};

const RESIZE_PX: f32 = 5.0;

/// Kleinste Fläche in Punkten, auf die Hauptfenster und Dialoge (Export ~760×690,
/// Einstellungen ~720×520) passen.
const MIN_UI: egui::Vec2 = vec2(900.0, 720.0);

/// Wirksame Oberflächengröße: die Einstellung, aber nur so groß, dass das Fenster (`base` =
/// Größe in Punkten bei 100 %) noch `MIN_UI` Punkte bietet. Nie kleiner als 100 % bzw. die
/// Einstellung selbst, wenn die darunter liegt.
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

/// Anzeige laufender Hintergrundaufgaben in der Statuszeile: Name (bei mehreren ein Zähler) und
/// Fortschritt (`None` = unbekannt → Lauflinie; bei mehreren nur, wenn alle einen melden: Mittel).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskSummary {
    pub label: String,
    pub progress: Option<f32>,
    pub count: usize,
}

/// Fasst laufende Aufgaben (Name, gemeldeter Fortschritt 0…1; 0 = noch keiner) zusammen.
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
    /// Laufende Hintergrundaufgaben (Proxy, Analyse, Untertitel, Import …) in der Statuszeile:
    /// Name, Fortschritts- bzw. Lauflinie, Abbrechen; mehrere = Zähler mit aufklappbarer Liste.
    /// Erscheint erst nach 150 ms (kurze Aufgaben blitzen nicht auf).
    pub fn tasks_badge(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::new("tasks-badge");
        let names: Vec<String> = self.tasks.list.iter().map(|t| crate::tr(&t.label)).collect();
        let list: Vec<(&str, f32)> = names.iter().zip(&self.tasks.list).map(|(n, t)| (n.as_str(), t.progress())).collect();
        let now = task_summary(&list);
        if !theme::loading(ui.ctx(), id, now.is_some()) {
            return;
        }
        // gerade fertig: die letzte Anzeige steht noch kurz, mit voller Linie
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
        if single && theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked() {
            if let Some(t) = self.tasks.list.first() {
                t.cancel();
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
            for (t, name) in self.tasks.list.iter().zip(&names) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name.trim_end_matches(" …")).size(theme::FS_UI));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked() {
                            t.cancel();
                        }
                    });
                });
                let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
                crate::widgets::task_line(ui.painter(), egui::Id::new(("task", t.id)), lr, t.progress());
                ui.add_space(6.0);
            }
        });
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

    /// Unsichtbare Ränder zum Ändern der Fenstergröße (nur wenn nicht maximiert).
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

        // Ganze Leiste ist Zieh-Fläche; Widgets darüber haben Vorrang.
        let drag = ui.interact(bar, ui.id().with("title-drag"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        // Rechts: Fensterknöpfe (feste Breite), Export
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
                // Icon-Glyphe der Fensterknöpfe
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

        // Inhalt links/rechts (ohne die Fensterknöpfe)
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
                // Startseite: Name steht schon als Wortmarke links, hier nur das Logo
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
                            // Icon-Glyphe als dezente Statusanzeige
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
        // Projekt lädt (Oberfläche steht schon, das erste Bild kommt noch): Lauflinie unter dem Titel
        let key = egui::Id::new("project-loading");
        let proj = egui::Id::new(&self.project_path).value();
        let (last, pending, n) = ctx.data(|d| d.get_temp::<(u64, bool, u32)>(key)).unwrap_or((!proj, true, 0));
        let (pending, n) = if last != proj || home { (true, 0) } else { (pending, n.saturating_add(1)) };
        // fertig mit dem ersten Bild – oder es wird keins gebraucht (nach zwei Bildern ohne Anforderung)
        let pending = pending && self.frames.is_empty() && !(n > 2 && !self.core.has_requests());
        ctx.data_mut(|d| d.insert_temp(key, (proj, pending, n)));
        if theme::loading(&ctx, key, pending && !home) {
            crate::widgets::busy_line(ui.painter(), Rect::from_min_max(pos2(bar.left(), bar.bottom() - 2.0), bar.max));
        }
        // Projektname mittig
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
                let item = |ui: &mut egui::Ui, g: &str, t: &str, k: &str| {
                    ui.add(theme::menu_row(crate::tr(format!("{g}   {t}"))).keys(k))
                };
                if item(ui, icon::FILE_PLUS, "Neues Projekt", "").clicked() {
                    ui.close_menu();
                    self.new_project();
                }
                if item(ui, icon::FOLDER_OPEN, "Öffnen …", "Strg+O").clicked() {
                    ui.close_menu();
                    self.open_dialog();
                }
                if item(ui, icon::FLOPPY_DISK, "Speichern", "Strg+S").clicked() {
                    ui.close_menu();
                    self.save(false);
                }
                if item(
                    ui,
                    icon::FLOPPY_DISK_BACK,
                    "Speichern unter …",
                    "Strg+Umschalt+S",
                )
                .clicked()
                {
                    ui.close_menu();
                    self.save(true);
                }
                ui.separator();
                if item(ui, icon::FILE_ARROW_DOWN, &self.t("import-media"), "Strg+I").clicked() {
                    ui.close_menu();
                    self.pick_media();
                }
                // gleicher Dialog wie auf der Startseite; aus, wenn alle Formate abgeschaltet sind
                let can_import = self.import_enabled();
                if ui
                    .add_enabled_ui(can_import, |ui| {
                        item(ui, icon::DOWNLOAD_SIMPLE, "Projekt aus anderem Programm importieren …", "")
                    })
                    .inner
                    .on_hover_text(crate::tr("Aus CapCut, Final Cut Pro oder DaVinci Resolve"))
                    .clicked()
                {
                    ui.close_menu();
                    self.import.open = true;
                }
                if item(ui, icon::TEXT_AA, &self.t("import-fonts"), "").clicked() {
                    ui.close_menu();
                    let ctx = ui.ctx().clone();
                    self.pick_fonts(&ctx);
                }
                if item(ui, icon::EXPORT, &self.t("export"), "Strg+E").clicked() {
                    ui.close_menu();
                    self.commit_live();
                    self.export
                        .open_for(&self.project, self.project_path.as_deref(), self.revision, self.range_in.zip(self.range_out));
                }
                if item(ui, icon::FILE_VIDEO, "Exporte", "").clicked() {
                    ui.close_menu();
                    self.exports_for = Some(crate::exports::ExportsList::new(&self.project.name, self.project_path.as_deref()));
                }
                if item(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Bearbeitungsverlauf", "").clicked() {
                    ui.close_menu();
                    self.show_history();
                }
                if item(ui, icon::LAYOUT, "Layout bearbeiten …", "").clicked() {
                    ui.close_menu();
                    if self.settings.layout_locked {
                        self.toast("Layout ist gesperrt (Einstellungen → Allgemein → Anordnung)");
                    } else {
                        self.player_max = false;
                        self.layout_edit = true;
                    }
                }
                ui.separator();
                if item(ui, icon::COPY_SIMPLE, "Als Vorlage speichern …", "").clicked() {
                    ui.close_menu();
                    self.tpl_dialog = Some(crate::templates::TplDialog::Save {
                        name: self.project.name.clone(),
                    });
                }
                if item(ui, icon::COPY_SIMPLE, "Aus Vorlage einfügen …", "").clicked() {
                    ui.close_menu();
                    self.tpl_dialog = Some(crate::templates::TplDialog::Insert(crate::templates::load_all()));
                }
                if item(ui, icon::ARCHIVE, "Projekt archivieren …", "")
                    .on_hover_text(crate::tr(
                        "Projekt mit allen Medien und Schriften in einen Ordner kopieren",
                    ))
                    .clicked()
                {
                    ui.close_menu();
                    self.start_archive(ui.ctx());
                }
                if item(ui, icon::LINK, "Medien neu verknüpfen …", "").clicked() {
                    ui.close_menu();
                    self.missing_open = true;
                }
                if item(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Sicherungen …", "").clicked() {
                    ui.close_menu();
                    self.backups_open = true;
                }
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
                if item(ui, icon::GEAR_SIX, "Einstellungen …", "Strg+,").clicked() {
                    self.settings_open = true;
                    ui.close_menu();
                }
                if item(ui, icon::HOUSE, "Projektübersicht", "").clicked() {
                    ui.close_menu();
                    self.go_home();
                }
                ui.separator();
                if item(ui, icon::COMPASS, "Rundgang", "").clicked() {
                    ui.close_menu();
                    self.start_tour();
                }
                if item(ui, icon::HAND_WAVING, "Einführung wiederholen", "").clicked() {
                    ui.close_menu();
                    self.start_onboarding();
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
        // eine Aufgabe: ihr Name ohne „…“, Fortschritt erst, sobald einer gemeldet wird
        let one = task_summary(&[("Proxy wird erstellt: a.mp4 …", 0.0)]).unwrap();
        assert_eq!((one.label.as_str(), one.progress, one.count), ("Proxy wird erstellt: a.mp4", None, 1));
        assert_eq!(task_summary(&[("Analyse", 0.4)]).unwrap().progress, Some(0.4));
        // mehrere: Zähler; Mittel nur, wenn alle einen Fortschritt melden, sonst Lauflinie
        let two = task_summary(&[("A", 0.2), ("B", 0.6)]).unwrap();
        assert_eq!(two.label, "2 Aufgaben");
        assert!((two.progress.unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(task_summary(&[("A", 0.2), ("B", 0.0), ("C", 1.0)]).unwrap().progress, None);
        assert_eq!(task_summary(&[("A", 0.2), ("B", 0.0), ("C", 1.0)]).unwrap().count, 3);
    }

    #[test]
    fn ui_scale_keeps_minimum_area() {
        // 1440×900 bei 200 %: nur so groß, dass 900×720 Punkte bleiben
        let s = effective_ui_scale(2.0, vec2(1440.0, 900.0));
        assert!((s - 1.25).abs() < 1e-4, "{s}");
        assert!(1440.0 / s >= MIN_UI.x - 0.01 && 900.0 / s >= MIN_UI.y - 0.01);
        // großes Fenster: Einstellung wirkt voll
        assert_eq!(effective_ui_scale(2.0, vec2(3840.0, 2160.0)), 2.0);
        // kleines Fenster: nie unter 100 % bzw. die kleinere Einstellung
        assert_eq!(effective_ui_scale(1.5, vec2(800.0, 500.0)), 1.0);
        assert_eq!(effective_ui_scale(0.6, vec2(800.0, 500.0)), 0.6);
    }
}
