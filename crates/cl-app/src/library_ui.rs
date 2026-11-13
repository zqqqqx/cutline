//! Oberfläche der eigenen Bibliothek (Sticker-Tab und Soundeffekte im Audio-Tab):
//! Suche, Kategorie-/Ordner-Chips mit Favoriten, Import per Knopf oder Ablegen,
//! Kontextmenü (Favorit, verschieben, umbenennen, entfernen) und kleine Dialoge.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::library::{self, Item, Kind};
use crate::theme::{self, icon};

/// Offener Dialog der Bibliothek.
pub enum LibDialog {
    NewFolder { kind: Kind, name: String },
    Rename { item: Item, name: String },
    DeleteFolder { kind: Kind, name: String },
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
        Self { tex: HashMap::new(), requested: Default::default(), tx, rx }
    }
}

fn load_rgba(p: &std::path::Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        let svg = std::fs::read_to_string(p).ok()?;
        // render_svg liefert vormultipliziertes RGBA
        let px = cl_fx::stickers::render_svg(&svg, size)?;
        return Some((size, size, px));
    }
    let img = image::open(p).ok()?.thumbnail(size, size).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

impl Thumbs {
    pub fn get(&mut self, ctx: &egui::Context, key: &str, path: &std::path::Path) -> Option<egui::TextureId> {
        while let Ok((k, w, h, px)) = self.rx.try_recv() {
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            let t = ctx.load_texture(format!("lib-{k}"), img, egui::TextureOptions::LINEAR);
            self.tex.insert(k, t);
        }
        if let Some(t) = self.tex.get(key) {
            return Some(t.id());
        }
        if self.requested.insert(key.to_string()) {
            let (tx, k, p, ctx) = (self.tx.clone(), key.to_string(), path.to_path_buf(), ctx.clone());
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

impl App {
    /// Kopfbereich: Suche, Import, neuer Ordner, Chips. Gibt (Kategorie, Suchtext) zurück.
    pub fn lib_header(&mut self, ui: &mut egui::Ui, kind: Kind, key: &'static str, builtin_cats: &[&str]) -> (String, String) {
        let qkey: &'static str = match kind {
            Kind::Sticker => "st-q",
            Kind::Sound => "snd-q",
        };
        let mut q = self.asset_cat.get(qkey).cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 2.0 * 30.0 - 2.0 * ui.spacing().item_spacing.x).max(60.0);
            ui.add(egui::TextEdit::singleline(&mut q).hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS))).desired_width(w));
            if theme::icon_button(ui, icon::UPLOAD_SIMPLE, match kind {
                Kind::Sticker => "Eigene Sticker oder Bilder hinzufügen (auch per Ablegen)",
                Kind::Sound => "Eigene Sounds hinzufügen (auch per Ablegen)",
            }, true, false)
            .clicked()
            {
                self.lib_pick(kind);
            }
            if theme::icon_button(ui, icon::FOLDER_PLUS, "Neuer Ordner", true, false).clicked() {
                self.lib_dialog = Some(LibDialog::NewFolder { kind, name: String::new() });
            }
        });
        self.asset_cat.insert(qkey, q.clone());
        ui.add_space(4.0);
        let folders = self.library.folders(kind);
        let cur = self.asset_cat.get(key).cloned().unwrap_or_else(|| "Alle".into());
        let mut del = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            let fav_label = format!("{} Favoriten", icon::STAR);
            for (c, label) in [("Alle".to_string(), "Alle".to_string()), ("Favoriten".to_string(), fav_label)].into_iter().chain(builtin_cats.iter().map(|c| (c.to_string(), c.to_string()))) {
                if theme::cat_chip(ui, &label, cur == c).clicked() {
                    self.asset_cat.insert(key, c);
                }
            }
            for f in &folders {
                let r = theme::cat_chip(ui, &format!("{}  {f}", icon::FOLDER_SIMPLE), cur == *f);
                if r.clicked() {
                    self.asset_cat.insert(key, f.clone());
                }
                r.context_menu(|ui| {
                    if ui.button(crate::tr(format!("{}  Ordner entfernen", icon::TRASH))).clicked() {
                        del = Some(f.clone());
                        ui.close_menu();
                    }
                });
            }
        });
        if let Some(name) = del {
            self.lib_dialog = Some(LibDialog::DeleteFolder { kind, name });
        }
        let prefix = match kind {
            Kind::Sticker => "builtin:st:",
            Kind::Sound => "builtin:sfx:",
        };
        let hidden = self.library.hidden_count(prefix);
        if hidden > 0 && ui.add(theme::ghost(format!("{}  {hidden} verborgen · wieder anzeigen", icon::EYE))).clicked() {
            self.library.unhide_all(prefix);
        }
        ui.add_space(4.0);
        let cat = self.asset_cat.get(key).cloned().unwrap_or_else(|| "Alle".into());
        // Ordner gelöscht? Zurück auf „Alle“
        let cat = if cat != "Alle" && cat != "Favoriten" && !builtin_cats.contains(&cat.as_str()) && !folders.contains(&cat) { "Alle".to_string() } else { cat };
        (cat, q)
    }

    /// Dateiauswahl für eigene Sticker/Sounds; Ziel ist der gewählte Ordner (sonst „Eigene“).
    pub fn lib_pick(&mut self, kind: Kind) {
        let label = match kind {
            Kind::Sticker => "Bilder",
            Kind::Sound => "Audio",
        };
        if let Some(files) = crate::dialog().add_filter(crate::tr(label), kind.exts()).pick_files() {
            self.lib_import(kind, files);
        }
    }

    pub fn lib_import(&mut self, kind: Kind, files: Vec<PathBuf>) {
        let key = match kind {
            Kind::Sticker => "sticker",
            Kind::Sound => "sfx",
        };
        let cur = self.asset_cat.get(key).cloned().unwrap_or_default();
        let folders = self.library.folders(kind);
        let folder = if folders.contains(&cur) { cur } else { Kind::DEFAULT_FOLDER.to_string() };
        let n = self.library.import(kind, &folder, &files);
        if n == 0 {
            self.toast_err("Keine passenden Dateien");
        } else {
            self.asset_cat.insert(key, folder.clone());
            self.toast(format!("{n} in „{folder}“ hinzugefügt"));
        }
    }

    /// Kontextmenü eines Eintrags.
    pub fn lib_item_menu(&mut self, resp: &egui::Response, kind: Kind, item: &Item) {
        let fav = self.library.is_favorite(&item.key);
        let folders = self.library.folders(kind);
        let mut act: Option<u8> = None;
        let mut move_to: Option<String> = None;
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            if ui.button(crate::tr(format!("{}  {}", icon::STAR, if fav { "Aus Favoriten entfernen" } else { "Zu Favoriten" }))).clicked() {
                act = Some(0);
                ui.close_menu();
            }
            if item.is_own() {
                ui.menu_button(crate::tr(format!("{}  Verschieben nach", icon::FOLDER_SIMPLE)), |ui| {
                    ui.set_min_width(150.0);
                    for f in folders.iter().filter(|f| **f != item.folder) {
                        if ui.button(f).clicked() {
                            move_to = Some(f.clone());
                            ui.close_menu();
                        }
                    }
                    if ui.button(crate::tr(format!("{}  Neuer Ordner …", icon::FOLDER_PLUS))).clicked() {
                        act = Some(3);
                        ui.close_menu();
                    }
                });
                if ui.button(crate::tr(format!("{}  Umbenennen", icon::PENCIL_SIMPLE))).clicked() {
                    act = Some(1);
                    ui.close_menu();
                }
                if ui.button(crate::tr(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                    if let Some(p) = &item.path {
                        crate::home::reveal(p);
                    }
                    ui.close_menu();
                }
            }
            ui.separator();
            let label = if item.is_own() { "Entfernen" } else { "Verbergen" };
            if ui.button(RichText::new(crate::tr(format!("{}  {label}", icon::TRASH))).color(Color32::from_rgb(0xE0, 0x80, 0x60))).clicked() {
                act = Some(2);
                ui.close_menu();
            }
        });
        match act {
            Some(0) => self.library.toggle_favorite(&item.key),
            Some(1) => self.lib_dialog = Some(LibDialog::Rename { item: item.clone(), name: item.name.clone() }),
            Some(2) if item.path.as_ref().is_some_and(|p| self.project.media.iter().any(|m| &m.path == p)) => {
                self.toast_err(format!("„{}“ wird im offenen Projekt verwendet und bleibt deshalb erhalten", item.name));
            }
            Some(2) => {
                if let Err(e) = self.library.remove(item) {
                    self.toast_err(e);
                } else if item.is_own() {
                    self.toast(format!("„{}“ in den Papierkorb verschoben", item.name));
                }
            }
            Some(3) => self.lib_dialog = Some(LibDialog::NewFolder { kind, name: String::new() }),
            _ => {}
        }
        if let Some(f) = move_to {
            if let Err(e) = self.library.move_to(item, kind, &f) {
                self.toast_err(e);
            }
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
            LibDialog::Rename { .. } => "Umbenennen",
            LibDialog::DeleteFolder { .. } => "Ordner entfernen?",
        };
        let modal = egui::Modal::new(egui::Id::new("lib-dialog")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(crate::tr(title)).strong().size(14.0));
            ui.add_space(8.0);
            match d {
                LibDialog::NewFolder { name, .. } | LibDialog::Rename { name, .. } => {
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if fresh {
                        r.request_focus();
                    }
                    if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                }
                LibDialog::DeleteFolder { name, .. } => {
                    ui.label(crate::tr(format!("„{name}“ und sein Inhalt werden in den Cutline-Papierkorb verschoben.")));
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if matches!(d, LibDialog::DeleteFolder { .. }) { "Entfernen" } else { "OK" };
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
                Some(LibDialog::NewFolder { kind, name }) => {
                    let r = self.library.create_folder(kind, &name);
                    if r.is_ok() {
                        let key = if kind == Kind::Sticker { "sticker" } else { "sfx" };
                        self.asset_cat.insert(key, name.trim().to_string());
                    }
                    r
                }
                Some(LibDialog::Rename { item, name }) => self.library.rename(&item, &name),
                Some(LibDialog::DeleteFolder { kind, name }) => self.library.delete_folder(kind, &name),
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
    pub fn add_own_sticker_at(&mut self, path: &std::path::Path, at: cl_core::Time, track: Option<cl_core::TrackId>) -> Option<cl_core::ClipId> {
        // SVG wird für den Clip einmal als PNG gerendert
        let file = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
            let out = crate::paths::data_dir().join("cache").join("svg").join(format!("{}.png", path.file_stem()?.to_string_lossy()));
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

    /// Eigene Sticker/Sounds in der Liste nach Ordner/Suche filtern (eingebaute davor).
    pub fn lib_items(&mut self, kind: Kind, builtin: Vec<Item>, cat: &str, q: &str) -> Vec<Item> {
        let mut all = builtin;
        all.extend(self.library.own_items(kind));
        library::filter(&self.library, all, cat, q)
    }
}
