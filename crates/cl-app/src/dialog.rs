//! One pattern for all dialogs: margin `PAD_DIALOG`, corners `R_WIN`, title on the left with a close cross,
//! footer right-aligned `[Cancel] [main action]`.
//! Question = [`modal`] (dims), tool window = [`window`] (stays open next to the work).

use eframe::egui::{self, RichText};

use crate::theme::{self, icon};

/// Shared frame for windows and modals
pub fn frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style())
        .inner_margin(theme::PAD_DIALOG)
        .corner_radius(theme::R_WIN)
}

/// Opening progress (0…1, one curve, `MOTION`) for window `id`: starts with the first frame
/// in which it is drawn (again).
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

/// Tool window without egui's title bar (title and cross come from [`title`]).
/// `id` stays the window ID (app.rs uses it to find the topmost window for Esc).
/// Opens softly: fades in and rises by 4 px.
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

/// Question (modal, with dimming); opens like [`window`], the dimming fades along.
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

/// Title row: title on the left in `FS_TITLE`, cross on the right. `true` = cross clicked.
pub fn title(ui: &mut egui::Ui, text: &str) -> bool {
    // Content fades in with the frame (see `window`/`modal`)
    let key = ui.layer_id().id.with("dialog-open");
    if let Some((since, _)) = ui.data(|d| d.get_temp::<(f64, u64)>(key)) {
        ui.multiply_opacity(opened(ui.ctx(), since));
    }
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::tr(text)).strong().size(theme::FS_TITLE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = theme::icon_button(ui, icon::X, "Close", true, false).clicked();
        });
    });
    ui.add_space(8.0);
    close
}

/// Footer, right-aligned. Add buttons from right to left (main action first).
pub fn footer<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(12.0);
    ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner)
        .inner
}

/// Standard footer `[Cancel] [main]`. Returns (main action, cancel).
pub fn buttons(ui: &mut egui::Ui, main: theme::Btn, enabled: bool) -> (bool, bool) {
    footer(ui, |ui| {
        let m = ui.add_enabled(enabled, main).clicked();
        (m, ui.add(theme::secondary("Cancel")).clicked())
    })
}

/// Esc and Enter, read before drawing. Enter does not count while a text field has focus.
/// Never use Enter for a destructive main action.
pub fn keys(ctx: &egui::Context) -> (bool, bool) {
    let typing = ctx.wants_keyboard_input();
    let (esc, enter) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
    (esc, enter && !typing)
}
