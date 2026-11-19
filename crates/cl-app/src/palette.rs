//! Befehlssuche (Strg+K): Feld mit Trefferliste oben über dem Editor. Sucht in den Aktionen aus
//! `keymap::ACTIONS` und führt sie über `App::run_action` aus – wie Tastenkürzel und Menüs.

use eframe::egui::{self, Key, Modifiers};

use crate::app::App;
use crate::keymap;
use crate::theme;

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    sel: usize,
    /// zuletzt ausgeführte Befehle dieser Sitzung, neueste zuerst (nicht gespeichert)
    recent: Vec<&'static str>,
}

impl Palette {
    pub fn show(&mut self) {
        self.open = true;
        self.query.clear();
        self.sel = 0;
    }

    fn used(&mut self, id: &'static str) {
        self.recent.retain(|r| *r != id);
        self.recent.insert(0, id);
        self.recent.truncate(5);
    }
}

/// Suchbare Befehle: (Aktion, [Name in der Oberflächensprache, deutscher Name falls anders])
fn items() -> Vec<(&'static str, Vec<String>)> {
    keymap::ACTIONS
        .iter()
        .filter(|a| a.0 != "command_search")
        .map(|(id, de, _)| {
            let shown = crate::tr(de);
            let names = if shown == *de { vec![shown] } else { vec![shown, de.to_string()] };
            (*id, names)
        })
        .collect()
}

impl App {
    /// Zeichnet die Befehlssuche, solange sie offen ist. Läuft vor den Bereichen des Editors, damit
    /// Esc, Enter und die Pfeile nur hier ankommen (Esc hebt dann keine Auswahl auf).
    pub fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let list = keymap::search(&self.palette.query, &items(), &self.palette.recent);
        let (esc, enter, up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        let last = list.len().saturating_sub(1);
        let mut sel = self.palette.sel.min(last);
        if down {
            sel = (sel + 1).min(last);
        }
        if up {
            sel = sel.saturating_sub(1);
        }
        let mut run = list.get(sel).copied().filter(|id| enter && self.action_enabled(id));
        let binds = keymap::bindings(&self.settings.shortcuts);
        let width = 460.0_f32.min(ctx.screen_rect().width() - 32.0);
        let area = egui::Area::new(egui::Id::new("command-search"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, theme::TITLE_H + 40.0))
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_width(width);
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.palette.query)
                            .hint_text(crate::tr("Befehl suchen"))
                            .font(egui::FontId::proportional(theme::FS_TITLE))
                            .frame(false)
                            .margin(egui::Margin::symmetric(8, 6))
                            .desired_width(f32::INFINITY),
                    );
                    field.request_focus();
                    if field.changed() {
                        sel = 0;
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            for (k, id) in list.iter().enumerate() {
                                let key = binds.iter().find(|(_, a)| a == id).map(|(c, _)| c.label()).unwrap_or_default();
                                // ohne Symbol ein Geviert, damit die Namen untereinander stehen
                                let g = Some(keymap::icon(id)).filter(|g| !g.is_empty()).unwrap_or("\u{2003}");
                                let mark = ui.painter().add(egui::Shape::Noop);
                                let row = ui.add_enabled(
                                    self.action_enabled(id),
                                    theme::menu_row(format!("{g}   {}", crate::tr(keymap::name(id)))).keys(&key),
                                );
                                if k == sel {
                                    ui.painter().set(mark, egui::Shape::rect_filled(row.rect, theme::R_CTRL, theme::BG_HOVER()));
                                    if up || down {
                                        row.scroll_to_me(None);
                                    }
                                }
                                if row.clicked() {
                                    run = Some(id);
                                }
                            }
                        });
                    });
                });
            });
        self.palette.sel = sel;
        if esc || run.is_some() || area.response.clicked_elsewhere() {
            self.palette.open = false;
            crate::app::release_text_focus(ctx);
        }
        if let Some(id) = run {
            self.palette.used(id);
            self.run_action(ctx, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_keeps_five_newest_without_duplicates() {
        let mut p = Palette::default();
        for id in ["a", "b", "c", "d", "e", "f", "c"] {
            p.used(id);
        }
        assert_eq!(p.recent, ["c", "f", "e", "d", "b"]);
    }

    #[test]
    fn items_cover_all_actions_but_the_search_itself() {
        let all = items();
        assert_eq!(all.len(), keymap::ACTIONS.len() - 1);
        assert!(!all.iter().any(|i| i.0 == "command_search"));
        assert!(all.iter().all(|i| !i.1.is_empty()));
    }

    /// Jede Aktion der Liste lässt sich über den Verteiler ausführen, ohne zu paniken.
    #[test]
    fn every_action_runs() {
        // öffnen einen Dateidialog des Systems und warten auf eine Wahl – ohne Fenster nicht testbar
        const FILE_DIALOGS: &[&str] = &["import", "open", "save_as", "import_fonts", "archive", "quick_speakers"];
        let (mut app, _home, _g) = crate::saving::tests::test_app();
        app.set_doc(cl_core::Project::new("Befehle"));
        let ctx = egui::Context::default();
        for (id, _, _) in keymap::ACTIONS.iter().filter(|a| !FILE_DIALOGS.contains(&a.0)) {
            app.run_action(&ctx, id);
            let _ = app.action_enabled(id);
        }
        assert!(app.palette.open, "„command_search“ öffnet die Suche");
    }
}
