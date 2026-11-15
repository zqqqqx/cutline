//! Einheitliche Verwaltung aller Asset-Bereiche (Sticker, Soundeffekte, Musik, Effekte, Filter,
//! Übergänge, Textvorlagen, Texteffekte, Schriften):
//! - Suche, „Alle“, „Favoriten“ und Kategorien bzw. Ordner als Chips
//! - Kategorien/Ordner: verschieben, umbenennen, verbergen, eigene entfernen
//! - eigene Sammelordner in jedem Bereich, Einträge hinzufügen/entfernen
//! - Einträge: Favorit, in Ordner legen, verbergen (eingebaut) bzw. entfernen (eigene Datei)
//! - Sticker und Sounds zusätzlich mit eigenen Dateien (Import per Knopf oder Ablegen)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::library::{Item, Kind};
use crate::theme::{self, icon};
use crate::timeline::menu_item;

/// Bereich mit eigenen Dateien?
pub fn area_kind(area: &str) -> Option<Kind> {
    match area {
        "sticker" => Some(Kind::Sticker),
        "sfx" => Some(Kind::Sound),
        "libmedia" => Some(Kind::Media),
        _ => None,
    }
}

/// Symbol für eine eingebaute Kategorie (Filter-Chips wie in CapCut)
pub fn cat_icon(name: &str) -> Option<&'static str> {
    Some(match name {
        "Video" => icon::FILM_STRIP,
        "Audio" => icon::MUSIC_NOTES,
        "Bilder" => icon::IMAGE,
        "Gaming" => icon::GAME_CONTROLLER,
        "Comedy" | "Reaktionen" => icon::SMILEY,
        "Übergang" => icon::ARROWS_LEFT_RIGHT,
        "Oberfläche" => icon::CURSOR_CLICK,
        "Spannung" => icon::HEARTBEAT,
        "Natur" => icon::TREE,
        "Formen" => icon::SHAPES,
        "Pfeile" => icon::ARROW_RIGHT,
        "Sprechblasen" => icon::CHAT_CIRCLE,
        "Deko" => icon::SPARKLE,
        "Creator" => icon::VIDEO_CAMERA,
        "Titel" => icon::TEXT_H,
        "Untertitel" => icon::SUBTITLES,
        "Social" => icon::SHARE_NETWORK,
        "Hinweis" => icon::INFO,
        "Basis" => icon::SQUARES_FOUR,
        "Retro" => icon::FILM_REEL,
        "Bewegung" => icon::LIGHTNING,
        "Licht" => icon::SUN,
        "Stil" => icon::PAINT_BRUSH,
        "Porträt" => icon::PERSON,
        "Landschaft" => icon::MOUNTAINS,
        "Film" => icon::FILM_SLATE,
        "Schwarzweiß" => icon::CIRCLE_HALF,
        "Stimmung" => icon::PALETTE,
        "Wischen" => icon::HAND_SWIPE_RIGHT,
        "Form" => icon::SHAPES,
        "Effekt" => icon::SPARKLE,
        "Aufrufe" => icon::BELL_RINGING,
        "Zeigen" => icon::HAND_POINTING,
        "Zeit" => icon::TIMER,
        "Partikel" => icon::CONFETTI,
        "Rahmen" => icon::FRAME_CORNERS,
        "Symbole" => icon::STAR_FOUR,
        "Tiere & Essen" => icon::PAW_PRINT,
        _ => return None,
    })
}

/// Offener Dialog der Bibliothek.
#[derive(Clone)]
pub enum LibDialog {
    /// Neuer Ordner (Dateiordner bei Sticker/Sounds, sonst Sammelordner); optional gleich mit Eintrag
    NewFolder {
        area: &'static str,
        name: String,
        add: Option<String>,
    },
    /// Eigene Datei umbenennen
    Rename { item: Item, name: String },
    /// Kategorie/Ordner umbenennen
    RenameCat {
        area: &'static str,
        key: String,
        name: String,
    },
    /// Eigenen Ordner entfernen
    DeleteCat {
        area: &'static str,
        key: String,
        label: String,
    },
    /// Mediendatei in einen neuen Ordner der Elemente bzw. Sounds kopieren
    SaveAs {
        kind: Kind,
        file: PathBuf,
        name: String,
    },
}

/// Untermenü „Als Element/Sound speichern ▸ eigene Ordner · Neuer Ordner …“ (nur für passende Dateien).
/// Gewählt: `Some(Some(ordner))`, neuer Ordner: `Some(None)`.
pub fn save_as_menu(ui: &mut egui::Ui, kind: Option<Kind>, folders: &[String], pick: &mut Option<Option<String>>) {
    let Some(k) = kind else { return };
    let (g, label) = match k {
        Kind::Sound => (icon::SPEAKER_HIGH, "Als Sound speichern"),
        _ => (icon::STICKER, "Als Element speichern"),
    };
    theme::submenu(ui, g, label, |ui| {
        ui.set_min_width(160.0);
        // verknüpfte Ordner gehören nicht Cutline: dort nichts ablegen
        for f in folders.iter().filter(|f| !f.starts_with(icon::LINK_SIMPLE)) {
            // Ordnernamen sind Nutzertext: nicht übersetzen
            if ui.add(theme::menu_row(format!("{}   {f}", icon::FOLDER_SIMPLE)).min_width(160.0)).clicked() {
                *pick = Some(Some(f.clone()));
                ui.close_menu();
            }
        }
        if menu_item(ui, icon::FOLDER_PLUS, "Neuer Ordner …", "") {
            *pick = Some(None);
        }
    });
}

/// Lädt Vorschaubilder eigener Sticker im Hintergrund.
pub struct Thumbs {
    pub tex: HashMap<String, egui::TextureHandle>,
    requested: std::collections::HashSet<String>,
    tx: Sender<(String, u32, u32, Vec<u8>)>,
    rx: Receiver<(String, u32, u32, Vec<u8>)>,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            tex: HashMap::new(),
            requested: Default::default(),
            tx,
            rx,
        }
    }
}

fn load_rgba(p: &std::path::Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        let svg = std::fs::read_to_string(p).ok()?;
        let px = cl_fx::stickers::render_svg(&svg, size)?;
        return Some((size, size, px));
    }
    let img = match anim_still(p) {
        Some(b) => image::DynamicImage::from(b),
        None => image::open(p).ok()?,
    };
    let img = img.thumbnail(size, size).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

/// Bewegtes Bild: das Einzelbild mit der größten deckenden Fläche (bei einem Auftritt wäre das
/// erste Bild leer).
fn anim_still(p: &std::path::Path) -> Option<image::RgbaImage> {
    use image::AnimationDecoder;
    cl_media::animation(p)?;
    let r = image::ImageReader::open(p).ok()?.with_guessed_format().ok()?;
    let format = r.format()?;
    let f = r.into_inner();
    let frames = match format {
        image::ImageFormat::Gif => image::codecs::gif::GifDecoder::new(f).ok()?.into_frames(),
        image::ImageFormat::Png => image::codecs::png::PngDecoder::new(f).ok()?.apng().ok()?.into_frames(),
        _ => return None,
    };
    frames
        .filter_map(Result::ok)
        .map(|f| f.into_buffer())
        .max_by_key(|b| b.pixels().map(|p| p[3] as u64).sum::<u64>())
}

impl Thumbs {
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &std::path::Path,
    ) -> Option<egui::TextureId> {
        while let Ok((k, w, h, px)) = self.rx.try_recv() {
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            let t = ctx.load_texture(format!("lib-{k}"), img, egui::TextureOptions::LINEAR);
            self.tex.insert(k, t);
        }
        if let Some(t) = self.tex.get(key) {
            return Some(t.id());
        }
        if self.requested.insert(key.to_string()) {
            let (tx, k, p, ctx) = (
                self.tx.clone(),
                key.to_string(),
                path.to_path_buf(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                if let Some((w, h, px)) = load_rgba(&p, 128) {
                    let _ = tx.send((k, w, h, px));
                    ctx.request_repaint();
                }
            });
        }
        None
    }
}

/// Dauerhafte `&'static str` für Schlüssel, die zur Laufzeit gebildet werden (einmal je Text).
pub fn intern(s: String) -> &'static str {
    static MAP: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, &'static str>>,
    > = std::sync::OnceLock::new();
    let mut m = MAP.get_or_init(Default::default).lock().unwrap();
    if let Some(v) = m.get(&s) {
        return v;
    }
    let v: &'static str = Box::leak(s.clone().into_boxed_str());
    m.insert(s, v);
    v
}

/// Kleines Zeichen auf einer Kachel (Stern, Nadel) um `c`, auf runder dunkler Unterlage: auf hellen
/// Bildern und Wellenformen sonst kaum zu sehen.
pub fn badge(p: &egui::Painter, c: egui::Pos2, glyph: &str) {
    p.circle_filled(c, 8.0, egui::Color32::from_black_alpha(120));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(10.0), // Icon-Glyphe im 16-px-Kreis
        theme::ACCENT(),
    );
}

/// Stern oben links auf einer Kachel
pub fn fav_badge(p: &egui::Painter, a: egui::Rect) {
    badge(p, a.left_top() + egui::vec2(11.0, 10.0), icon::STAR);
}

/// Nadel oben rechts auf einer Kachel (oben angepinnt)
pub fn pin_badge(p: &egui::Painter, a: egui::Rect) {
    badge(p, a.right_top() + egui::vec2(-11.0, 10.0), icon::PUSH_PIN);
}

/// Bearbeiten-Modus der Reihenfolge: (Bereich, Ordner, Durchgang der letzten Leiste). Einzige Stelle des Zustands.
fn edit_id() -> egui::Id {
    egui::Id::new("order-edit")
}

/// Beendet Esc den Bearbeiten-Modus? Nur wenn er an ist (Leiste im letzten Bild gezeichnet, Durchgang
/// `drawn`) und kein Textfeld die Taste will; dann gehört Esc allein ihm.
fn esc_ends_edit(drawn: Option<u64>, pass: u64, esc: bool, typing: bool) -> bool {
    esc && !typing && drawn.is_some_and(|p| p + 1 >= pass)
}

/// Vor den Tastenkürzeln aufrufen: Esc beendet den Bearbeiten-Modus der Reihenfolge und wird dabei
/// verbraucht (hebt also nicht zusätzlich die Auswahl auf). `true` = so geschehen.
pub fn esc_order_edit(ctx: &egui::Context) -> bool {
    let drawn = ctx.data(|d| d.get_temp::<(String, String, u64)>(edit_id())).map(|s| s.2);
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    // offenes Kontextmenü: Esc schließt erst das Menü
    let busy = ctx.wants_keyboard_input() || ctx.is_context_menu_open();
    if !esc_ends_edit(drawn, ctx.cumulative_pass_nr(), esc, busy) {
        return false;
    }
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    ctx.data_mut(|d| d.remove::<(String, String, u64)>(edit_id()));
    true
}

/// Ist der Bearbeiten-Modus für Bereich und Ordner an?
fn editing(ctx: &egui::Context, area: &str, cat: &str) -> bool {
    ctx.data(|d| d.get_temp::<(String, String, u64)>(edit_id()))
        .is_some_and(|(a, c, _)| a == area && c == cat)
}

use crate::widgets::ReorderPayload as Reordering;

/// Umsortieren eines Tabs: Pins zeichnen und im Bearbeiten-Modus Ziehen/Ablegen der Kacheln.
pub struct Reorder {
    on: bool,
    area: &'static str,
    cat: String,
    /// eigene Reihenfolge: alles verschiebbar, sonst nur `pins`
    whole: bool,
    pins: Vec<String>,
    /// angezeigte Schlüssel in Reihenfolge (nur im Modus)
    shown: Vec<String>,
    drop: std::cell::RefCell<Option<(String, Option<String>)>>,
    over: std::cell::Cell<bool>,
    last: std::cell::Cell<Option<egui::Rect>>,
}

impl Reorder {
    pub fn pinned(&self, key: &str) -> bool {
        self.pins.iter().any(|k| k == key)
    }

    /// Nach `widgets::tile` (bzw. einer Zeile) aufrufen: zeichnet die Nadel angepinnter Einträge (`badge`)
    /// und übernimmt im Bearbeiten-Modus das Ziehen. `true` = Modus an: dann keine Zeitleisten-Nutzlast
    /// setzen und Klicks nicht auswerten.
    pub fn tile(&self, ui: &egui::Ui, resp: &egui::Response, key: &str, badge: bool) -> bool {
        let pinned = self.pinned(key);
        if badge && pinned {
            pin_badge(ui.painter(), resp.rect);
        }
        if !self.on {
            return false;
        }
        let p = ui.painter();
        // zurückgenommen: Panel-Hintergrund halbdeckend darüber (samt Namen), wirkt in jedem Design
        let fade = theme::BG_CARD().gamma_multiply(0.7);
        if !(self.whole || pinned) {
            p.rect_filled(resp.rect, theme::R_CARD, fade);
            return true;
        }
        self.last.set(Some(resp.rect));
        resp.dnd_set_drag_payload(Reordering(key.to_string()));
        let held = egui::DragAndDrop::payload::<Reordering>(ui.ctx());
        if held.as_ref().is_some_and(|h| h.0 == key) {
            p.rect_filled(resp.rect, theme::R_CARD, fade);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        // Raster mit Lücke: Ziel kommt vom Raster (`reorder_done`), keine Einfügelinie
        let gap = ui.data(|d| d.get_temp::<crate::widgets::GridDrop>(crate::widgets::grid_drop_id())).is_some();
        if let Some(h) = resp.dnd_hover_payload::<Reordering>() {
            self.over.set(true);
            if h.0 != key && !gap {
                insert_mark(p, resp.rect, false);
            }
        }
        if let Some(h) = resp.dnd_release_payload::<Reordering>().filter(|_| !gap) {
            *self.drop.borrow_mut() = Some((h.0.clone(), Some(key.to_string())));
        }
        true
    }
}

/// Einfügemarke vor (`after` = hinter) einer Kachel; bei breiten Zeilen waagrecht
fn insert_mark(p: &egui::Painter, r: egui::Rect, after: bool) {
    let s = egui::Stroke::new(2.0_f32, theme::ACCENT());
    if r.width() > 2.0 * r.height() {
        let y = if after { r.bottom() + 2.0 } else { r.top() - 2.0 };
        p.hline(r.x_range(), y, s);
    } else {
        let x = if after { r.right() + 4.0 } else { r.left() - 4.0 };
        p.vline(x, r.y_range(), s);
    }
}

impl App {
    /// Datei als Element bzw. Sound in den Ordner kopieren (`None`: Namen erfragen, Ordner anlegen).
    pub fn save_as(&mut self, kind: Kind, folder: Option<String>, file: PathBuf) {
        let Some(f) = folder else {
            self.lib_dialog = Some(LibDialog::SaveAs { kind, file, name: String::new() });
            return;
        };
        if self.library.import(kind, &f, std::slice::from_ref(&file)) == 0 {
            self.toast_err("Datei nicht gefunden");
        } else {
            self.toast(if kind == Kind::Sound { "Als Sound gespeichert" } else { "Als Element gespeichert" });
        }
    }

    /// Liste ordnen (eigene Reihenfolge bzw. Pins zuerst, nicht bei Suche) und das Umsortieren holen.
    pub fn lib_arrange<T>(
        &self,
        ui: &egui::Ui,
        area: &'static str,
        cat: &str,
        q: &str,
        items: &mut Vec<T>,
        key: impl Fn(&T) -> String,
    ) -> Reorder {
        self.library.arrange(area, cat, q, items, &key);
        self.reorder(ui, area, cat, items, key)
    }

    /// Umsortieren für eine schon geordnete Liste (siehe [`Self::lib_arrange`]).
    pub fn reorder<T>(&self, ui: &egui::Ui, area: &'static str, cat: &str, items: &[T], key: impl Fn(&T) -> String) -> Reorder {
        let on = editing(ui.ctx(), area, cat);
        if on {
            ui.data_mut(|d| d.insert_temp(crate::widgets::no_plus_id(), true));
        }
        let kind = crate::library::order_kind(cat);
        let pins = if kind == crate::library::Order::Pins { self.library.order(area, cat).to_vec() } else { Vec::new() };
        // Schlüssel fürs Raster: Kacheln gleiten an ihren neuen Platz (Ablegen, Anpinnen)
        let shown: Vec<String> = if kind == crate::library::Order::Fixed { Vec::new() } else { items.iter().map(key).collect() };
        ui.data_mut(|d| d.remove::<crate::widgets::GridDrop>(crate::widgets::grid_drop_id()));
        if !shown.is_empty() {
            let whole = kind == crate::library::Order::Own;
            let movable = if whole { shown.len() } else { shown.iter().take_while(|k| pins.contains(k)).count() };
            let order = crate::widgets::GridOrder { keys: std::sync::Arc::new(shown.clone()), movable, editing: on };
            ui.data_mut(|d| d.insert_temp(crate::widgets::grid_order_id(), order));
        }
        Reorder {
            on,
            area,
            cat: cat.to_string(),
            whole: kind == crate::library::Order::Own,
            pins,
            shown,
            drop: Default::default(),
            over: Default::default(),
            last: Default::default(),
        }
    }

    /// Nach dem Raster: Ablegen übernehmen. Abgelegt in `list` außerhalb einer verschiebbaren Kachel = ans Ende.
    pub fn reorder_done(&mut self, ui: &egui::Ui, ed: Reorder, list: egui::Rect) {
        ui.data_mut(|d| d.remove::<crate::widgets::GridOrder>(crate::widgets::grid_order_id()));
        let gap = ui.data_mut(|d| d.remove_temp::<crate::widgets::GridDrop>(crate::widgets::grid_drop_id()));
        if !ed.on {
            return;
        }
        ui.data_mut(|d| d.remove::<bool>(crate::widgets::no_plus_id()));
        let inside = ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| list.contains(p));
        if let Some(g) = gap {
            // Raster mit Lücke: Ablegen irgendwo im Raster landet an der gezeigten Lücke
            if ui.input(|i| i.pointer.any_released()) && egui::DragAndDrop::take_payload::<Reordering>(ui.ctx()).is_some() {
                *ed.drop.borrow_mut() = Some((g.0, g.1));
            }
        } else if inside && !ed.over.get() && egui::DragAndDrop::has_payload_of_type::<Reordering>(ui.ctx()) {
            if let Some(r) = ed.last.get() {
                insert_mark(&ui.painter().with_clip_rect(list), r, true);
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(h) = egui::DragAndDrop::take_payload::<Reordering>(ui.ctx()) {
                    *ed.drop.borrow_mut() = Some((h.0.clone(), None));
                }
            }
        }
        if let Some((key, before)) = ed.drop.into_inner() {
            self.library.move_entry(ed.area, &ed.cat, &ed.shown, &key, before.as_deref());
        }
    }

    /// Kopfbereich eines Asset-Bereichs: Suche, (Import), neuer Ordner, Chips mit Menüs.
    /// Gibt (Kategorie-Schlüssel, Suchtext) zurück; Kategorie ist „Alle“, „Favoriten“ oder `b:/p:/v:…`.
    pub fn asset_bar(
        &mut self,
        ui: &mut egui::Ui,
        area: &'static str,
        builtin: &[&str],
    ) -> (String, String) {
        let phys = area_kind(area);
        let qkey: &'static str = intern(format!("{area}-q"));
        let ckey: &'static str = intern(format!("{area}-cat"));
        let mut q = self.asset_cat.get(qkey).cloned().unwrap_or_default();
        // Reihenfolge bearbeiten: eigene Reihenfolge immer, sonst nur mit Angepinnten; nie bei Suche
        let sel = self.asset_cat.get(ckey).cloned().unwrap_or_else(|| "Alle".into());
        let can_edit = q.trim().is_empty()
            && match crate::library::order_kind(&sel) {
                crate::library::Order::Own => true,
                crate::library::Order::Pins => !self.library.order(area, &sel).is_empty(),
                crate::library::Order::Fixed => false,
            };
        let pass = ui.ctx().cumulative_pass_nr();
        let state = ui.data(|d| d.get_temp::<(String, String, u64)>(edit_id()));
        // Modus endet bei Ordner-/Tab-Wechsel (auch wenn die Leiste ein Bild lang nicht gezeichnet wurde)
        let mut edit = can_edit
            && state.as_ref().is_some_and(|(a, c, p)| a == area && *c == sel && p + 1 >= pass)
            && !ui.input(|i| i.key_pressed(egui::Key::Escape));
        ui.data_mut(|d| d.remove::<bool>(crate::widgets::no_plus_id()));
        ui.horizontal(|ui| {
            let buttons = if phys.is_some() { 2.0 } else { 1.0 } + if can_edit { 1.0 } else { 0.0 };
            let w = (ui.available_width() - buttons * 30.0 - buttons * ui.spacing().item_spacing.x)
                .max(60.0);
            ui.add(
                egui::TextEdit::singleline(&mut q)
                    .hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS)))
                    .desired_width(w),
            );
            if let Some(k) = phys {
                let tip = match k {
                    Kind::Sticker => "Eigene Sticker oder Bilder hinzufügen (auch per Ablegen)",
                    Kind::Sound => "Eigene Sounds hinzufügen (auch per Ablegen)",
                    Kind::Media => "Dateien zur Bibliothek hinzufügen (auch per Ablegen)",
                };
                if theme::icon_button(ui, icon::UPLOAD_SIMPLE, tip, true, false).clicked() {
                    self.lib_pick(k);
                }
            }
            if theme::icon_button(ui, icon::FOLDER_PLUS, "Neuer Ordner", true, false).clicked() {
                self.lib_dialog = Some(LibDialog::NewFolder {
                    area,
                    name: String::new(),
                    add: None,
                });
            }
            if can_edit {
                let (g, tip) = if edit { (icon::CHECK, "Fertig") } else { (icon::PENCIL_SIMPLE, "Reihenfolge bearbeiten") };
                if theme::icon_button(ui, g, tip, true, edit).clicked() {
                    edit = !edit;
                }
            }
        });
        if edit {
            ui.data_mut(|d| d.insert_temp(edit_id(), (area.to_string(), sel.clone(), pass)));
        } else if state.is_some() {
            ui.data_mut(|d| d.remove::<(String, String, u64)>(edit_id()));
        }
        self.asset_cat.insert(qkey, q.clone());
        ui.add_space(4.0);
        let cats = self.library.cats(area, builtin, phys);
        let mut cur = self
            .asset_cat
            .get(ckey)
            .cloned()
            .unwrap_or_else(|| "Alle".into());
        // Kategorie verschwunden (verborgen/gelöscht/umbenannt)? Zurück auf „Alle“
        if cur != "Alle" && cur != "Favoriten" && cur != "Zuletzt" && !cats.iter().any(|c| c.key == cur) {
            cur = "Alle".into();
            self.asset_cat.insert(ckey, cur.clone());
        }
        let mut act: Option<(u8, String)> = None;
        let n = cats.len();
        // Eine Zeile, waagrecht scrollbar (Mausrad oder Ziehen) statt mehrzeiligem Umbruch
        let mut chips: Vec<egui::Rect> = Vec::new();
        let mut row = egui::ScrollArea::horizontal()
            .id_salt(ckey)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            let marks = ui.painter().add(egui::Shape::Noop);
            let mut active = None;
            if ui.rect_contains_pointer(ui.clip_rect()) {
                let d = ui.input(|i| i.smooth_scroll_delta);
                if d.x == 0.0 && d.y != 0.0 {
                    ui.scroll_with_delta(egui::vec2(d.y, 0.0));
                }
            }
            for (key, label, tip) in [
                ("Alle", "Alle", ""),
                ("Favoriten", icon::STAR, "Favoriten"),
                ("Zuletzt", icon::CLOCK_COUNTER_CLOCKWISE, "Zuletzt verwendet"),
            ] {
                if cur == key {
                    active = Some(chips.len());
                }
                let mut r = theme::strip_chip(ui, label, cur == key);
                if !tip.is_empty() {
                    r = r.on_hover_text(crate::tr(tip));
                }
                chips.push(r.rect);
                if r.clicked() {
                    self.asset_cat.insert(ckey, key.into());
                }
            }
            for (i, c) in cats.iter().enumerate() {
                let label = if c.builtin {
                    match cat_icon(c.key.trim_start_matches("b:")) {
                        Some(g) => format!("{g} {}", c.label),
                        None => c.label.clone(),
                    }
                } else {
                    format!("{}  {}", icon::FOLDER_SIMPLE, c.label)
                };
                if cur == c.key {
                    active = Some(chips.len());
                }
                let r = theme::strip_chip(ui, &label, cur == c.key);
                chips.push(r.rect);
                if r.clicked() {
                    self.asset_cat.insert(ckey, c.key.clone());
                }
                r.context_menu(|ui| {
                    ui.set_min_width(170.0);
                    if menu_item(ui, icon::PENCIL_SIMPLE, "Umbenennen", "") {
                        act = Some((0, c.key.clone()));
                    }
                    if i > 0 && menu_item(ui, icon::ARROW_LEFT, "Nach vorne", "") {
                        act = Some((1, c.key.clone()));
                    }
                    if i + 1 < n && menu_item(ui, icon::ARROW_RIGHT, "Nach hinten", "") {
                        act = Some((2, c.key.clone()));
                    }
                    if menu_item(ui, icon::EYE_SLASH, "Verbergen", "") {
                        act = Some((3, c.key.clone()));
                    }
                    if !c.builtin {
                        ui.separator();
                        if ui
                            .button(
                                RichText::new(crate::tr(format!(
                                    "{}  Ordner entfernen",
                                    icon::TRASH
                                )))
                                .color(theme::WARN_TEXT()),
                            )
                            .clicked()
                        {
                            act = Some((4, c.key.clone()));
                            ui.close_menu();
                        }
                    }
                });
            }
            theme::chip_marks(ui, marks, egui::Id::new(("chips", ckey)), &chips, active);
            // kleiner Rand, damit der letzte Eintrag nicht an der Kante klebt
            ui.add_space(2.0);
        }));
        theme::edge_arrows(ui, &mut row, &chips);
        if let Some((a, key)) = act {
            let label = cats
                .iter()
                .find(|c| c.key == key)
                .map(|c| c.label.clone())
                .unwrap_or_default();
            match a {
                0 => {
                    self.lib_dialog = Some(LibDialog::RenameCat {
                        area,
                        key,
                        name: label,
                    })
                }
                1 => self.library.move_cat(area, &key, -1, builtin, phys),
                2 => self.library.move_cat(area, &key, 1, builtin, phys),
                3 => self.library.hide_cat(area, &key),
                _ => self.lib_dialog = Some(LibDialog::DeleteCat { area, key, label }),
            }
        }
        // Verborgenes wieder anzeigen
        let hidden_cats = self.library.hidden_cats(area);
        let packs: Vec<String> = match area {
            "sfx" => Some(cl_fx::catalog::Kind::Sound),
            "sticker" => Some(cl_fx::catalog::Kind::Sticker),
            _ => None,
        }
        .map(|k| cl_fx::catalog::list(k).into_iter().map(|e| e.id).collect())
        .unwrap_or_default();
        let hidden_items = self.library.hidden_count(|k| restorable(area, k, &packs));
        if hidden_cats + hidden_items > 0
            && ui
                .add(theme::ghost(format!(
                    "{}  {} verborgen · wieder anzeigen",
                    icon::EYE,
                    hidden_cats + hidden_items
                )))
                .clicked()
        {
            self.library.unhide_cats(area);
            self.library.unhide_all(|k| restorable(area, k, &packs));
        }
        ui.add_space(4.0);
        let cat = self
            .asset_cat
            .get(ckey)
            .cloned()
            .unwrap_or_else(|| "Alle".into());
        (cat, q)
    }

    /// Kontextmenü eines Eintrags (eingebaut oder eigene Datei).
    /// `key` = eindeutiger Schlüssel des Eintrags, `cat` = aktuell gewählte Kategorie.
    pub fn asset_menu(
        &mut self,
        resp: &egui::Response,
        area: &'static str,
        key: &str,
        own: Option<&Item>,
        cat: &str,
    ) {
        self.asset_menu_with(resp, area, key, own, cat, &mut |_| {});
    }

    /// Wie `asset_menu`, mit eigenen Einträgen oben im Menü.
    pub fn asset_menu_with(
        &mut self,
        resp: &egui::Response,
        area: &'static str,
        key: &str,
        own: Option<&Item>,
        cat: &str,
        extra: &mut dyn FnMut(&mut egui::Ui),
    ) {
        let fav = self.library.is_favorite(key);
        let pin = (crate::library::order_kind(cat) == crate::library::Order::Pins)
            .then(|| self.library.is_pinned(area, cat, key));
        let virt = self.library.virtual_folders(area);
        let phys_folders = area_kind(area)
            .map(|k| self.library.folders(k))
            .unwrap_or_default();
        let mut act: Option<u8> = None;
        let mut add_to: Option<String> = None;
        let mut move_to: Option<String> = None;
        resp.context_menu(|ui| {
            ui.set_min_width(200.0);
            extra(ui);
            if menu_item(
                ui,
                icon::STAR,
                if fav {
                    "Aus Favoriten entfernen"
                } else {
                    "Zu Favoriten"
                },
                "",
            ) {
                act = Some(0);
            }
            if let Some(p) = pin {
                let (g, t) = if p { (icon::PUSH_PIN_SLASH, "Nicht mehr anpinnen") } else { (icon::PUSH_PIN, "Oben anpinnen") };
                if menu_item(ui, g, t, "") {
                    act = Some(5);
                }
            }
            theme::submenu(
                ui,
                icon::FOLDER_SIMPLE_PLUS,
                "Zu Ordner hinzufügen",
                |ui| {
                    ui.set_min_width(160.0);
                    for f in &virt {
                        // Ordnernamen sind Nutzertext: nicht übersetzen
                        if ui.add(theme::menu_row(format!("{}   {f}", icon::FOLDER_SIMPLE)).min_width(160.0)).clicked() {
                            add_to = Some(f.clone());
                            ui.close_menu();
                        }
                    }
                    if menu_item(ui, icon::FOLDER_PLUS, "Neuer Ordner …", "") {
                        act = Some(3);
                    }
                },
            );
            if let Some(name) = cat.strip_prefix("v:") {
                if menu_item(ui, icon::FOLDER_MINUS, &format!("Aus „{name}“ entfernen"), "") {
                    act = Some(4);
                }
            }
            if let Some(item) = own {
                theme::submenu(
                    ui,
                    icon::FOLDER_SIMPLE,
                    "Verschieben nach",
                    |ui| {
                        ui.set_min_width(150.0);
                        for f in phys_folders.iter().filter(|f| **f != item.folder) {
                            if ui.add(theme::menu_row(format!("{}   {f}", icon::FOLDER_SIMPLE)).min_width(150.0)).clicked() {
                                move_to = Some(f.clone());
                                ui.close_menu();
                            }
                        }
                    },
                );
                if menu_item(ui, icon::PENCIL_SIMPLE, "Umbenennen", "") {
                    act = Some(1);
                }
                if menu_item(ui, icon::FOLDER_OPEN, "Im Explorer zeigen", "") {
                    if let Some(p) = &item.path {
                        crate::home::reveal(p);
                    }
                }
            }
            ui.separator();
            let label = if own.is_some() {
                "Entfernen"
            } else {
                "Verbergen"
            };
            if ui
                .button(
                    RichText::new(crate::tr(format!("{}  {label}", icon::TRASH)))
                        .color(theme::WARN_TEXT()),
                )
                .clicked()
            {
                act = Some(2);
                ui.close_menu();
            }
        });
        match act {
            Some(0) => self.library.toggle_favorite(key),
            Some(1) => {
                if let Some(item) = own {
                    self.lib_dialog = Some(LibDialog::Rename {
                        item: item.clone(),
                        name: item.name.clone(),
                    });
                }
            }
            Some(2) => match own {
                Some(item)
                    if item
                        .path
                        .as_ref()
                        .is_some_and(|p| self.project.media.iter().any(|m| &m.path == p)) =>
                {
                    self.toast_err(format!(
                        "„{}“ wird im offenen Projekt verwendet und bleibt deshalb erhalten",
                        item.name
                    ));
                }
                Some(item) => match self.library.remove(item) {
                    Ok(()) => self.toast(format!("„{}“ in den Papierkorb verschoben", item.name)),
                    Err(e) => self.toast_err(e),
                },
                None => self.library.hide(key),
            },
            Some(3) => {
                self.lib_dialog = Some(LibDialog::NewFolder {
                    area,
                    name: String::new(),
                    add: Some(key.to_string()),
                })
            }
            Some(4) => {
                if let Some(name) = cat.strip_prefix("v:") {
                    self.library.remove_from_virtual(area, name, key);
                }
            }
            Some(5) => self.library.toggle_pin(area, cat, key),
            _ => {}
        }
        if let Some(f) = add_to {
            self.library.add_to_virtual(area, &f, key);
            self.toast(format!("Zu „{f}“ hinzugefügt"));
        }
        if let (Some(f), Some(item)) = (move_to, own) {
            if let Some(k) = area_kind(area) {
                if let Err(e) = self.library.move_to(item, k, &f) {
                    self.toast_err(e);
                }
            }
        }
    }

    /// Dateiauswahl für eigene Sticker/Sounds; Ziel ist der gewählte Ordner (sonst „Eigene“).
    pub fn lib_pick(&mut self, kind: Kind) {
        let label = match kind {
            Kind::Sticker => "Bilder",
            Kind::Sound => "Audio",
            Kind::Media => "Medien",
        };
        if let Some(files) = crate::dialog()
            .add_filter(crate::tr(label), kind.exts())
            .pick_files()
        {
            self.lib_import(kind, files);
        }
    }

    pub fn lib_import(&mut self, kind: Kind, files: Vec<PathBuf>) {
        let ckey = match kind {
            Kind::Sticker => "sticker-cat",
            Kind::Sound => "sfx-cat",
            Kind::Media => "libmedia-cat",
        };
        let cur = self.asset_cat.get(ckey).cloned().unwrap_or_default();
        let folder = cur
            .strip_prefix("p:")
            .map(String::from)
            .unwrap_or_else(|| Kind::DEFAULT_FOLDER.to_string());
        let n = self.library.import(kind, &folder, &files);
        if n == 0 {
            self.toast_err("Keine passenden Dateien");
        } else {
            self.asset_cat.insert(ckey, format!("p:{folder}"));
            self.toast(format!("{n} in „{folder}“ hinzugefügt"));
        }
    }

    /// Dialoge der Bibliothek (jeden Frame aufrufen).
    pub fn lib_dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.lib_dialog else {
            self.lib_dialog_since = 0;
            return;
        };
        // Der Klick, der den Dialog öffnet, darf ihn nicht gleich als Klick daneben schließen
        let pass = ctx.cumulative_pass_nr();
        if self.lib_dialog_since == 0 {
            self.lib_dialog_since = pass;
        }
        let fresh = pass <= self.lib_dialog_since + 1;
        let mut close = false;
        let mut ok = false;
        let err = self.dialog_error.clone();
        let mut edited = false;
        let title = match d {
            LibDialog::NewFolder { .. } | LibDialog::SaveAs { .. } => "Neuer Ordner",
            LibDialog::Rename { .. } | LibDialog::RenameCat { .. } => "Umbenennen",
            LibDialog::DeleteCat { .. } => "Ordner entfernen?",
        };
        let modal = crate::dialog::modal(ctx, "lib-dialog").show(ctx, |ui| {
            ui.set_width(320.0);
            close |= crate::dialog::title(ui, title);
            match d {
                LibDialog::NewFolder { name, .. }
                | LibDialog::Rename { name, .. }
                | LibDialog::RenameCat { name, .. }
                | LibDialog::SaveAs { name, .. } => {
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if fresh {
                        r.request_focus();
                    }
                    edited |= r.changed();
                    theme::field_error(ui, err.as_deref());
                    if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                }
                LibDialog::DeleteCat { area, key, label } => {
                    let text = if key.starts_with("p:") && area_kind(area).is_some() {
                        format!("„{label}“ und sein Inhalt werden in den Cutline-Papierkorb verschoben.")
                    } else {
                        format!("Der Ordner „{label}“ wird entfernt. Die Einträge darin bleiben erhalten.")
                    };
                    ui.label(crate::tr(text));
                    theme::field_error(ui, err.as_deref());
                }
            }
            let main = if matches!(d, LibDialog::DeleteCat { .. }) { theme::danger("Entfernen") } else { theme::primary("OK") };
            let (m, c) = crate::dialog::buttons(ui, main, true);
            ok |= m;
            close |= c;
        });
        if modal.should_close() && !fresh {
            close = true;
        }
        if edited {
            self.dialog_error = None;
        }
        if ok {
            let backup = self.lib_dialog.clone();
            let res = match self.lib_dialog.take() {
                Some(LibDialog::NewFolder { area, name, add }) => {
                    let ckey: &'static str = intern(format!("{area}-cat"));
                    // Sticker/Sounds ohne Eintrag: echter Dateiordner, sonst Sammelordner
                    match (area_kind(area), &add) {
                        (Some(k), None) => self.library.create_folder(k, &name).map(|_| {
                            self.asset_cat.insert(ckey, format!("p:{}", name.trim()));
                        }),
                        _ => self.library.create_virtual(area, &name).map(|key| {
                            if let Some(item) = &add {
                                self.library.add_to_virtual(area, name.trim(), item);
                            } else {
                                self.asset_cat.insert(ckey, key);
                            }
                        }),
                    }
                }
                Some(LibDialog::Rename { item, name }) => self.library.rename(&item, &name),
                Some(LibDialog::RenameCat { area, key, name }) => {
                    let ckey: &'static str = intern(format!("{area}-cat"));
                    let was = self.asset_cat.get(ckey).cloned();
                    self.library
                        .rename_cat(area, &key, &name, area_kind(area))
                        .map(|_| {
                            // gewählte Kategorie folgt dem neuen Namen
                            if was.as_deref() == Some(key.as_str()) && !key.starts_with("b:") {
                                let pre = &key[..2];
                                self.asset_cat.insert(ckey, format!("{pre}{}", name.trim()));
                            }
                        })
                }
                Some(LibDialog::DeleteCat { area, key, .. }) => {
                    let used: Vec<PathBuf> = self.project.media.iter().map(|m| m.path.clone()).collect();
                    self.library.delete_cat(area, &key, area_kind(area), &used)
                }
                Some(LibDialog::SaveAs { kind, file, name }) => {
                    // Ordner anlegen und gleich hineinkopieren
                    self.library.create_folder(kind, &name).map(|_| self.save_as(kind, Some(name.trim().to_string()), file))
                }
                None => Ok(()),
            };
            match res {
                // Dialog bleibt mit der Eingabe offen, der Fehler steht darin
                Err(e) => {
                    self.lib_dialog = backup;
                    self.dialog_error = Some(e);
                }
                Ok(()) => self.dialog_error = None,
            }
        } else if close {
            self.lib_dialog = None;
            self.dialog_error = None;
        }
    }

    /// Eigener Sticker/Bild: als Bild-Clip auf eine Overlay-Spur.
    pub fn add_own_sticker_at(
        &mut self,
        path: &std::path::Path,
        at: cl_core::Time,
        track: Option<cl_core::TrackId>,
    ) -> Option<cl_core::ClipId> {
        self.library.mark_used(&format!("file:{}", path.display()));
        // SVG wird für den Clip einmal als PNG gerendert
        let file = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
        {
            // nicht im Cache (das Projekt zeigt darauf); Pfad-Kennung trennt gleichnamige SVGs
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(path, &mut h);
            let out = crate::paths::generated_dir().join("svg").join(format!(
                "{}-{:016x}.png",
                path.file_stem()?.to_string_lossy(),
                std::hash::Hasher::finish(&h)
            ));
            if !out.is_file() {
                let svg = std::fs::read_to_string(path).ok()?;
                let px = cl_fx::stickers::render_svg(&svg, 512)?;
                let _ = std::fs::create_dir_all(out.parent()?);
                // vormultipliziert → gerade
                let mut px = px;
                for c in px.chunks_mut(4) {
                    let a = c[3] as f32 / 255.0;
                    if a > 0.0 {
                        for v in &mut c[..3] {
                            *v = ((*v as f32 / a).min(255.0)) as u8;
                        }
                    }
                }
                image::save_buffer(&out, &px, 512, 512, image::ExtendedColorType::Rgba8).ok()?;
            }
            out
        } else {
            path.to_path_buf()
        };
        self.add_image_overlay_at(file, at, track)
    }

    /// Eingebaute und eigene Einträge eines Bereichs nach Kategorie und Suche gefiltert.
    pub fn lib_items(
        &mut self,
        area: &'static str,
        kind: Kind,
        builtin: Vec<Item>,
        cat: &str,
        q: &str,
    ) -> Vec<Item> {
        let mut all = builtin;
        all.extend(self.library.own_items(kind));
        let mut items: Vec<Item> = all
            .into_iter()
            .filter(|i| {
                self.library
                    .visible_tagged(area, cat, q, &i.key, &i.name, &i.folder, &i.tags)
            })
            .collect();
        self.library.arrange(area, cat, q, &mut items, |i| i.key.clone());
        items
    }
}

/// Holt „wieder anzeigen“ im Bereich `area` diesen verborgenen Eintrag zurück?
/// `packs`: IDs der Paket-Einträge dieses Bereichs (Schlüssel `pack:<id>`).
fn restorable(area: &str, key: &str, packs: &[String]) -> bool {
    key.strip_prefix(area).is_some_and(|r| r.starts_with(':'))
        || match area {
            "sticker" => key.starts_with("builtin:st:"),
            "sfx" => key.starts_with("builtin:sfx:"),
            _ => false,
        }
        || key.strip_prefix("pack:").is_some_and(|id| packs.iter().any(|p| p == id))
}

#[cfg(test)]
mod tests {
    #[test]
    fn esc_ends_the_order_edit_mode_only_while_it_is_on() {
        use super::esc_ends_edit;
        // Leiste im letzten Bild (Durchgang 9) gezeichnet, jetzt Durchgang 10
        assert!(esc_ends_edit(Some(9), 10, true, false));
        // ohne Esc, beim Tippen, ohne Modus oder mit verwaistem Zustand (Tab gewechselt): nichts
        assert!(!esc_ends_edit(Some(9), 10, false, false));
        assert!(!esc_ends_edit(Some(9), 10, true, true));
        assert!(!esc_ends_edit(None, 10, true, false));
        assert!(!esc_ends_edit(Some(3), 10, true, false));
    }

    #[test]
    fn hidden_pack_entries_restore_only_in_their_area() {
        use super::restorable;
        let sounds = vec!["cutline.basis/treffer".to_string()];
        let elems = vec!["cutline.elemente/abo".to_string()];
        assert!(restorable("sfx", "pack:cutline.basis/treffer", &sounds));
        assert!(restorable("sticker", "pack:cutline.elemente/abo", &elems));
        assert!(!restorable("sticker", "pack:cutline.basis/treffer", &elems));
        assert!(!restorable("sfx", "pack:cutline.basis/treffer2", &sounds));
        assert!(restorable("sfx", "builtin:sfx:pop", &sounds) && restorable("sfx", "sfx:x", &[]));
        assert!(!restorable("sfx", "sfxx:x", &[]) && !restorable("sticker", "builtin:sfx:pop", &[]));
    }

    #[test]
    fn animated_tile_shows_the_fullest_frame() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // Auftritt: von links wachsender Balken, erstes Bild leer
        let grow = "color=red:size=64x64:rate=10:duration=1,format=rgba,geq=r=255:g=0:b=0:a='if(lt(X,T*64),255,0)'";
        for (name, extra) in [("a.png", vec!["-plays", "1", "-f", "apng"]), ("a.gif", vec!["-vf", "split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse"])] {
            let f = dir.path().join(name);
            let st = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", grow])
                .args(extra)
                .arg(&f)
                .status()
                .unwrap();
            assert!(st.success());
            let (w, _, px) = super::load_rgba(&f, 64).unwrap();
            let opaque = (0..w as usize).filter(|x| px[(32 * w as usize + x) * 4 + 3] > 128).count();
            assert!(opaque >= 55, "{name}: {opaque}");
        }
    }
}
