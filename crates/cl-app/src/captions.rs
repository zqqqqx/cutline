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
];

pub fn preset(id: &str) -> &'static CapPreset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

impl App {
    pub fn caption_preset(&self) -> &'static CapPreset {
        preset(&self.settings.caption_preset)
    }

    /// Spuren mit Untertiteln: die Untertitel-Spuren, sonst (ältere Projekte) alle Textspuren
    pub fn caption_tracks(&self) -> Vec<TrackId> {
        let named: Vec<TrackId> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Text && t.name.as_deref() == Some(TRACK_NAME))
            .map(|t| t.id)
            .collect();
        if !named.is_empty() {
            return named;
        }
        self.project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Text)
            .map(|t| t.id)
            .collect()
    }

    pub fn caption_clips(&self) -> Vec<Clip> {
        let tracks = self.caption_tracks();
        let mut v: Vec<Clip> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| tracks.contains(&t.id))
            .flat_map(|t| t.clips.iter().cloned())
            .collect();
        v.sort_by_key(|c| c.start);
        v
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
        let mut c = Clip::new(
            ClipContent::Text(TextStyle {
                text: text.to_string(),
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
                    let sample = if p.id == "word" { "Wort".to_string() } else if s.uppercase { "SO SIEHT ES AUS".into() } else { crate::tr("So sieht es aus") };
                    let fam = self.font_family(s.font_family.as_deref());
                    let size = (s.size * 0.24).clamp(11.0, 22.0);
                    let col = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
                    let c = pos2(r.center().x, r.top() + 18.0);
                    let font = egui::FontId::new(size, fam);
                    if let Some(bg) = s.background {
                        let g = pt.layout_no_wrap(sample.clone(), font.clone(), col);
                        // Vorschau des Untertitel-Stils (Hintergrundkasten), kein UI-Radius
                        pt.rect_filled(Rect::from_center_size(c, g.size() + vec2(8.0, 2.0)), 3.0, Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));
                    }
                    if let Some((_, oc)) = s.outline {
                        let ocol = Color32::from_rgba_unmultiplied(oc[0], oc[1], oc[2], oc[3]);
                        for d in [vec2(-1.2, 0.0), vec2(1.2, 0.0), vec2(0.0, -1.2), vec2(0.0, 1.2)] {
                            pt.text(c + d, egui::Align2::CENTER_CENTER, &sample, font.clone(), ocol);
                        }
                    }
                    pt.with_clip_rect(r.shrink(2.0)).text(c, egui::Align2::CENTER_CENTER, &sample, font, col);
                    pt.text(
                        pos2(r.left() + 8.0, r.bottom() - 9.0),
                        egui::Align2::LEFT_CENTER,
                        crate::tr(p.name),
                        egui::FontId::proportional(theme::FS_HINT),
                        Color32::from_gray(200),
                    );
                    let stroke = if on {
                        egui::Stroke::new(2.0_f32, theme::ACCENT())
                    } else {
                        // ohne Ruhe-Rahmen (Fläche hebt sich schon ab), nur Hover/Auswahl
                        egui::Stroke::new(1.0_f32, theme::TEXT_DIM().gamma_multiply(hover))
                    };
                    pt.rect_stroke(r, theme::R_CARD, stroke, egui::StrokeKind::Inside);
                    if resp.on_hover_text(crate::tr(p.tip)).clicked() {
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
                if ui
                    .add(theme::secondary(format!("{}  Hinzufügen", icon::PLUS)))
                    .on_hover_text(crate::tr("Neuer Untertitel am Playhead"))
                    .clicked()
                {
                    self.add_caption_at_playhead();
                }
            });
        });
        ui.add_space(4.0);
        if items.is_empty() {
            theme::empty_state(ui, icon::SUBTITLES, "Noch keine Untertitel", "Erzeugen, SRT importieren oder von Hand hinzufügen");
            return;
        }
        // Suchen & Ersetzen
        let mut q = self.asset_cat.get("cap-q").cloned().unwrap_or_default();
        let mut rep = self.asset_cat.get("cap-r").cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            let w = ((ui.available_width() - 40.0) / 2.0).max(50.0);
            ui.add(egui::TextEdit::singleline(&mut q).hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS))).desired_width(w));
            ui.add(egui::TextEdit::singleline(&mut rep).hint_text(crate::tr("Ersetzen durch")).desired_width(w));
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
        // Stil bzw. Aufteilung der gewählten Vorlage auf alle
        ui.horizontal(|ui| {
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
                self.resegment_captions(&items);
            }
        });
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
                            let id = egui::Id::new(("cap-text", c.id));
                            let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| text.clone());
                            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width((ui.available_width() - 120.0).max(40.0)).frame(false));
                            if focus == Some(c.id) {
                                focused = true;
                                r.scroll_to_me(Some(egui::Align::Center));
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
                                    ops.push(EditOp::ReplaceClip { clip: n });
                                }
                            }
                            // Dauer (bis zum nächsten Untertitel)
                            let room = next.map(|n| (n.start - c.start).as_secs_f64()).unwrap_or(60.0);
                            let mut d = c.duration.as_secs_f64();
                            if ui
                                .add(egui::DragValue::new(&mut d).range(0.2..=room.max(0.2)).speed(0.02).max_decimals(1).suffix(" s"))
                                .on_hover_text(crate::tr("Dauer"))
                                .changed()
                            {
                                ops.push(EditOp::SetClipTiming { clip: c.id, start: c.start, duration: Time::from_secs_f64(d), source_in: c.source_in });
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
                            if theme::icon_button(ui, icon::TRASH, "Löschen", true, false).clicked() {
                                ops.push(EditOp::RemoveClip { clip: c.id });
                            }
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
    fn presets_are_valid() {
        for p in PRESETS {
            assert!(p.max_chars >= 1 && p.max_dur > 0.2, "{}", p.id);
            if let Some(a) = p.intro {
                assert!(cl_core::ANIMATIONS.iter().any(|x| x.0 == a), "{a}");
            }
            assert!((p.style)().size > 10.0);
        }
        assert_eq!(preset("gibt-es-nicht").id, "classic");
    }
}
