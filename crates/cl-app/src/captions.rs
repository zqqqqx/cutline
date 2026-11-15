//! Untertitel: Stil-Vorlagen (Zeilenlänge, Schrift, Position, Animation), manuelles Anlegen und
//! Bearbeiten (Text, Dauer, Zusammenführen), Suchen & Ersetzen, Stil/Aufteilung für alle.

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};

use crate::app::{App, ClipTab};
use crate::theme::{self, icon};

/// Name der Spur, auf der Untertitel landen
pub const TRACK_NAME: &str = "Untertitel";

pub struct CapPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub tip: &'static str,
    pub max_chars: usize,
    pub max_dur: f64,
    /// Abstand unter der Bildmitte, Anteil der Bildhöhe
    pub y: f32,
    pub intro: Option<&'static str>,
    pub style: fn() -> TextStyle,
}

fn st(f: impl FnOnce(&mut TextStyle)) -> TextStyle {
    let mut s = TextStyle {
        text: String::new(),
        ..Default::default()
    };
    f(&mut s);
    s
}

pub static PRESETS: &[CapPreset] = &[
    CapPreset {
        id: "classic",
        name: "Klassisch",
        tip: "Ganze Sätze, ruhig unten im Bild",
        max_chars: 42,
        max_dur: 5.0,
        y: 0.36,
        intro: None,
        style: || {
            st(|s| {
                s.size = 46.0;
                s.color = [255, 255, 255, 255];
                s.outline = Some((3.0, [0, 0, 0, 220]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "box",
        name: "Box",
        tip: "Gut lesbar auf hellen Bildern",
        max_chars: 42,
        max_dur: 5.0,
        y: 0.36,
        intro: None,
        style: || {
            st(|s| {
                s.size = 42.0;
                s.color = [255, 255, 255, 255];
                s.background = Some([0, 0, 0, 190]);
                s.shadow = false;
            })
        },
    },
    CapPreset {
        id: "punchy",
        name: "Kurz & fett",
        tip: "Wenige Wörter, groß in der Mitte – passt ins Hochformat",
        max_chars: 16,
        max_dur: 1.8,
        y: 0.12,
        intro: Some("pop"),
        style: || {
            st(|s| {
                s.size = 84.0;
                s.font_family = Some("Anton".into());
                s.uppercase = true;
                s.color = [255, 255, 255, 255];
                s.outline = Some((8.0, [0, 0, 0, 255]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "word",
        name: "Wort für Wort",
        tip: "Jedes Wort einzeln, schnell und auffällig",
        max_chars: 1,
        max_dur: 1.0,
        y: 0.05,
        intro: Some("pop"),
        style: || {
            st(|s| {
                s.size = 110.0;
                s.font_family = Some("Lilita One".into());
                s.color = [255, 214, 0, 255];
                s.outline = Some((9.0, [0, 0, 0, 255]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "soft",
        name: "Weich",
        tip: "Kurze Zeilen, sanft eingeblendet",
        max_chars: 24,
        max_dur: 3.0,
        y: 0.22,
        intro: Some("fade"),
        style: || {
            st(|s| {
                s.size = 60.0;
                s.font_family = Some("Poppins".into());
                s.color = [255, 255, 255, 255];
                s.outline = Some((4.0, [0, 0, 0, 200]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "cinema",
        name: "Kino",
        tip: "Schmale Serifenschrift, elegant",
        max_chars: 36,
        max_dur: 4.0,
        y: 0.40,
        intro: Some("fade"),
        style: || {
            st(|s| {
                s.size = 44.0;
                s.font_family = Some("Georgia".into());
                s.italic = true;
                s.color = [240, 232, 214, 255];
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "yellow-box",
        name: "Gelb auf Schwarz",
        tip: "Hoher Kontrast auf jedem Bild",
        max_chars: 30,
        max_dur: 3.5,
        y: 0.30,
        intro: None,
        style: || {
            st(|s| {
                s.size = 50.0;
                s.font_family = Some("Archivo Black".into());
                s.color = [255, 221, 0, 255];
                s.background = Some([0, 0, 0, 225]);
                s.shadow = false;
            })
        },
    },
    CapPreset {
        id: "pop",
        name: "Pop",
        tip: "Ein bis drei Wörter, springen ins Bild",
        max_chars: 12,
        max_dur: 1.2,
        y: 0.10,
        intro: Some("elastic"),
        style: || {
            st(|s| {
                s.size = 96.0;
                s.font_family = Some("Titan One".into());
                s.color = [255, 255, 255, 255];
                s.outline = Some((8.0, [0, 0, 0, 255]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "light-box",
        name: "Helle Box",
        tip: "Dunkle Schrift auf weißer Fläche",
        max_chars: 32,
        max_dur: 3.5,
        y: 0.28,
        intro: Some("wipe-fade"),
        style: || {
            st(|s| {
                s.size = 48.0;
                s.font_family = Some("Varela Round".into());
                s.color = [20, 20, 24, 255];
                s.background = Some([255, 255, 255, 240]);
                s.shadow = false;
            })
        },
    },
    CapPreset {
        id: "neon",
        name: "Neon",
        tip: "Leuchtfarbe mit dunkler Kontur",
        max_chars: 20,
        max_dur: 2.5,
        y: 0.18,
        intro: Some("flicker"),
        style: || {
            st(|s| {
                s.size = 70.0;
                s.font_family = Some("Righteous".into());
                s.color = [90, 255, 240, 255];
                s.outline = Some((5.0, [0, 40, 70, 255]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "typewriter",
        name: "Schreibmaschine",
        tip: "Festbreitenschrift auf dunkler Fläche",
        max_chars: 32,
        max_dur: 3.5,
        y: 0.32,
        intro: Some("fade"),
        style: || {
            st(|s| {
                s.size = 42.0;
                s.font_family = Some("Space Mono".into());
                s.color = [255, 255, 255, 255];
                s.background = Some([0, 0, 0, 200]);
                s.shadow = false;
            })
        },
    },
    CapPreset {
        id: "docu",
        name: "Doku",
        tip: "Klein und zurückhaltend",
        max_chars: 48,
        max_dur: 5.0,
        y: 0.42,
        intro: None,
        style: || {
            st(|s| {
                s.size = 36.0;
                s.font_family = Some("Varela Round".into());
                s.color = [255, 255, 255, 255];
                s.outline = Some((2.0, [0, 0, 0, 200]));
                s.shadow = true;
            })
        },
    },
    CapPreset {
        id: "podcast",
        name: "Podcast",
        tip: "Zwei ruhige Zeilen",
        max_chars: 64,
        max_dur: 6.0,
        y: 0.33,
        intro: Some("fade"),
        style: || {
            st(|s| {
                s.size = 44.0;
                s.font_family = Some("Varela Round".into());
                s.color = [255, 255, 255, 255];
                s.background = Some([12, 12, 16, 170]);
                s.shadow = false;
                s.line_spacing = 1.15;
            })
        },
    },
    CapPreset {
        id: "gaming",
        name: "Gaming",
        tip: "Kantig und grell",
        max_chars: 18,
        max_dur: 2.0,
        y: 0.15,
        intro: Some("glitch-in"),
        style: || {
            st(|s| {
                s.size = 72.0;
                s.font_family = Some("Russo One".into());
                s.uppercase = true;
                s.color = [90, 255, 130, 255];
                s.outline = Some((6.0, [0, 0, 0, 255]));
                s.shadow = true;
            })
        },
    },
];

/// Längere Zeilen auf zwei möglichst gleich lange Zeilen verteilen (Stil „Podcast“)
fn two_lines(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let flat = words.join(" ");
    if words.len() < 2 || flat.chars().count() < 24 {
        return flat;
    }
    let half = flat.chars().count() as f32 / 2.0;
    let mut best = (f32::MAX, 1);
    let mut len = 0;
    for (i, w) in words.iter().enumerate().take(words.len() - 1) {
        len += w.chars().count() + usize::from(i > 0);
        let d = (len as f32 - half).abs();
        if d < best.0 {
            best = (d, i + 1);
        }
    }
    format!("{}\n{}", words[..best.1].join(" "), words[best.1..].join(" "))
}

pub fn preset(id: &str) -> &'static CapPreset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

impl App {
    pub fn caption_preset(&self) -> &'static CapPreset {
        preset(&self.settings.caption_preset)
    }

    /// Spuren mit Untertiteln: nur die Untertitel-Spuren (normale Textclips sind keine Untertitel)
    pub fn caption_tracks(&self) -> Vec<TrackId> {
        self.project.timeline.tracks.iter().filter(|t| is_caption_track(t)).map(|t| t.id).collect()
    }

    pub fn caption_clips(&self) -> Vec<Clip> {
        bulk_targets(&self.project)
    }

    /// Neue Untertitel-Spur anlegen
    pub fn new_caption_track(&mut self) -> Option<TrackId> {
        let index = self.project.timeline.tracks.len();
        let Some(OpOutput::Track(t)) = self.apply(EditOp::InsertTrack {
            index,
            kind: TrackKind::Text,
        }) else {
            return None;
        };
        self.apply(EditOp::RenameTrack {
            track: t,
            name: Some(TRACK_NAME.into()),
        });
        Some(t)
    }

    /// Untertitel-Clip im gewählten Stil
    pub fn caption_clip(&self, text: &str, start: Time, dur: Time) -> Clip {
        let p = self.caption_preset();
        let text = if p.id == "podcast" { two_lines(text) } else { text.to_string() };
        let mut c = Clip::new(
            ClipContent::Text(TextStyle {
                text,
                ..(p.style)()
            }),
            start,
            dur,
        );
        c.transform.y = self.project.settings.height as f32 * p.y;
        if let Some(a) = p.intro {
            c.anim.intro = Some(AnimRef {
                id: a.into(),
                duration: Time::from_secs_f64(0.18f64.min(dur.as_secs_f64() / 3.0)),
            });
        }
        c
    }

    /// Erkannte Zeilen als Untertitel einfügen (eigene Spur, gewählter Stil)
    pub fn add_caption_segments(&mut self, segs: &[cl_analyze::asr::Segment], offset: Time) -> usize {
        let Some(t) = self.new_caption_track() else { return 0 };
        let ops: Vec<EditOp> = segs
            .iter()
            .filter(|s| s.end > s.start)
            .map(|s| EditOp::AddClip {
                track: Some(t),
                clip: self.caption_clip(
                    &s.text,
                    offset + Time::from_secs_f64(s.start),
                    Time::from_secs_f64(s.end - s.start),
                ),
            })
            .collect();
        let n = ops.len();
        self.apply_batch(ops);
        n
    }

    /// Stil-Auswahl als kleine Vorschaukacheln
    pub fn caption_preset_picker(&mut self, ui: &mut egui::Ui) {
        let cols = 2;
        let gap = 6.0;
        let w = ((ui.available_width() - gap) / cols as f32).floor();
        let h = 46.0;
        for row in PRESETS.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for p in row {
                    let (r, resp) = ui.allocate_exact_size(vec2(w, h), egui::Sense::click());
                    let on = self.settings.caption_preset == p.id;
                    let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), theme::anim(0.12));
                    let pt = ui.painter();
                    pt.rect_filled(r, theme::R_CARD, Color32::from_rgb(0x1E, 0x24, 0x2C));
                    let s = (p.style)();
                    let sample = if p.id == "word" { crate::tr("Wort") } else if s.uppercase { crate::tr("So sieht es aus").to_uppercase() } else { crate::tr("So sieht es aus") };
                    let fam = self.font_family(s.font_family.as_deref());
                    let base = (s.size * 0.24).clamp(11.0, 22.0);
                    // Platz in der Kachel: Rand je Seite, Hintergrundkasten (+8) bzw. Kontur (±1,2)
                    let room = r.width() - 12.0 - if s.background.is_some() { 8.0 } else { 0.0 } - if s.outline.is_some() { 2.4 } else { 0.0 };
                    let size = theme::fit_size(base, theme::text_width(ui.ctx(), &sample, egui::FontId::new(base, fam.clone())), room);
                    let col = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
                    let c = pos2(r.center().x, r.top() + 18.0);
                    let font = egui::FontId::new(size, fam);
                    // Glyphenbreiten wachsen nicht ganz linear: alles zusätzlich auf die Kachel begrenzen
                    let tp = pt.with_clip_rect(r.shrink(1.0));
                    if let Some(bg) = s.background {
                        let g = tp.layout_no_wrap(sample.clone(), font.clone(), col);
                        // Vorschau des Untertitel-Stils (Hintergrundkasten), kein UI-Radius
                        tp.rect_filled(Rect::from_center_size(c, g.size() + vec2(8.0, 2.0)), 3.0, Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));
                    }
                    if let Some((_, oc)) = s.outline {
                        let ocol = Color32::from_rgba_unmultiplied(oc[0], oc[1], oc[2], oc[3]);
                        for d in [vec2(-1.2, 0.0), vec2(1.2, 0.0), vec2(0.0, -1.2), vec2(0.0, 1.2)] {
                            tp.text(c + d, egui::Align2::CENTER_CENTER, &sample, font.clone(), ocol);
                        }
                    }
                    tp.text(c, egui::Align2::CENTER_CENTER, &sample, font, col);
                    let name = crate::tr(p.name);
                    let hint = egui::FontId::proportional(theme::FS_HINT);
                    let shown = theme::elide(&name, r.width() - 16.0, |t| theme::text_width(ui.ctx(), t, hint.clone()));
                    pt.text(pos2(r.left() + 8.0, r.bottom() - 9.0), egui::Align2::LEFT_CENTER, &shown, hint.clone(), Color32::from_gray(200));
                    let stroke = if on {
                        egui::Stroke::new(2.0_f32, theme::ACCENT())
                    } else {
                        // ohne Ruhe-Rahmen (Fläche hebt sich schon ab), nur Hover/Auswahl
                        egui::Stroke::new(1.0_f32, theme::TEXT_DIM().gamma_multiply(hover))
                    };
                    pt.rect_stroke(r, theme::R_CARD, stroke, egui::StrokeKind::Inside);
                    let tip = if shown == name { crate::tr(p.tip) } else { format!("{name}\n{}", crate::tr(p.tip)) };
                    if resp.on_hover_text(tip).clicked() {
                        self.settings.caption_preset = p.id.to_string();
                        self.settings.save();
                    }
                }
            });
            ui.add_space(gap);
        }
    }

    /// Liste zum Bearbeiten: hinzufügen, Text, Dauer, zusammenführen, löschen, suchen & ersetzen
    pub fn captions_editor(&mut self, ui: &mut egui::Ui) {
        let items = self.caption_clips();
        let fps = self.project.settings.fps;
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr(format!("Untertitel ({})", items.len()))).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // zu schmal für die Beschriftung: nur das Symbol
                let full = format!("{}  {}", icon::PLUS, crate::tr("Hinzufügen"));
                let need = theme::text_width(ui.ctx(), &full, egui::FontId::proportional(theme::FS_UI)) + 2.0 * ui.spacing().button_padding.x;
                let label = if need <= ui.available_width() { full } else { icon::PLUS.to_string() };
                if ui
                    .add(theme::secondary(label))
                    .on_hover_text(crate::tr("Neuer Untertitel am Playhead"))
                    .clicked()
                {
                    self.add_caption_at_playhead();
                }
            });
        });
        ui.add_space(4.0);
        if items.is_empty() {
            theme::empty_state(ui, icon::SUBTITLES, "Noch keine Untertitel", "");
            return;
        }
        // Suchen & Ersetzen
        let mut q = self.asset_cat.get("cap-q").cloned().unwrap_or_default();
        let mut rep = self.asset_cat.get("cap-r").cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            // Breite aus dem Platz: zwei Felder (Innenrand je 2 × 4) + Symbolknopf + zwei Abstände
            let sp = ui.spacing().item_spacing.x;
            let btn = (theme::text_width(ui.ctx(), icon::SWAP, egui::FontId::proportional(14.0)) + 2.0 * ui.spacing().button_padding.x).max(24.0);
            let w = ((ui.available_width() - btn - 2.0 * sp) / 2.0 - 8.0).max(30.0);
            let margin = egui::Margin::symmetric(4, 2);
            ui.add(egui::TextEdit::singleline(&mut q).hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS))).margin(margin).desired_width(w));
            ui.add(egui::TextEdit::singleline(&mut rep).hint_text(crate::tr("Ersetzen durch")).margin(margin).desired_width(w));
            let hits = if q.is_empty() { 0 } else { items.iter().filter(|c| text_of(c).contains(&q)).count() };
            if theme::icon_button(ui, icon::SWAP, format!("{}  ({hits})", crate::tr("Alle ersetzen")), hits > 0, false).clicked() {
                let ops: Vec<EditOp> = items
                    .iter()
                    .filter(|c| text_of(c).contains(&q))
                    .map(|c| {
                        let mut n = c.clone();
                        if let ClipContent::Text(s) = &mut n.content {
                            s.text = s.text.replace(&q, &rep);
                        }
                        EditOp::ReplaceClip { clip: n }
                    })
                    .collect();
                self.apply_batch(ops);
                self.toast(format!("{} {hits} {}", icon::CHECK, crate::tr("ersetzt")));
            }
        });
        self.asset_cat.insert("cap-q", q.clone());
        self.asset_cat.insert("cap-r", rep);
        // Stil bzw. Aufteilung der gewählten Vorlage auf alle: nur echte Untertitel-Spuren
        {
            let items = &items;
            ui.horizontal_wrapped(|ui| {
                let p = self.caption_preset();
                if ui
                    .add(theme::ghost(format!("{}  {}", icon::PAINT_BRUSH, crate::tr(format!("Stil „{}“ auf alle", crate::tr(p.name))))))
                    .clicked()
                {
                    let ops: Vec<EditOp> = items
                        .iter()
                        .map(|c| {
                            let n = self.caption_clip(&text_of(c), c.start, c.duration);
                            let mut c2 = c.clone();
                            c2.content = n.content;
                            c2.transform = n.transform;
                            c2.anim = n.anim;
                            EditOp::ReplaceClip { clip: c2 }
                        })
                        .collect();
                    self.apply_batch(ops);
                }
                if ui
                    .add(theme::ghost(format!("{}  {}", icon::SPLIT_HORIZONTAL, crate::tr("Neu aufteilen"))))
                    .on_hover_text(crate::tr("Zeilen so lang wie in der gewählten Vorlage"))
                    .clicked()
                {
                    self.resegment_captions(items);
                }
            });
        }
        ui.add_space(4.0);
        let mut jump = None;
        let mut ops: Vec<EditOp> = Vec::new();
        let focus = self.caption_focus;
        let mut focused = false;
        ui.vertical(|ui| {
            for (i, c) in items.iter().enumerate() {
                let text = text_of(c);
                if !q.is_empty() && !text.contains(&q) {
                    continue;
                }
                let next = items.get(i + 1);
                let active = c.contains(self.playhead);
                egui::Frame::default()
                    .fill(if active { theme::BG_HOVER() } else { theme::BG_RAISED() })
                    .corner_radius(theme::R_CARD)
                    .inner_margin(egui::Margin::symmetric(theme::PAD_CARD, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let t = c.start.timecode(fps);
                            if ui
                                .add(theme::ghost(&t[3..8]))
                                .on_hover_text(crate::tr("Hinspringen"))
                                .clicked()
                            {
                                jump = Some(c.id);
                            }
                            // Knöpfe rechts zuerst, das Textfeld nimmt den Rest: nichts ragt über den Rand
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // Textänderung vor Zusammenführen/Dauer anwenden (wie zuvor in Lesereihenfolge)
                                let at = ops.len();
                                if theme::icon_button(ui, icon::TRASH, "Löschen", true, false).clicked() {
                                    ops.push(EditOp::RemoveClip { clip: c.id });
                                }
                                if let Some(n) = next {
                                    if theme::icon_button(ui, icon::ARROWS_MERGE, "Mit dem nächsten zusammenführen", true, false).clicked() {
                                        let mut m = c.clone();
                                        if let ClipContent::Text(s) = &mut m.content {
                                            s.text = format!("{} {}", text, text_of(n));
                                        }
                                        ops.push(EditOp::RemoveClip { clip: n.id });
                                        ops.push(EditOp::SetClipTiming { clip: c.id, start: c.start, duration: n.end() - c.start, source_in: c.source_in });
                                        ops.push(EditOp::ReplaceClip { clip: m });
                                    }
                                }
                                // Dauer (bis zum nächsten Untertitel)
                                let room = next.map(|n| (n.start - c.start).as_secs_f64()).unwrap_or(60.0);
                                let mut d = c.duration.as_secs_f64();
                                if ui
                                    .add(egui::DragValue::new(&mut d).range(0.2..=room.max(0.2)).speed(0.02).max_decimals(1).suffix(" s").update_while_editing(false))
                                    .on_hover_text(crate::tr("Dauer"))
                                    .changed()
                                {
                                    ops.push(EditOp::SetClipTiming { clip: c.id, start: c.start, duration: Time::from_secs_f64(d), source_in: c.source_in });
                                }
                                let id = egui::Id::new(("cap-text", c.id));
                                let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| text.clone());
                                let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY).frame(false));
                                if focus == Some(c.id) {
                                    focused = true;
                                    // nur wenn nötig blättern: sonst wandert der Knopf „Hinzufügen“ bei jedem Klick
                                    r.scroll_to_me(None);
                                    r.request_focus();
                                    // ganzen Platzhaltertext markieren: Tippen ersetzt ihn
                                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                                        let all = egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(buf.chars().count()),
                                        );
                                        state.cursor.set_char_range(Some(all));
                                        state.store(ui.ctx(), r.id);
                                    }
                                }
                                if r.changed() {
                                    ui.data_mut(|d| d.insert_temp(id, buf.clone()));
                                }
                                if r.lost_focus() {
                                    ui.data_mut(|d| d.remove::<String>(id));
                                    if buf != text {
                                        let mut n = c.clone();
                                        if let ClipContent::Text(s) = &mut n.content {
                                            s.text = buf;
                                        }
                                        ops.insert(at, EditOp::ReplaceClip { clip: n });
                                    }
                                }
                            });
                        });
                    });
                ui.add_space(3.0);
            }
        });
        if focused {
            self.caption_focus = None;
        }
        if let Some(id) = jump {
            if let Some(start) = self.project.find_clip(id).map(|(_, c)| c.start) {
                self.stop_playback();
                self.playhead = start;
            }
            self.select_only(Some(id));
            self.clip_tab = ClipTab::Text;
        }
        if !ops.is_empty() {
            self.apply_batch(ops);
        }
    }

    /// Neuer Untertitel am Playhead (bis zum nächsten, höchstens 2 s), Textfeld bekommt den Fokus
    pub fn add_caption_at_playhead(&mut self) {
        let items = self.caption_clips();
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        if items.iter().any(|c| c.contains(at)) {
            self.toast_err("Hier liegt schon ein Untertitel");
            return;
        }
        let next = items.iter().map(|c| c.start).filter(|s| *s > at).min();
        let dur = next.map(|n| n - at).unwrap_or(Time::from_secs_f64(2.0)).min(Time::from_secs_f64(2.0));
        if dur < Time::from_secs_f64(0.2) {
            self.toast_err("Zu wenig Platz bis zum nächsten Untertitel");
            return;
        }
        let track = self
            .project
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Text && t.name.as_deref() == Some(TRACK_NAME))
            .map(|t| t.id)
            .or_else(|| self.new_caption_track());
        let clip = self.caption_clip(&crate::tr("Neuer Untertitel"), at, dur);
        if let Some(OpOutput::Clip(id)) = self.apply(EditOp::AddClip { track, clip }) {
            self.select_only(Some(id));
            self.caption_focus = Some(id);
        }
    }

    /// Bestehende Untertitel in Zeilen der gewählten Länge neu aufteilen
    fn resegment_captions(&mut self, items: &[Clip]) {
        let segs: Vec<cl_analyze::asr::Segment> = items
            .iter()
            .map(|c| cl_analyze::asr::Segment {
                start: c.start.as_secs_f64(),
                end: c.end().as_secs_f64(),
                text: text_of(c),
            })
            .collect();
        let p = self.caption_preset();
        let new = cl_analyze::asr::resegment(&segs, p.max_chars, p.max_dur);
        let mut ops: Vec<EditOp> = items.iter().map(|c| EditOp::RemoveClip { clip: c.id }).collect();
        self.apply_batch(std::mem::take(&mut ops));
        let n = self.add_caption_segments(&new, Time::ZERO);
        self.toast(format!("{} {n} {}", icon::CHECK, crate::tr("Zeilen")));
    }
}

/// Clips, die „Stil auf alle“ und „Neu aufteilen“ ändern: nur auf Spuren namens „Untertitel“.
fn is_caption_track(t: &cl_core::Track) -> bool {
    t.kind == TrackKind::Text && t.name.as_deref() == Some(TRACK_NAME)
}

fn bulk_targets(p: &Project) -> Vec<Clip> {
    let mut v: Vec<Clip> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| is_caption_track(t))
        .flat_map(|t| t.clips.iter().cloned())
        .collect();
    v.sort_by_key(|c| c.start);
    v
}

fn text_of(c: &Clip) -> String {
    match &c.content {
        ClipContent::Text(s) => s.text.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_for_all_leaves_plain_text_clips_alone() {
        let mut p = Project::new("x");
        let text = |s: &str| Clip::new(ClipContent::Text(TextStyle { text: s.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(1.0));
        let mut plain = Track::new(TrackKind::Text);
        plain.clips.push(text("DER FILM"));
        p.timeline.tracks.push(plain);
        assert!(bulk_targets(&p).is_empty(), "normale Textspur ist kein Untertitel");
        let mut caps = Track::new(TrackKind::Text);
        caps.name = Some(TRACK_NAME.into());
        caps.clips.push(text("Hallo"));
        p.timeline.tracks.push(caps);
        assert_eq!(bulk_targets(&p).iter().map(text_of).collect::<Vec<_>>(), ["Hallo"]);
        // die Liste im Untertitel-Tab nutzt dieselbe Auswahl: nur die Untertitelspur
        assert_eq!(p.timeline.tracks.iter().filter(|t| is_caption_track(t)).count(), 1);
    }

    #[test]
    fn presets_are_valid() {
        for p in PRESETS {
            assert!(p.max_chars >= 1 && p.max_dur > 0.2, "{}", p.id);
            if let Some(a) = p.intro {
                assert!(cl_core::ANIMATIONS.iter().any(|x| x.0 == a), "{a}");
            }
            assert!((p.style)().size > 10.0);
        }
        assert_eq!(preset("gibt-es-nicht").id, "classic");
        let ids: std::collections::HashSet<_> = PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PRESETS.len());
        for (i, id) in ["classic", "box", "punchy", "word", "soft", "cinema"].iter().enumerate() {
            assert_eq!(PRESETS[i].id, *id, "vorhandene Stile bleiben");
        }
        // lesbar auf hellem und dunklem Bild: Kontur oder Fläche
        for p in PRESETS {
            let s = (p.style)();
            assert!(s.outline.is_some() || s.background.is_some() || p.id == "cinema", "{}", p.id);
        }
    }

    #[test]
    fn podcast_splits_into_two_balanced_lines() {
        assert_eq!(two_lines("kurz"), "kurz");
        assert_eq!(two_lines("  zwei   Wörter "), "zwei Wörter");
        let t = two_lines("Heute reden wir über\nSchnitt und Ton in langen Videos");
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines.len(), 2, "{t}");
        assert!(lines[0].len().abs_diff(lines[1].len()) <= 8, "{t}");
        assert_eq!(t.replace('\n', " "), "Heute reden wir über Schnitt und Ton in langen Videos");
    }
}
