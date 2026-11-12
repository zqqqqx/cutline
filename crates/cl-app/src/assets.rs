//! Linke Karte: senkrechte Icon-Leiste (passt bei jeder Fensterbreite)
//! und die Bibliotheken: Medien, Audio (Soundeffekte, Musik, Aufnahmen), Text (Vorlagen,
//! Texteffekte, Schriften), Sticker, Effekte, Übergänge, Filter, Untertitel, Plugins.
//! Alles lässt sich anklicken (am Playhead bzw. auf die Auswahl), über „+“ hinzufügen oder
//! in Zeitleiste/Player ziehen.

use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense};

use crate::app::{App, ClipTab, DragAsset, LeftTab};
use crate::preview::{sample_source, Job};
use crate::theme::{self, icon};
use crate::widgets;

const RAIL_W: f32 = 58.0;

const TABS: [(LeftTab, &str, &str); 9] = [
    (LeftTab::Media, icon::FILM_STRIP, "Medien"),
    (LeftTab::Audio, icon::MUSIC_NOTES, "Audio"),
    (LeftTab::Text, icon::TEXT_T, "Text"),
    (LeftTab::Stickers, icon::STICKER, "Sticker"),
    (LeftTab::Effects, icon::SPARKLE, "Effekte"),
    (LeftTab::Transitions, icon::INTERSECT, "Übergänge"),
    (LeftTab::Filters, icon::CIRCLE_HALF, "Filter"),
    (LeftTab::Captions, icon::SUBTITLES, "Untertitel"),
    (LeftTab::Plugins, icon::PUZZLE_PIECE, "Plugins"),
];

const THUMB: (u32, u32) = (192, 108);

/// Audiodateien im Musikordner des Benutzers (einmalig im Hintergrund gesucht).
static MUSIC_LIST: std::sync::Mutex<Option<Vec<PathBuf>>> = std::sync::Mutex::new(None);
static MUSIC_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn music_files() -> Option<Vec<PathBuf>> {
    if !MUSIC_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        std::thread::spawn(|| {
            fn walk(d: &std::path::Path, out: &mut Vec<PathBuf>, depth: u8) {
                let Ok(rd) = std::fs::read_dir(d) else { return };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() && depth < 3 {
                        walk(&p, out, depth + 1);
                    } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| ["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus"].contains(&x.to_ascii_lowercase().as_str())) {
                        out.push(p);
                    }
                    if out.len() > 400 {
                        return;
                    }
                }
            }
            let mut out = Vec::new();
            if let Some(b) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from) {
                for d in ["Music", "Musik"] {
                    walk(&b.join(d), &mut out, 0);
                }
            }
            out.sort();
            *MUSIC_LIST.lock().unwrap() = Some(out);
        });
    }
    MUSIC_LIST.lock().ok().and_then(|g| g.clone())
}

impl App {
    pub fn assets_panel(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        // Breite gehört dem Panel (sonst schrumpft es auf den Inhalt)
        ui.expand_to_include_rect(full);
        let rail = Rect::from_min_max(full.min, pos2(full.left() + RAIL_W, full.bottom()));
        ui.painter().rect_filled(rail, egui::CornerRadius { nw: theme::RADIUS, sw: theme::RADIUS, ne: 0, se: 0 }, Color32::from_rgb(0x13, 0x13, 0x16));
        // Icon-Leiste
        let mut y = rail.top() + 8.0;
        for (tab, glyph, label) in TABS {
            let r = Rect::from_min_size(pos2(rail.left() + 5.0, y), vec2(RAIL_W - 10.0, 46.0));
            let resp = ui.interact(r, ui.id().with(("rail", label)), Sense::click());
            let active = self.tab == tab;
            if active {
                ui.painter().rect_filled(r, 7.0, theme::BG_RAISED);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 7.0, Color32::from_rgb(0x1C, 0x1C, 0x20));
            }
            let col = if active { theme::ACCENT } else if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
            ui.painter().text(r.center() - vec2(0.0, 7.0), egui::Align2::CENTER_CENTER, crate::tr(glyph), egui::FontId::proportional(17.0), col);
            ui.painter().text(r.center() + vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, crate::tr(label), egui::FontId::proportional(9.5), col);
            if resp.clicked() {
                self.tab = tab;
            }
            y += 50.0;
            if y > rail.bottom() - 46.0 {
                break;
            }
        }
        let content = Rect::from_min_max(pos2(rail.right() + 10.0, full.top() + 8.0), pos2(full.right() - 8.0, full.bottom() - 6.0));
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.set_clip_rect(content.expand2(vec2(2.0, 4.0)));
            match self.tab {
                LeftTab::Media => self.media_tab(ui),
                LeftTab::Audio => self.audio_tab(ui),
                LeftTab::Text => self.text_tab(ui),
                LeftTab::Stickers => self.sticker_tab(ui),
                LeftTab::Effects => self.effects_tab(ui, false),
                LeftTab::Filters => self.effects_tab(ui, true),
                LeftTab::Transitions => self.transitions_tab(ui),
                LeftTab::Captions => self.captions_tab(ui),
                LeftTab::Plugins => self.plugins_tab(ui),
            }
        });
    }

    fn tab_title(ui: &mut egui::Ui, title: &str, actions: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            ui.label(RichText::new(crate::tr(title)).strong().size(13.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
        });
        ui.add_space(4.0);
    }

    fn categories(&mut self, ui: &mut egui::Ui, key: &'static str, cats: &[&str]) -> String {
        let cur = self.asset_cat.get(key).cloned().unwrap_or_else(|| "Alle".into());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for c in std::iter::once("Alle").chain(cats.iter().copied()) {
                if theme::cat_chip(ui, c, cur == c).clicked() {
                    self.asset_cat.insert(key, c.to_string());
                }
            }
        });
        ui.add_space(6.0);
        self.asset_cat.get(key).cloned().unwrap_or_else(|| "Alle".into())
    }

    // ------------------------------------------------------------------ Medien

    fn media_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Medien", |_| {});
        ui.horizontal(|ui| {
            if ui.add(theme::primary(format!("{}  Importieren", icon::PLUS))).on_hover_text(crate::tr("Strg+I")).clicked() {
                self.pick_media();
            }
            if ui.add(theme::secondary(icon::MICROPHONE)).on_hover_text(crate::tr(format!("{} (R)", self.t("voiceover")))).clicked() {
                self.voiceover.open = true;
            }
            ui.add(egui::TextEdit::singleline(&mut self.media_filter).hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS))).desired_width(f32::INFINITY));
        });
        ui.add_space(8.0);
        if self.project.media.is_empty() {
            theme::empty_state(ui, icon::FILE_ARROW_DOWN, &self.t("media-empty"), &self.t("drop-hint"));
            return;
        }
        let filter = self.media_filter.to_lowercase();
        let media: Vec<MediaRef> = self
            .project
            .media
            .iter()
            .filter(|m| filter.is_empty() || m.path.to_string_lossy().to_lowercase().contains(&filter))
            .filter(|m| !m.path.starts_with(crate::paths::data_dir().join("Stickers")) && !m.path.starts_with(crate::paths::data_dir().join("Sfx")))
            .cloned()
            .collect();
        let used: std::collections::HashSet<MediaId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| match &c.content {
                ClipContent::Media { media, .. } => Some(*media),
                _ => None,
            })
            .collect();
        let mut add: Option<MediaId> = None;
        egui::ScrollArea::vertical().id_salt("media").auto_shrink([false, false]).show(ui, |ui| {
            widgets::grid(ui, &media, 104.0, 16.0 / 9.0, 16.0, |ui, r, m| {
                let name = m.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let thumb = self.thumbs.get(&m.id).map(|t| (t.id(), t.size_vec2()));
                let resp = widgets::tile(ui, egui::Id::new(("media", m.id)), r, &name, false, |p, a| {
                    if let Some((tid, ts)) = thumb {
                        let s = (a.width() / ts.x).max(a.height() / ts.y);
                        let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2((a.width() / (ts.x * s)).min(1.0), (a.height() / (ts.y * s)).min(1.0)));
                        p.add(egui::epaint::RectShape::filled(a, 6.0, Color32::WHITE).with_texture(tid, uv));
                    } else {
                        let g = match m.kind {
                            MediaKind::Audio => icon::WAVEFORM,
                            MediaKind::Image => icon::IMAGE,
                            MediaKind::Video => icon::FILM_STRIP,
                        };
                        p.text(a.center(), egui::Align2::CENTER_CENTER, crate::tr(g), egui::FontId::proportional(22.0), theme::TEXT_DIM);
                    }
                    if m.kind != MediaKind::Image {
                        let s = m.duration.as_secs_f64();
                        let g = p.layout_no_wrap(format!("{}:{:02}", (s / 60.0).floor(), s.floor() as i64 % 60), egui::FontId::proportional(9.5), Color32::WHITE);
                        let br = Rect::from_min_size(a.left_bottom() + vec2(5.0, -g.size().y - 7.0), g.size() + vec2(6.0, 2.0));
                        p.rect_filled(br, 3.0, Color32::from_black_alpha(170));
                        p.galley(br.min + vec2(3.0, 1.0), g, Color32::WHITE);
                    }
                    if used.contains(&m.id) {
                        p.text(a.left_top() + vec2(6.0, 5.0), egui::Align2::LEFT_TOP, icon::CHECK_CIRCLE, egui::FontId::proportional(12.0), theme::ACCENT);
                    }
                });
                resp.dnd_set_drag_payload(DragAsset::Media(m.id));
                let resp = resp.on_hover_text(crate::tr(format!("{}\n{}×{} · {}\nKlick auf + oder Doppelklick fügt am Playhead ein", m.path.display(), m.width, m.height, m.video_codec.clone().or(m.audio_codec.clone()).unwrap_or_default())));
                if resp.double_clicked() || (resp.clicked() && widgets::hit_plus(&resp, r, 16.0)) {
                    add = Some(m.id);
                }
                resp.context_menu(|ui| {
                    if ui.button(crate::tr(format!("{}  Am Playhead einfügen", icon::PLUS))).clicked() {
                        add = Some(m.id);
                        ui.close_menu();
                    }
                    if ui.button(crate::tr(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                        crate::home::reveal(&m.path);
                        ui.close_menu();
                    }
                });
            });
        });
        if let Some(m) = add {
            self.add_media_at(m, self.playhead, None);
        }
    }

    // ------------------------------------------------------------------ Audio

    fn audio_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Audio", |ui| {
            if ui.add(theme::secondary(format!("{}  Importieren", icon::PLUS))).clicked() {
                self.pick_media();
            }
        });
        let mut sel = match self.asset_cat.get("audio-mode").map(|s| s.as_str()) {
            Some("music") => 1,
            Some("rec") => 2,
            _ => 0,
        };
        if widgets::segmented(ui, &["Soundeffekte", "Musik", "Aufnahmen"], &mut sel) {
            self.asset_cat.insert("audio-mode", ["sfx", "music", "rec"][sel].into());
        }
        ui.add_space(8.0);
        match sel {
            0 => {
                let cat = self.categories(ui, "sfx", &cl_fx::sfx::SFX_CATEGORIES);
                let items: Vec<&'static cl_fx::sfx::Sfx> = cl_fx::sfx::SFX.iter().filter(|s| cat == "Alle" || s.category == cat).collect();
                egui::ScrollArea::vertical().id_salt("sfx").auto_shrink([false, false]).show(ui, |ui| {
                    for s in items {
                        let path = App::sfx_path(s.id);
                        let key = format!("sfx:{}", s.id);
                        let playing = self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key);
                        let (play, add, drag) = self.sound_row(ui, &key, s.name, s.category, playing);
                        if let Some(r) = drag {
                            r.dnd_set_drag_payload(DragAsset::Sfx(s.id));
                        }
                        if play {
                            if cl_fx::sfx::render(s, &path).is_ok() {
                                self.preview_sound(key.clone(), &path);
                            }
                        }
                        if add {
                            self.add_sfx_at(s.id, self.playhead, None);
                        }
                    }
                });
            }
            1 => {
                let list = music_files();
                match list {
                    None => {
                        ui.label(RichText::new(crate::tr("Musikordner wird durchsucht …")).color(theme::TEXT_DIM));
                    }
                    Some(l) if l.is_empty() => theme::empty_state(ui, icon::MUSIC_NOTES, "Keine Musik gefunden", "Lege Dateien in deinen Musik-Ordner oder importiere sie"),
                    Some(l) => {
                        ui.label(RichText::new(crate::tr(format!("{} Titel aus deinem Musik-Ordner", l.len()))).small().color(theme::TEXT_DIM));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("music").auto_shrink([false, false]).show(ui, |ui| {
                            for p in l {
                                let key = format!("music:{}", p.display());
                                let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                let dir = p.parent().and_then(|d| d.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                let playing = self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key);
                                let (play, add, drag) = self.sound_row(ui, &key, &name, &dir, playing);
                                if let Some(r) = drag {
                                    r.dnd_set_drag_payload(DragAsset::File(p.clone()));
                                }
                                if play {
                                    self.preview_sound(key.clone(), &p);
                                }
                                if add {
                                    self.add_file_at(p.clone(), self.playhead, None);
                                }
                            }
                        });
                    }
                }
            }
            _ => {
                let r = egui::Frame::default().fill(theme::BG_RAISED).corner_radius(8).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::MICROPHONE).size(22.0).color(theme::ACCENT));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(crate::tr(self.t("voiceover"))).strong());
                            ui.label(RichText::new(crate::tr("Am Playhead einsprechen, die Waveform wächst live mit (R)")).small().color(theme::TEXT_DIM));
                        });
                    });
                });
                if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.voiceover.open = true;
                }
                ui.add_space(8.0);
                let recs: Vec<MediaRef> = self.project.media.iter().filter(|m| m.path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Voiceover-"))).cloned().collect();
                if recs.is_empty() {
                    ui.label(RichText::new(crate::tr("Noch keine Aufnahmen in diesem Projekt")).small().color(theme::TEXT_DIM));
                }
                for m in recs {
                    let key = format!("rec:{}", m.id);
                    let playing = self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key);
                    let name = m.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let (play, add, drag) = self.sound_row(ui, &key, &name, &format!("{:.1} s", m.duration.as_secs_f64()), playing);
                    if let Some(r) = drag {
                        r.dnd_set_drag_payload(DragAsset::Media(m.id));
                    }
                    if play {
                        self.preview_sound(key.clone(), &m.path);
                    }
                    if add {
                        self.add_media_at(m.id, self.playhead, None);
                    }
                }
            }
        }
    }

    /// Zeile mit Abspielen, Name, Info und „+“. Gibt (abspielen, hinzufügen, Zieh-Antwort) zurück.
    fn sound_row(&mut self, ui: &mut egui::Ui, key: &str, name: &str, info: &str, playing: bool) -> (bool, bool, Option<egui::Response>) {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click_and_drag());
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(r, 6.0, theme::BG_RAISED);
        }
        let play_r = Rect::from_center_size(r.left_center() + vec2(18.0, 0.0), vec2(26.0, 26.0));
        p.circle_filled(play_r.center(), 13.0, if playing { theme::ACCENT } else { theme::BG_HOVER });
        p.text(play_r.center(), egui::Align2::CENTER_CENTER, crate::tr(if playing { icon::STOP } else { icon::PLAY }), egui::FontId::proportional(11.0), if playing { theme::ACCENT_INK } else { theme::TEXT });
        p.text(r.left_center() + vec2(40.0, -7.0), egui::Align2::LEFT_CENTER, crate::tr(name), egui::FontId::proportional(12.0), theme::TEXT);
        p.text(r.left_center() + vec2(40.0, 8.0), egui::Align2::LEFT_CENTER, crate::tr(info), egui::FontId::proportional(10.0), theme::TEXT_FAINT);
        let plus_r = Rect::from_center_size(r.right_center() - vec2(16.0, 0.0), vec2(22.0, 22.0));
        if resp.hovered() {
            p.circle_filled(plus_r.center(), 10.0, theme::ACCENT);
            p.text(plus_r.center(), egui::Align2::CENTER_CENTER, icon::PLUS, egui::FontId::proportional(11.0), theme::ACCENT_INK);
        }
        let pos = resp.interact_pointer_pos();
        let clicked = resp.clicked();
        let play = clicked && pos.is_some_and(|q| play_r.expand(4.0).contains(q));
        let add = (clicked && pos.is_some_and(|q| plus_r.expand(4.0).contains(q))) || resp.double_clicked();
        let _ = key;
        (play, add, Some(resp.on_hover_text(crate::tr("Ziehen oder + fügt am Playhead ein"))))
    }

    // ------------------------------------------------------------------ Text

    fn text_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Text", |ui| {
            if ui.add(theme::secondary(format!("{}  Schrift importieren", icon::UPLOAD_SIMPLE))).clicked() {
                let ctx = ui.ctx().clone();
                self.pick_fonts(&ctx);
            }
        });
        let mut mode = match self.asset_cat.get("text-mode").map(|s| s.as_str()) {
            Some("effects") => 1,
            Some("fonts") => 2,
            _ => 0,
        };
        if widgets::segmented(ui, &["Vorlagen", "Texteffekte", "Schriften"], &mut mode) {
            self.asset_cat.insert("text-mode", ["tpl", "effects", "fonts"][mode].into());
        }
        ui.add_space(8.0);
        match mode {
            0 => {
                let cat = self.categories(ui, "text", &cl_fx::text::TEMPLATE_CATEGORIES);
                egui::ScrollArea::vertical().id_salt("tpl").auto_shrink([false, false]).show(ui, |ui| {
                    // Standardtext zuerst
                    let mut items: Vec<Option<&'static cl_fx::text::TextTemplate>> = vec![None];
                    items.extend(cl_fx::text::TEMPLATES.iter().filter(|t| cat == "Alle" || t.category == cat).map(Some));
                    let mut add: Option<Option<&'static str>> = None;
                    widgets::grid(ui, &items, 120.0, 16.0 / 10.0, 16.0, |ui, r, t| {
                        let (id, name) = match t {
                            Some(t) => (t.id, t.name),
                            None => ("default", "Standardtext"),
                        };
                        let style = t.map(|t| t.make()).unwrap_or_else(|| TextStyle { text: "Text".into(), ..Default::default() });
                        let resp = widgets::tile(ui, egui::Id::new(("tpl", id)), r, name, false, |p, a| {
                            p.rect_filled(a, 6.0, Color32::from_rgb(0x2A, 0x2D, 0x36));
                            self.text_preview(p, &style, a);
                        });
                        resp.dnd_set_drag_payload(DragAsset::Text(t.map(|t| t.id)));
                        if resp.clicked() || resp.double_clicked() {
                            add = Some(t.map(|t| t.id));
                        }
                    });
                    if let Some(t) = add {
                        self.add_text_at(self.playhead, t, None);
                    }
                });
            }
            1 => {
                let sel_text = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| matches!(c.content, ClipContent::Text(_)));
                ui.label(RichText::new(crate::tr(if sel_text { "Klick wendet den Effekt auf den ausgewählten Text an" } else { "Klick fügt einen neuen Text mit diesem Effekt ein" })).small().color(theme::TEXT_DIM));
                ui.add_space(6.0);
                let items: Vec<&'static cl_fx::text::TextEffect> = cl_fx::text::TEXT_EFFECTS.iter().collect();
                let mut chosen = None;
                egui::ScrollArea::vertical().id_salt("teff").auto_shrink([false, false]).show(ui, |ui| {
                    widgets::grid(ui, &items, 76.0, 1.0, 16.0, |ui, r, e| {
                        let mut st = TextStyle { text: "Aa".into(), size: 64.0, bold: true, ..Default::default() };
                        (e.apply)(&mut st);
                        let resp = widgets::tile(ui, egui::Id::new(("teff", e.id)), r, e.name, false, |p, a| {
                            p.rect_filled(a, 6.0, Color32::from_rgb(0x2A, 0x2D, 0x36));
                            self.text_preview(p, &st, a);
                        });
                        if resp.clicked() {
                            chosen = Some(e.apply);
                        }
                    });
                });
                if let Some(apply) = chosen {
                    if sel_text {
                        self.modify_selection(|c| {
                            if let ClipContent::Text(s) = &mut c.content {
                                apply(s);
                            }
                        });
                    } else if let Some(id) = self.add_text_at(self.playhead, None, None) {
                        self.modify_selection(|c| {
                            if let ClipContent::Text(s) = &mut c.content {
                                apply(s);
                            }
                        });
                        let _ = id;
                    }
                }
            }
            _ => self.fonts_section(ui),
        }
    }

    /// Text eingepasst in eine Kachel zeichnen.
    fn text_preview(&self, p: &egui::Painter, st: &TextStyle, a: Rect) {
        let mut s = st.clone();
        s.auto_wrap = false;
        let probe = p.layout_no_wrap(if s.uppercase { s.text.to_uppercase() } else { s.text.clone() }, egui::FontId::new(s.size, self.font_family(s.font_family.as_deref())), Color32::WHITE);
        let k = ((a.width() * 0.8) / probe.size().x.max(1.0)).min((a.height() * 0.6) / probe.size().y.max(1.0)).min(1.0);
        self.draw_text(p, &s, a.center(), k, 0.0, 1.0, a.width());
    }

    fn fonts_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (g, pr) = (self.t("font-scope-global"), self.t("font-scope-project"));
            ui.label(RichText::new(crate::tr(self.t("font-scope"))).small().color(theme::TEXT_DIM));
            ui.radio_value(&mut self.font_scope_project, false, crate::tr(g));
            ui.radio_value(&mut self.font_scope_project, true, crate::tr(pr));
        });
        let fam_count = self.fonts.lock().unwrap().families().len() as i64;
        let sys = (*self.system_font_count.lock().unwrap()).map(|n| format!(" · {n} Systemschriften")).unwrap_or_default();
        ui.label(RichText::new(crate::tr(format!("{}{sys} · TTF, OTF, TTC, WOFF hierher ziehen", self.i18n.t_num("fonts-count", "count", fam_count)))).small().color(theme::TEXT_DIM));
        ui.add_space(6.0);
        let families: Vec<(String, Vec<cl_fonts::FaceInfo>)> = self.fonts.lock().unwrap().families().into_iter().map(|(k, v)| (k, v.into_iter().map(|f| f.info.clone()).collect())).collect();
        if families.is_empty() {
            theme::empty_state(ui, icon::TEXT_AA, "Noch keine eigenen Schriften", "Importiere Schriftdateien oder ziehe sie hierher");
        }
        let mut remove = None;
        let mut use_font = None;
        egui::ScrollArea::vertical().id_salt("fonts").auto_shrink([false, false]).show(ui, |ui| {
            for (fam, faces) in families {
                egui::Frame::default().fill(theme::BG_RAISED).corner_radius(8).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::tr(&fam)).strong());
                        let styles: Vec<&str> = faces.iter().map(|f| f.style.as_str()).collect();
                        ui.label(RichText::new(crate::tr(styles.join(" · "))).small().color(theme::TEXT_DIM));
                        if faces.iter().any(|f| f.is_variable()) {
                            ui.label(RichText::new(crate::tr(self.t("font-variable"))).small().color(theme::ACCENT));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, icon::TRASH, self.t("font-remove"), true, false).clicked() {
                                remove = Some(fam.clone());
                            }
                            if theme::icon_button(ui, icon::TEXT_T, "Für ausgewählten Text verwenden", true, false).clicked() {
                                use_font = Some(fam.clone());
                            }
                        });
                    });
                    ui.label(RichText::new(crate::tr("Cutline 0123 ÄÖÜ äöü ß")).font(egui::FontId::new(20.0, self.font_family(Some(&fam)))));
                    if faces.iter().any(|f| f.embedding == cl_fonts::Embedding::Restricted) {
                        ui.label(RichText::new(crate::tr(format!("{} {}", icon::WARNING, self.t("font-restricted")))).small().color(Color32::from_rgb(0xE0, 0xA0, 0x40)));
                    }
                });
                ui.add_space(6.0);
            }
        });
        if let Some(f) = use_font {
            self.modify_selection(|c| {
                if let ClipContent::Text(s) = &mut c.content {
                    s.font_family = Some(f.clone());
                }
            });
        }
        if let Some(f) = remove {
            let res = self.fonts.lock().unwrap().remove_family(&f);
            if let Err(e) = res {
                self.toast_err(e.to_string());
            }
            let ctx = ui.ctx().clone();
            self.register_fonts(&ctx);
        }
    }

    // ------------------------------------------------------------------ Sticker

    fn sticker_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Sticker", |_| {});
        let cat = self.categories(ui, "sticker", &cl_fx::stickers::STICKER_CATEGORIES);
        let items: Vec<&'static cl_fx::stickers::Sticker> = cl_fx::stickers::STICKERS.iter().filter(|s| cat == "Alle" || s.category == cat).collect();
        for s in &items {
            if !self.sticker_tex.contains_key(s.id) {
                if let Some(px) = cl_fx::stickers::render_rgba(s, 128) {
                    let img = egui::ColorImage::from_rgba_premultiplied([128, 128], &px);
                    let t = ui.ctx().load_texture(format!("st-{}", s.id), img, egui::TextureOptions::LINEAR);
                    self.sticker_tex.insert(s.id, t);
                }
            }
        }
        let mut add = None;
        egui::ScrollArea::vertical().id_salt("stickers").auto_shrink([false, false]).show(ui, |ui| {
            widgets::grid(ui, &items, 70.0, 1.0, 16.0, |ui, r, s| {
                let tex = self.sticker_tex.get(s.id).map(|t| t.id());
                let resp = widgets::tile(ui, egui::Id::new(("st", s.id)), r, s.name, false, |p, a| {
                    if let Some(t) = tex {
                        let inner = Rect::from_center_size(a.center(), vec2(a.height() * 0.72, a.height() * 0.72));
                        p.image(t, inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                });
                resp.dnd_set_drag_payload(DragAsset::Sticker(s.id));
                if resp.clicked() {
                    add = Some(s.id);
                }
            });
        });
        if let Some(id) = add {
            self.add_sticker_at(id, self.playhead, None);
        }
    }

    // ------------------------------------------------------------------ Effekte / Filter

    fn fx_thumb(&mut self, key: &str, graph: impl FnOnce() -> String) -> Option<egui::TextureId> {
        if let Some(t) = self.fx_thumbs.get(key) {
            return Some(t.id());
        }
        if self.fx_thumbs_requested.insert(key.to_string()) {
            let cache = crate::paths::data_dir().join("cache").join("fx").join(format!("{}.rgba", key.replace([':', '/', '\\'], "_")));
            self.decoder.send(Job::FxThumb { key: key.to_string(), graph: graph(), w: THUMB.0, h: THUMB.1, cache });
        }
        None
    }

    fn effects_tab(&mut self, ui: &mut egui::Ui, filters: bool) {
        let target = self.selected.filter(|s| self.project.find_clip(*s).is_some_and(|(_, c)| matches!(c.content, ClipContent::Media { kind: MediaKind::Video | MediaKind::Image, .. })));
        Self::tab_title(ui, if filters { "Filter" } else { "Effekte" }, |ui| {
            if filters && ui.add(theme::secondary(format!("{}  LUT", icon::UPLOAD_SIMPLE))).on_hover_text(crate::tr("LUT (.cube) auf den ausgewählten Clip anwenden")).clicked() {
                match target {
                    Some(c) => {
                        if let Some(f) = crate::dialog().add_filter("LUT", &["cube"]).pick_file() {
                            let path = f.to_string_lossy().into_owned();
                            self.modify_selection(|x| x.fx.lut = Some(path.clone()));
                            let _ = c;
                        }
                    }
                    None => self.toast_err("Wähle zuerst einen Video- oder Bildclip"),
                }
            }
        });
        ui.label(RichText::new(crate::tr(if target.is_some() { "Klick wendet auf den ausgewählten Clip an, Ziehen auf einen Clip in der Zeitleiste" } else { "Clip auswählen oder Effekt auf einen Clip ziehen" })).small().color(theme::TEXT_DIM));
        ui.add_space(6.0);
        let plugin: Vec<cl_fx::PluginFx> = cl_fx::plugin_fx().into_iter().filter(|p| p.is_filter == filters).collect();
        let mut cats: Vec<&str> = if filters { cl_fx::FILTER_CATEGORIES.to_vec() } else { cl_fx::EFFECT_CATEGORIES.to_vec() };
        if !plugin.is_empty() {
            cats.push("Plugins");
        }
        let cat = self.categories(ui, if filters { "filter" } else { "effect" }, &cats);
        let mut items: Vec<(String, String)> = if filters { cl_fx::FILTERS.iter() } else { cl_fx::EFFECTS.iter() }.filter(|d| cat == "Alle" || d.category == cat).map(|d| (d.id.to_string(), d.name.to_string())).collect();
        if cat == "Alle" || cat == "Plugins" {
            items.extend(plugin.iter().map(|p| (p.id.clone(), p.name.clone())));
        }
        let current: Vec<String> = target
            .and_then(|t| self.project.find_clip(t))
            .map(|(_, c)| if filters { c.fx.filter.iter().map(|f| f.id.clone()).collect() } else { c.fx.effects.iter().map(|e| e.id.clone()).collect() })
            .unwrap_or_default();
        let mut chosen = None;
        egui::ScrollArea::vertical().id_salt(if filters { "flt" } else { "eff" }).auto_shrink([false, false]).show(ui, |ui| {
            if filters {
                // „Kein Filter“
                let none_sel = target.is_some() && current.is_empty();
                let mut list = vec![("".to_string(), "Ohne".to_string())];
                list.extend(items.clone());
                items = list;
                let _ = none_sel;
            }
            widgets::grid(ui, &items, 84.0, 16.0 / 9.0, 16.0, |ui, r, (id, name)| {
                let key = format!("{}:{id}", if filters { "flt" } else { "fx" });
                let tex = if id.is_empty() {
                    self.fx_thumb("orig", || format!("{},trim=start=0.5,setpts=PTS-STARTPTS,format=rgba[out]", sample_source(THUMB.0, THUMB.1, true)))
                } else {
                    let id2 = id.clone();
                    self.fx_thumb(&key, || {
                        let mut fx = ClipFx::default();
                        if filters {
                            fx.filter = Some(FxRef::new(id2));
                        } else {
                            fx.effects.push(FxRef { id: id2, intensity: 0.8, params: Default::default() });
                        }
                        let mut lines = vec![format!("{},trim=start=0.5,setpts=PTS-STARTPTS[s0]", sample_source(THUMB.0, THUMB.1, true))];
                        lines.extend(cl_fx::fx_graph(&fx, cl_fx::Src { w: THUMB.0, h: THUMB.1, has_alpha: false }, &cl_fx::ChainOpts { t_offset: 0.3, apply_reverse: false, lut: None, matte: None }, "s0", "fx", "t"));
                        lines.push("[fx]format=rgba[out]".into());
                        lines.join(";")
                    })
                };
                let selected = if id.is_empty() { target.is_some() && current.is_empty() } else { current.contains(id) };
                let resp = widgets::tile(ui, egui::Id::new(("fx", key.as_str())), r, name, selected, |p, a| match tex {
                    Some(t) => {
                        p.add(egui::epaint::RectShape::filled(a, 6.0, Color32::WHITE).with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
                    }
                    None => {
                        p.text(a.center(), egui::Align2::CENTER_CENTER, crate::tr(if filters { icon::CIRCLE_HALF } else { icon::SPARKLE }), egui::FontId::proportional(18.0), theme::TEXT_FAINT);
                    }
                });
                if !id.is_empty() {
                    resp.dnd_set_drag_payload(if filters { DragAsset::Filter(id.clone()) } else { DragAsset::Effect(id.clone()) });
                }
                if resp.clicked() {
                    chosen = Some(id.clone());
                }
            });
        });
        if let Some(id) = chosen {
            match target {
                Some(c) if filters => self.set_filter(c, if id.is_empty() { None } else { Some(id) }),
                Some(c) => {
                    if current.contains(&id) {
                        self.modify_selection(|x| x.fx.effects.retain(|e| e.id != id));
                    } else {
                        self.add_effect(c, &id);
                    }
                }
                None => self.toast_err("Wähle zuerst einen Video- oder Bildclip in der Zeitleiste"),
            }
            self.clip_tab = ClipTab::Effects;
        }
    }

    fn transitions_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Übergänge", |_| {});
        ui.label(RichText::new(crate::tr("Auf einen Schnitt ziehen oder anklicken (nächster Schnitt am Playhead bzw. nach dem ausgewählten Clip)")).small().color(theme::TEXT_DIM));
        ui.add_space(6.0);
        let cat = self.categories(ui, "trans", &cl_fx::TRANSITION_CATEGORIES);
        let items: Vec<(&'static str, &'static str)> = cl_fx::TRANSITIONS.iter().filter(|t| cat == "Alle" || t.2 == cat).map(|t| (t.0, t.1)).collect();
        let current = self.selected_transition.and_then(|c| self.project.find_clip(c)).and_then(|(_, c)| c.transition_out.as_ref().map(|t| t.id.clone()));
        let mut chosen = None;
        egui::ScrollArea::vertical().id_salt("trans").auto_shrink([false, false]).show(ui, |ui| {
            widgets::grid(ui, &items, 84.0, 16.0 / 9.0, 16.0, |ui, r, (id, name)| {
                let key = format!("tr:{id}");
                let tex = self.fx_thumb(&key, || {
                    format!(
                        "{}[a];{}[b];[a]format=gbrap[a1];[b]format=gbrap[b1];[a1][b1]xfade=transition={id}:duration=1:offset=0.2,trim=start=0.72,setpts=PTS-STARTPTS,format=rgba[out]",
                        sample_source(THUMB.0, THUMB.1, true),
                        sample_source(THUMB.0, THUMB.1, false)
                    )
                });
                let resp = widgets::tile(ui, egui::Id::new(("tr", *id)), r, name, current.as_deref() == Some(*id), |p, a| match tex {
                    Some(t) => {
                        p.add(egui::epaint::RectShape::filled(a, 6.0, Color32::WHITE).with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
                    }
                    None => {
                        p.text(a.center(), egui::Align2::CENTER_CENTER, icon::INTERSECT, egui::FontId::proportional(18.0), theme::TEXT_FAINT);
                    }
                });
                resp.dnd_set_drag_payload(DragAsset::Transition(id));
                if resp.clicked() {
                    chosen = Some(*id);
                }
            });
        });
        if let Some(id) = chosen {
            match self.selected_transition {
                Some(c) => self.set_transition(c, Some(id)),
                None => self.apply_transition_near_playhead(id),
            }
        }
    }

    // ------------------------------------------------------------------ Untertitel

    fn captions_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Untertitel", |_| {});
        self.captions_pro(ui);
    }

    pub(crate) fn import_srt(&mut self, path: &std::path::Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            self.toast_err("Datei nicht lesbar");
            return;
        };
        let ts = |s: &str| -> Option<Time> {
            let s = s.trim().replace(',', ".");
            let mut it = s.split(':');
            let (h, m, sec) = (it.next()?.parse::<f64>().ok()?, it.next()?.parse::<f64>().ok()?, it.next()?.parse::<f64>().ok()?);
            Some(Time::from_secs_f64(h * 3600.0 + m * 60.0 + sec))
        };
        let style = cl_fx::text::TEMPLATES.iter().find(|t| t.id == "subtitle-clean").map(|t| t.make()).unwrap_or_default();
        let index = self.project.timeline.tracks.len();
        let mut ops = vec![EditOp::InsertTrack { index, kind: TrackKind::Text }];
        let mut clips = Vec::new();
        for block in text.replace('\r', "").split("\n\n") {
            let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
            let Some(tl) = lines.iter().position(|l| l.contains("-->")) else { continue };
            let Some((a, b)) = lines[tl].split_once("-->") else { continue };
            let (Some(a), Some(b)) = (ts(a), ts(b)) else { continue };
            if b <= a {
                continue;
            }
            let txt = lines[tl + 1..].join("\n");
            let mut c = Clip::new(ClipContent::Text(TextStyle { text: txt, ..style.clone() }), a, b - a);
            c.transform.y = self.project.settings.height as f32 * 0.36;
            clips.push(c);
        }
        if clips.is_empty() {
            self.toast_err("Keine Untertitel gefunden");
            return;
        }
        let n = clips.len();
        // erst Spur anlegen, dann Clips gezielt darauf
        if let Some(OpOutput::Track(t)) = self.apply(ops.remove(0)) {
            let ops = clips.into_iter().map(|c| EditOp::AddClip { track: Some(t), clip: c }).collect();
            self.apply_batch(ops);
            self.toast(format!("{n} Untertitel importiert"));
        }
    }

    // ------------------------------------------------------------------ Plugins

    fn plugins_tab(&mut self, ui: &mut egui::Ui) {
        Self::tab_title(ui, "Plugins", |ui| {
            if ui.add(theme::secondary(format!("{}  Ordner", icon::FOLDER_OPEN))).on_hover_text(crate::tr("Plugin-Ordner öffnen")).clicked() {
                let d = crate::paths::data_dir().join("Plugins");
                let _ = std::fs::create_dir_all(&d);
                crate::home::reveal(&d);
            }
        });
        ui.label(RichText::new(crate::tr(format!("{} · {}", self.i18n.t_num("plugins-loaded", "count", self.plugins.plugins.len() as i64), self.i18n.t_num("plugin-effects", "count", self.plugins.asset_count() as i64)))).small().color(theme::TEXT_DIM));
        ui.add_space(6.0);
        let lang = self.i18n.lang().code().to_string();
        let mut changed = false;
        for p in &mut self.plugins.plugins {
            egui::Frame::default().fill(theme::BG_RAISED).corner_radius(8).inner_margin(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::tr(cl_plugin::localized(&p.manifest.plugin.name, &lang))).strong());
                    ui.label(RichText::new(crate::tr(format!("v{}", p.manifest.plugin.version))).small().color(theme::TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        changed |= widgets::toggle(ui, &mut p.enabled);
                    });
                });
                ui.label(RichText::new(crate::tr(cl_plugin::localized(&p.manifest.plugin.description, &lang))).small().color(theme::TEXT_DIM));
                ui.label(RichText::new(crate::tr(format!("{} Assets · {}", p.manifest.assets.len(), p.manifest.plugin.license))).small().color(theme::TEXT_FAINT));
            });
            ui.add_space(6.0);
        }
        for e in &self.plugins.errors {
            ui.label(RichText::new(crate::tr(format!("{} {e}", icon::WARNING))).small().color(Color32::from_rgb(0xE0, 0x80, 0x60)));
        }
        if changed {
            cl_fx::register_plugin_fx(
                self.plugins
                    .ffmpeg_assets(&lang)
                    .into_iter()
                    .filter(|a| matches!(a.1, cl_plugin::AssetKind::VideoEffect | cl_plugin::AssetKind::Filter))
                    .map(|(id, kind, name, chain)| cl_fx::PluginFx { id, name, is_filter: kind == cl_plugin::AssetKind::Filter, chain })
                    .collect(),
            );
            self.invalidate_preview();
        }
    }
}
