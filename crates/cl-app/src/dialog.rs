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

/// Stand des Öffnens (0…1, eine Kurve, `MOTION`) für das Fenster `id`: beginnt mit dem ersten Bild,
/// in dem es (wieder) gezeichnet wird.
fn opening(ctx: &egui::Context, id: egui::Id) -> f32 {
    let key = id.with("dialog-open");
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let (since, last) = ctx.data(|d| d.get_temp::<(f64, u64)>(key)).unwrap_or((now, pass));
    let since = if last + 1 < pass { now } else { since };
    ctx.data_mut(|d| d.insert_temp(key, (since, pass)));
    opened(ctx, since)
}

fn opened(ctx: &egui::Context, since: f64) -> f32 {
    let secs = theme::anim(theme::MOTION);
    let age = (ctx.input(|i| i.time) - since) as f32;
    if secs > 0.0 && age < secs {
        ctx.request_repaint();
        theme::ease(age / secs)
    } else {
        1.0
    }
}

/// Werkzeugfenster ohne eguis Titelleiste (Titel und Kreuz kommen aus [`title`]).
/// `id` bleibt die Fenster-ID (app.rs sucht darüber das oberste Fenster für Esc).
/// Öffnet weich: blendet ein und hebt sich um 4 px.
pub fn window(ctx: &egui::Context, title: impl Into<egui::WidgetText>, id: &str) -> egui::Window<'static> {
    let wid = egui::Id::new(id);
    let t = opening(ctx, wid);
    ctx.set_transform_layer(
        egui::LayerId::new(egui::Order::Middle, wid),
        egui::emath::TSTransform::from_translation(egui::vec2(0.0, 4.0 * (1.0 - t))),
    );
    egui::Window::new(title)
        .id(wid)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fade_in(false)
        .frame(frame(ctx).multiply_with_opacity(t))
}

/// Frage (modal, mit Abdunkelung); öffnet wie [`window`], die Abdunkelung blendet mit.
pub fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    let mid = egui::Id::new(id);
    let t = opening(ctx, mid);
    egui::Modal::new(mid)
        .area(
            egui::Modal::default_area(mid)
                .fade_in(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 4.0 * (1.0 - t))),
        )
        .backdrop_color(egui::Color32::from_black_alpha((100.0 * t) as u8))
        .frame(frame(ctx).multiply_with_opacity(t))
}

/// Titelzeile: Titel links in `FS_TITLE`, Kreuz rechts. `true` = Kreuz geklickt.
pub fn title(ui: &mut egui::Ui, text: &str) -> bool {
    // Inhalt blendet mit dem Rahmen ein (siehe `window`/`modal`)
    let key = ui.layer_id().id.with("dialog-open");
    if let Some((since, _)) = ui.data(|d| d.get_temp::<(f64, u64)>(key)) {
        ui.multiply_opacity(opened(ui.ctx(), since));
    }
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
