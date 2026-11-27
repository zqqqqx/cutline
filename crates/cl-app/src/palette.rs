//! Command search (Ctrl+K): field with a result list at the top above the editor. Searches the actions from
//! `keymap::ACTIONS` and runs them via `App::run_action` – like shortcuts and menus.

use eframe::egui::{self, Key, Modifiers};

use crate::app::App;
use crate::keymap;
use crate::theme;

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    sel: usize,
    /// commands run in this session, newest first (not saved)
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

/// Searchable commands: (action, [name in the UI language, name in the other language if different])
fn items() -> Vec<(&'static str, Vec<String>)> {
    keymap::ACTIONS
        .iter()
        .filter(|a| a.0 != "command_search")
        .map(|(id, name, _)| {
            let mut names = vec![crate::tr(name)];
            for lang in [cl_i18n::Lang::En, cl_i18n::Lang::De] {
                let other = cl_i18n::tr_in(lang, name);
                if !names.contains(&other) {
                    names.push(other);
                }
            }
            (*id, names)
        })
        .collect()
}

impl App {
    /// Draws the command search while it is open. Runs before the editor panels so that
    /// Esc, Enter and the arrows only arrive here (Esc then clears no selection).
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
                            .hint_text(crate::tr("Search commands"))
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
                                // without an icon an em quad, so the names line up
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

    /// Every action in the list can be run via the dispatcher without panicking.
    #[test]
    fn every_action_runs() {
        // open a system file dialog and wait for a choice – not testable without a window
        const FILE_DIALOGS: &[&str] = &["import", "open", "save_as", "import_fonts", "archive", "quick_speakers"];
        let mut app = crate::saving::tests::test_app();
        app.set_doc(cl_core::Project::new("Befehle"));
        let ctx = egui::Context::default();
        for (id, _, _) in keymap::ACTIONS.iter().filter(|a| !FILE_DIALOGS.contains(&a.0)) {
            app.run_action(&ctx, id);
            let _ = app.action_enabled(id);
        }
        assert!(app.palette.open, "„command_search“ opens the search");
    }
}
