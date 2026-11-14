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

impl App {
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
                                "gerade eben".to_string()
                            } else {
                                format!("vor {} min", s / 60)
                            }
                        });
                        (
                            icon::CHECK_CIRCLE,
                            when.map(|w| format!("Gespeichert {w}"))
                                .unwrap_or_else(|| "Gespeichert".into()),
                            theme::TEXT_FAINT(),
                        )
                    } else if self.project_path.is_none() && self.revision == self.saved_revision {
                        (
                            icon::CIRCLE_DASHED,
                            "Noch nicht gespeichert".to_string(),
                            theme::TEXT_FAINT(),
                        )
                    } else {
                        (
                            icon::CIRCLE_DASHED,
                            "Wird gespeichert …".to_string(),
                            theme::TEXT_FAINT(),
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
                            .open_for(&self.project, self.project_path.as_deref(), self.revision);
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
                                .color(theme::TEXT_FAINT()),
                        )
                        .on_hover_text(crate::tr(
                            "MCP-Server aktiv (Einstellungen → MCP-Schnittstelle)",
                        ));
                    }
                });
            },
        );
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
                        .open_for(&self.project, self.project_path.as_deref(), self.revision);
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
                    self.player_max = false;
                    self.layout_edit = true;
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
                    self.tpl_dialog = Some(crate::templates::TplDialog::Insert);
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
