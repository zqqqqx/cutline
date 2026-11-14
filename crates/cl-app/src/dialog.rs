//! Ein Muster für alle Dialoge: Rand `PAD_DIALOG`, Ecken `R_WIN`, Titel links mit Schließen-Kreuz,
//! Fußzeile rechtsbündig `[Abbrechen] [Hauptaktion]`.
//! „Frage“ = [`modal`] (dunkelt ab), „Werkzeugfenster“ = [`window`] (bleibt neben der Arbeit offen).

use eframe::egui::{self, RichText};

use crate::theme::{self, icon};

/// Gemeinsamer Rahmen für Fenster und Modals
pub fn frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style())
        .inner_margin(theme::PAD_DIALOG)
        .corner_radius(theme::R_WIN)
}

/// Werkzeugfenster ohne eguis Titelleiste (Titel und Kreuz kommen aus [`title`]).
/// `id` bleibt die Fenster-ID (app.rs sucht darüber das oberste Fenster für Esc).
pub fn window(ctx: &egui::Context, title: impl Into<egui::WidgetText>, id: &str) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame(ctx))
}

/// Frage (modal, mit Abdunkelung)
pub fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(frame(ctx))
}

/// Titelzeile: Titel links in `FS_TITLE`, Kreuz rechts. `true` = Kreuz geklickt.
pub fn title(ui: &mut egui::Ui, text: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::tr(text)).strong().size(theme::FS_TITLE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = theme::icon_button(ui, icon::X, "Schließen", true, false).clicked();
        });
    });
    ui.add_space(8.0);
    close
}

/// Fußzeile, rechtsbündig. Knöpfe von rechts nach links hinzufügen (Hauptaktion zuerst).
pub fn footer<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(12.0);
    ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner)
        .inner
}

/// Standard-Fußzeile `[Abbrechen] [main]`. Gibt (Hauptaktion, Abbrechen) zurück.
pub fn buttons(ui: &mut egui::Ui, main: theme::Btn, enabled: bool) -> (bool, bool) {
    footer(ui, |ui| {
        let m = ui.add_enabled(enabled, main).clicked();
        (m, ui.add(theme::secondary("Abbrechen")).clicked())
    })
}

/// Esc und Enter, vor dem Zeichnen gelesen. Enter zählt nicht, solange ein Textfeld den Fokus hat.
/// Enter nie für eine zerstörende Hauptaktion verwenden.
pub fn keys(ctx: &egui::Context) -> (bool, bool) {
    let typing = ctx.wants_keyboard_input();
    let (esc, enter) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
    (esc, enter && !typing)
}
