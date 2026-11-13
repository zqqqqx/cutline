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

use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::library::{Item, Kind};
use crate::theme::{self, icon};

/// Bereich mit eigenen Dateien?
pub fn area_kind(area: &str) -> Option<Kind> {
    match area {
        "sticker" => Some(Kind::Sticker),
        "sfx" => Some(Kind::Sound),
        _ => None,
    }
}

/// Offener Dialog der Bibliothek.
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
    let img = image::open(p).ok()?.thumbnail(size, size).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
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

/// Stern oben links auf einer Kachel
pub fn fav_badge(p: &egui::Painter, a: egui::Rect) {
    p.text(
        a.left_top() + egui::vec2(5.0, 4.0),
        egui::Align2::LEFT_TOP,
        icon::STAR,
        egui::FontId::proportional(11.0),
        theme::ACCENT,
    );
}

impl App {
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
        ui.horizontal(|ui| {
            let buttons = if phys.is_some() { 2.0 } else { 1.0 };
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
        });
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
        egui::ScrollArea::horizontal()
            .id_salt(ckey)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            if ui.rect_contains_pointer(ui.clip_rect()) {
                let d = ui.input(|i| i.smooth_scroll_delta);
                if d.x == 0.0 && d.y != 0.0 {
                    ui.scroll_with_delta(egui::vec2(d.y, 0.0));
                }
            }
            if theme::cat_chip(ui, "Alle", cur == "Alle").clicked() {
                self.asset_cat.insert(ckey, "Alle".into());
            }
            if theme::cat_chip(
                ui,
                &format!("{} {}", icon::STAR, crate::tr("Favoriten")),
                cur == "Favoriten",
            )
            .clicked()
            {
                self.asset_cat.insert(ckey, "Favoriten".into());
            }
            if theme::cat_chip(
                ui,
                &format!("{} {}", icon::CLOCK_COUNTER_CLOCKWISE, crate::tr("Zuletzt")),
                cur == "Zuletzt",
            )
            .on_hover_text(crate::tr("Zuletzt verwendet"))
            .clicked()
            {
                self.asset_cat.insert(ckey, "Zuletzt".into());
            }
            for (i, c) in cats.iter().enumerate() {
                let label = if c.builtin {
                    c.label.clone()
                } else {
                    format!("{}  {}", icon::FOLDER_SIMPLE, c.label)
                };
                let r = theme::cat_chip(ui, &label, cur == c.key);
                if r.clicked() {
                    self.asset_cat.insert(ckey, c.key.clone());
                }
                r.context_menu(|ui| {
                    ui.set_min_width(170.0);
                    if ui
                        .button(crate::tr(format!("{}  Umbenennen", icon::PENCIL_SIMPLE)))
                        .clicked()
                    {
                        act = Some((0, c.key.clone()));
                        ui.close_menu();
                    }
                    if i > 0
                        && ui
                            .button(crate::tr(format!("{}  Nach vorne", icon::ARROW_LEFT)))
                            .clicked()
                    {
                        act = Some((1, c.key.clone()));
                        ui.close_menu();
                    }
                    if i + 1 < n
                        && ui
                            .button(crate::tr(format!("{}  Nach hinten", icon::ARROW_RIGHT)))
                            .clicked()
                    {
                        act = Some((2, c.key.clone()));
                        ui.close_menu();
                    }
                    if ui
                        .button(crate::tr(format!("{}  Verbergen", icon::EYE_SLASH)))
                        .clicked()
                    {
                        act = Some((3, c.key.clone()));
                        ui.close_menu();
                    }
                    if !c.builtin {
                        ui.separator();
                        if ui
                            .button(
                                RichText::new(crate::tr(format!(
                                    "{}  Ordner entfernen",
                                    icon::TRASH
                                )))
                                .color(Color32::from_rgb(0xE0, 0x80, 0x60)),
                            )
                            .clicked()
                        {
                            act = Some((4, c.key.clone()));
                            ui.close_menu();
                        }
                    }
                });
            }
        }));
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
        let prefix = format!("{area}:");
        let hidden_items = self.library.hidden_count(&prefix)
            + if area == "sticker" {
                self.library.hidden_count("builtin:st:")
            } else if area == "sfx" {
                self.library.hidden_count("builtin:sfx:")
            } else {
                0
            };
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
            self.library.unhide_all(&prefix);
            if area == "sticker" {
                self.library.unhide_all("builtin:st:");
            } else if area == "sfx" {
                self.library.unhide_all("builtin:sfx:");
            }
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
            if ui
                .button(crate::tr(format!(
                    "{}  {}",
                    icon::STAR,
                    if fav {
                        "Aus Favoriten entfernen"
                    } else {
                        "Zu Favoriten"
                    }
                )))
                .clicked()
            {
                act = Some(0);
                ui.close_menu();
            }
            ui.menu_button(
                crate::tr(format!(
                    "{}  Zu Ordner hinzufügen",
                    icon::FOLDER_SIMPLE_PLUS
                )),
                |ui| {
                    ui.set_min_width(160.0);
                    for f in &virt {
                        if ui.button(f).clicked() {
                            add_to = Some(f.clone());
                            ui.close_menu();
                        }
                    }
                    if ui
                        .button(crate::tr(format!("{}  Neuer Ordner …", icon::FOLDER_PLUS)))
                        .clicked()
                    {
                        act = Some(3);
                        ui.close_menu();
                    }
                },
            );
            if let Some(name) = cat.strip_prefix("v:") {
                if ui
                    .button(crate::tr(format!(
                        "{}  Aus „{name}“ entfernen",
                        icon::FOLDER_MINUS
                    )))
                    .clicked()
                {
                    act = Some(4);
                    ui.close_menu();
                }
            }
            if let Some(item) = own {
                ui.menu_button(
                    crate::tr(format!("{}  Verschieben nach", icon::FOLDER_SIMPLE)),
                    |ui| {
                        ui.set_min_width(150.0);
                        for f in phys_folders.iter().filter(|f| **f != item.folder) {
                            if ui.button(f).clicked() {
                                move_to = Some(f.clone());
                                ui.close_menu();
                            }
                        }
                    },
                );
                if ui
                    .button(crate::tr(format!("{}  Umbenennen", icon::PENCIL_SIMPLE)))
                    .clicked()
                {
                    act = Some(1);
                    ui.close_menu();
                }
                if ui
                    .button(crate::tr(format!(
                        "{}  Im Explorer zeigen",
                        icon::FOLDER_OPEN
                    )))
                    .clicked()
                {
                    if let Some(p) = &item.path {
                        crate::home::reveal(p);
                    }
                    ui.close_menu();
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
                        .color(Color32::from_rgb(0xE0, 0x80, 0x60)),
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
        let title = match d {
            LibDialog::NewFolder { .. } => "Neuer Ordner",
            LibDialog::Rename { .. } | LibDialog::RenameCat { .. } => "Umbenennen",
            LibDialog::DeleteCat { .. } => "Ordner entfernen?",
        };
        let modal = egui::Modal::new(egui::Id::new("lib-dialog")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(crate::tr(title)).strong().size(14.0));
            ui.add_space(8.0);
            match d {
                LibDialog::NewFolder { name, .. } | LibDialog::Rename { name, .. } | LibDialog::RenameCat { name, .. } => {
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if fresh {
                        r.request_focus();
                    }
                    if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                    if let LibDialog::RenameCat { key, .. } = d {
                        if key.starts_with("b:") {
                            ui.label(RichText::new(crate::tr("Eingebaute Kategorien bekommen nur einen eigenen Anzeigenamen.")).small().color(theme::TEXT_FAINT));
                        }
                    }
                }
                LibDialog::DeleteCat { area, key, label } => {
                    let text = if key.starts_with("p:") && area_kind(area).is_some() {
                        format!("„{label}“ und sein Inhalt werden in den Cutline-Papierkorb verschoben.")
                    } else {
                        format!("Der Ordner „{label}“ wird entfernt. Die Einträge darin bleiben erhalten.")
                    };
                    ui.label(crate::tr(text));
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if matches!(d, LibDialog::DeleteCat { .. }) { "Entfernen" } else { "OK" };
                    if ui.add(theme::primary(label)).clicked() {
                        ok = true;
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if modal.should_close() && !fresh {
            close = true;
        }
        if ok {
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
                    self.library.delete_cat(area, &key, area_kind(area))
                }
                None => Ok(()),
            };
            if let Err(e) = res {
                self.toast_err(e);
            }
        } else if close {
            self.lib_dialog = None;
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
            let out = crate::paths::data_dir()
                .join("cache")
                .join("svg")
                .join(format!("{}.png", path.file_stem()?.to_string_lossy()));
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
        all.into_iter()
            .filter(|i| {
                self.library
                    .visible(area, cat, q, &i.key, &i.name, &i.folder)
            })
            .collect()
    }
}
