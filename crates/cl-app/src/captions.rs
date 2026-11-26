//! Captions: style presets (line length, font, position, animation), manual creation and
//! editing (text, duration, merge), find & replace, style/splitting for all.

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};

use crate::app::{App, ClipTab};
use crate::theme::{self, icon};

/// Name of the track captions go to
pub const TRACK_NAME: &str = "Untertitel";

/// Track name for display: the default name of the caption track in the UI language
/// (it stays saved in German, the track is recognized by it), custom names as typed.
pub fn shown_track_name(name: &str) -> String {
    if name == TRACK_NAME { crate::tr(name) } else { name.to_string() }
}

pub struct CapPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub tip: &'static str,
    pub max_chars: usize,
    pub max_dur: f64,
    /// Distance below the frame center, fraction of the frame height
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
    CapPreset {
        id: "karaoke",
        name: "Karaoke",
        tip: "Das gesprochene Wort leuchtet auf",
        max_chars: 30,
        max_dur: 3.5,
        y: 0.25,
        intro: None,
        style: || karaoke_style(false),
    },
    CapPreset {
        id: "karaoke-fill",
        name: "Karaoke füllen",
        tip: "Gesprochene Wörter bleiben hervorgehoben",
        max_chars: 30,
        max_dur: 3.5,
        y: 0.25,
        intro: None,
        style: || karaoke_style(true),
    },
];

/// Highlight like the „Gelb auf Schwarz“ style
pub const KARAOKE_YELLOW: [u8; 4] = [255, 221, 0, 255];

fn karaoke_style(fill: bool) -> TextStyle {
    st(|s| {
        s.size = 64.0;
        s.font_family = Some("Archivo Black".into());
        s.color = [255, 255, 255, 255];
        s.outline = Some((6.0, [0, 0, 0, 255]));
        s.shadow = true;
        s.karaoke = Some(Karaoke { color: KARAOKE_YELLOW, fill, words: Vec::new() });
    })
}

/// Spread longer lines over two lines of about equal length („Podcast“ style)
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

    /// Tracks with captions: only the caption tracks (normal text clips are not captions)
    pub fn caption_tracks(&self) -> Vec<TrackId> {
        self.project.timeline.tracks.iter().filter(|t| is_caption_track(t)).map(|t| t.id).collect()
    }

    pub fn caption_clips(&self) -> Vec<Clip> {
        bulk_targets(&self.project)
    }

    /// Create a new caption track
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

    /// Caption clip in the chosen style; `words`: start per word relative to the clip start (karaoke)
    pub fn caption_clip(&self, text: &str, start: Time, dur: Time, words: Vec<Time>) -> Clip {
        let p = self.caption_preset();
        let text = if p.id == "podcast" { two_lines(text) } else { text.to_string() };
        let mut st = TextStyle { text, ..(p.style)() };
        // Brand font for captions only in the „Klassisch“ style (the others have their own fonts)
        if p.id == "classic" {
            if let Some(f) = self.brand.font(crate::brand::CAPTION) {
                st.font_family = Some(f.clone());
            }
        }
        if let Some(k) = &mut st.karaoke {
            k.words = words;
        }
        let mut c = Clip::new(ClipContent::Text(st), start, dur);
        c.transform.y = self.project.settings.height as f32 * p.y;
        if let Some(a) = p.intro {
            c.anim.intro = Some(AnimRef {
                id: a.into(),
                duration: Time::from_secs_f64(0.18f64.min(dur.as_secs_f64() / 3.0)),
            });
        }
        c
    }

    /// Insert recognized lines as captions (own track, chosen style)
    pub fn add_caption_segments(&mut self, segs: &[cl_analyze::asr::Segment], offset: Time) -> usize {
        // track and lines as ONE undo step
        self.one_step(|app| {
            let Some(t) = app.new_caption_track() else { return 0 };
            let ops: Vec<EditOp> = segs
                .iter()
                // checked after rounding to µs: one empty line would fail the whole batch
                .filter(|s| Time::from_secs_f64(s.end - s.start) > Time::ZERO)
                .map(|s| EditOp::AddClip {
                    track: Some(t),
                    clip: app.caption_clip(
                        &s.text,
                        offset + Time::from_secs_f64(s.start),
                        Time::from_secs_f64(s.end - s.start),
                        s.words.iter().map(|w| Time::from_secs_f64(w - s.start)).collect(),
                    ),
                })
                .collect();
            let n = ops.len();
            if app.apply_batch(ops).is_some() { n } else { 0 }
        })
    }

    /// Style picker as small preview tiles
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
                    let hover = theme::fade(ui.ctx(), resp.id.with("h"), resp.hovered(), theme::MOTION_FAST);
                    let pt = ui.painter();
                    pt.rect_filled(r, theme::R_CARD, theme::STAGE);
                    let s = (p.style)();
                    let sample = if p.id == "word" { crate::tr("Wort") } else if s.uppercase { crate::tr("So sieht es aus").to_uppercase() } else { crate::tr("So sieht es aus") };
                    let fam = self.font_family(s.font_family.as_deref());
                    let base = (s.size * 0.24).clamp(11.0, 22.0);
                    // Room in the tile: margin per side, background box (+8) or outline (±1.2)
                    let room = r.width() - 12.0 - if s.background.is_some() { 8.0 } else { 0.0 } - if s.outline.is_some() { 2.4 } else { 0.0 };
                    let size = theme::fit_size(base, theme::text_width(ui.ctx(), &sample, egui::FontId::new(base, fam.clone())), room);
                    let col = Color32::from_rgba_unmultiplied(s.color[0], s.color[1], s.color[2], s.color[3]);
                    let c = pos2(r.center().x, r.top() + 18.0);
                    let font = egui::FontId::new(size, fam);
                    // Glyph widths do not grow quite linearly: additionally limit everything to the tile
                    let tp = pt.with_clip_rect(r.shrink(1.0));
                    if let Some(bg) = s.background {
                        let g = tp.layout_no_wrap(sample.clone(), font.clone(), col);
                        // Preview of the caption style (background box), no UI radius
                        tp.rect_filled(Rect::from_center_size(c, g.size() + vec2(8.0, 2.0)), 3.0, Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));
                    }
                    if let Some((_, oc)) = s.outline {
                        let ocol = Color32::from_rgba_unmultiplied(oc[0], oc[1], oc[2], oc[3]);
                        for d in [vec2(-1.2, 0.0), vec2(1.2, 0.0), vec2(0.0, -1.2), vec2(0.0, 1.2)] {
                            tp.text(c + d, egui::Align2::CENTER_CENTER, &sample, font.clone(), ocol);
                        }
                    }
                    match &s.karaoke {
                        // Karaoke: second word highlighted („Füllen“: the first two)
                        Some(k) => {
                            let hc = Color32::from_rgba_unmultiplied(k.color[0], k.color[1], k.color[2], k.color[3]);
                            let (a, b) = word_ranges(&sample).get(1).map(|r| (if k.fill { 0 } else { r.start }, r.end)).unwrap_or((0, 0));
                            let mut job = egui::text::LayoutJob::default();
                            for (part, color) in [(&sample[..a], col), (&sample[a..b], hc), (&sample[b..], col)] {
                                job.append(part, 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
                            }
                            let g = tp.layout_job(job);
                            tp.galley(c - g.size() / 2.0, g, col);
                        }
                        None => {
                            tp.text(c, egui::Align2::CENTER_CENTER, &sample, font, col);
                        }
                    }
                    let name = crate::tr(p.name);
                    let hint = egui::FontId::proportional(theme::FS_HINT);
                    let shown = theme::elide(&name, r.width() - 16.0, |t| theme::text_width(ui.ctx(), t, hint.clone()));
                    pt.text(pos2(r.left() + 8.0, r.bottom() - 9.0), egui::Align2::LEFT_CENTER, &shown, hint.clone(), Color32::from_gray(200));
                    let stroke = if on {
                        egui::Stroke::new(2.0_f32, theme::ACCENT())
                    } else {
                        // without an idle frame (the area already stands out), only hover/selection
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

    /// List for editing: add, text, duration, merge, delete, find & replace
    pub fn captions_editor(&mut self, ui: &mut egui::Ui) {
        // the same query as the list below: the cache keeps its matches
        let items = self.caption_list(ui.ctx(), self.asset_cat.get("cap-q").map_or("", |q| q.as_str())).items;
        let fps = self.project.settings.fps;
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr(format!("Untertitel ({})", items.len()))).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // too narrow for the label: only the icon
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
        // Find & replace
        let mut q = self.asset_cat.get("cap-q").cloned().unwrap_or_default();
        let mut rep = self.asset_cat.get("cap-r").cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            // Width from the room: two fields (inner margin 2 × 4 each) + icon button + two gaps
            let sp = ui.spacing().item_spacing.x;
            let btn = (theme::text_width(ui.ctx(), icon::SWAP, egui::FontId::proportional(14.0)) + 2.0 * ui.spacing().button_padding.x).max(24.0);
            let w = ((ui.available_width() - btn - 2.0 * sp) / 2.0 - 8.0).max(30.0);
            let margin = egui::Margin::symmetric(4, 2);
            ui.add(egui::TextEdit::singleline(&mut q).hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS))).margin(margin).desired_width(w));
            ui.add(egui::TextEdit::singleline(&mut rep).hint_text(crate::tr("Ersetzen durch")).margin(margin).desired_width(w));
            let hits = if q.is_empty() { 0 } else { self.caption_list(ui.ctx(), &q).shown.len() };
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
        // Style or splitting of the chosen preset on all: only real caption tracks
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
                            let n = self.caption_clip(&text_of(c), c.start, c.duration, karaoke_words(c));
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
        let shown = self.caption_list(ui.ctx(), &q).shown;
        let ctx = ui.ctx().clone();
        // All rows are equally high (single-line text field): height measured on the last frame
        let hid = egui::Id::new("cap-row-h");
        let row_h = ctx.data(|d| d.get_temp::<f32>(hid)).unwrap_or(ui.spacing().interact_size.y + 8.0);
        let gap = 3.0 + ui.spacing().item_spacing.y;
        let stride = row_h + gap;
        // While playing, bring the caption under the playhead into view once when it changes
        let active = active_row(&items, &shown, self.playhead);
        let aid = egui::Id::new("cap-active");
        let follow = self.playing && active.is_some() && ctx.data(|d| d.get_temp::<Option<usize>>(aid)) != Some(active);
        ctx.data_mut(|d| d.insert_temp(aid, active));
        let reveal = self.caption_focus.and_then(|f| shown.iter().position(|&i| items[i].id == f)).or(active.filter(|_| follow));
        // Text field with focus: when its row scrolls out, egui drops the focus without `lost_focus`
        let eid = egui::Id::new("cap-editing");
        let editing = ctx.data(|d| d.get_temp::<Option<ClipId>>(eid)).flatten();
        let mut editing_now = None;
        let mut measured = None;
        let mut jump = None;
        let mut ops: Vec<EditOp> = Vec::new();
        let focus = self.caption_focus;
        let mut focused = false;
        // own scroll area down to the bottom edge: a new entry only scrolls the list,
        // the header with „Hinzufügen“ stays put
        let h = (ui.clip_rect().bottom() - ui.cursor().top() - 4.0).max(160.0);
        egui::ScrollArea::vertical().id_salt("cap-list").max_height(h).auto_shrink([false, true]).show_viewport(ui, |ui, vp| {
            ui.set_height((stride * shown.len() as f32 - gap).max(0.0));
            let (top, xs) = (ui.max_rect().top(), ui.max_rect().x_range());
            let row_rect = |k: usize| Rect::from_x_y_ranges(xs, top + k as f32 * stride..=top + k as f32 * stride + row_h);
            if let Some(k) = reveal {
                ui.scroll_to_rect(row_rect(k), None);
            }
            let first = ((vp.min.y / stride).floor().max(0.0) as usize).min(shown.len());
            let last = ((vp.max.y / stride).ceil().max(0.0) as usize).min(shown.len());
            // one child Ui per row: the auto IDs inside a row stay the same while scrolling
            ui.skip_ahead_auto_ids(first);
            for (k, &i) in shown.iter().enumerate().take(last).skip(first) {
                let c = &items[i];
                #[cfg(test)]
                tests::BUILT.with(|b| b.borrow_mut().push(c.id));
                let text = text_of(c);
                let next = items.get(i + 1);
                let active = c.contains(self.playhead);
                let row = ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect(k)), |ui| {
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
                                // Buttons on the right first, the text field takes the rest: nothing sticks out over the edge
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Apply the text change before merge/duration (as before, in reading order)
                                    let at = ops.len();
                                    if theme::icon_button(ui, icon::TRASH, "Löschen", true, false).clicked() {
                                        ops.push(EditOp::RemoveClip { clip: c.id });
                                    }
                                    if let Some(n) = next {
                                        if theme::icon_button(ui, icon::ARROWS_MERGE, "Mit dem nächsten zusammenführen", true, false).clicked() {
                                            let mut m = c.clone();
                                            // Word times of both parts together (the second from its start)
                                            let mut words = karaoke_words(c);
                                            words.extend(karaoke_words(n).into_iter().map(|w| w + (n.start - c.start)));
                                            if let ClipContent::Text(s) = &mut m.content {
                                                s.text = format!("{} {}", text, text_of(n));
                                                if let Some(k) = &mut s.karaoke {
                                                    k.words = words;
                                                }
                                            }
                                            ops.push(EditOp::RemoveClip { clip: n.id });
                                            ops.push(EditOp::SetClipTiming { clip: c.id, start: c.start, duration: n.end() - c.start, source_in: c.source_in });
                                            ops.push(EditOp::ReplaceClip { clip: m });
                                        }
                                    }
                                    // Duration (up to the next caption)
                                    let room = next.map(|n| (n.start - c.start).as_secs_f64()).unwrap_or(60.0);
                                    let mut d = c.duration.as_secs_f64();
                                    // the range must contain the current value: egui clamps it otherwise and
                                    // reports a change every frame (short captions: "Ungültige Dauer" per frame)
                                    let range = 0.2f64.min(d)..=room.max(d);
                                    if ui
                                        .add(egui::DragValue::new(&mut d).range(range).speed(0.02).custom_formatter(|v, _| crate::theme::dec(v, 1)).custom_parser(crate::theme::parse_dec).suffix(" s").update_while_editing(false))
                                        .on_hover_text(crate::tr("Dauer"))
                                        .changed()
                                    {
                                        ops.push(EditOp::SetClipTiming { clip: c.id, start: c.start, duration: Time::from_secs_f64(d), source_in: c.source_in });
                                    }
                                    let id = egui::Id::new(("cap-text", c.id));
                                    let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| text.clone());
                                    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id.with("edit")).desired_width(f32::INFINITY).frame(false));
                                    if r.has_focus() {
                                        editing_now = Some(c.id);
                                    }
                                    if focus == Some(c.id) {
                                        focused = true;
                                        // only scroll if needed: otherwise the „Hinzufügen“ button moves with every click
                                        r.scroll_to_me(None);
                                        r.request_focus();
                                        // select the whole placeholder text: typing replaces it
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
                        })
                });
                measured = Some(row.response.rect.height());
            }
        });
        if let Some(m) = measured.filter(|m| (m - row_h).abs() > 0.5) {
            ctx.data_mut(|d| d.insert_temp(hid, m));
            ctx.request_repaint();
        }
        // The edited row is gone from view: keep its text as if the field had lost focus
        if let Some(e) = editing.filter(|e| editing_now != Some(*e)) {
            let tid = egui::Id::new(("cap-text", e));
            if let Some(buf) = ctx.data_mut(|d| d.get_temp::<String>(tid)) {
                ctx.data_mut(|d| d.remove::<String>(tid));
                if let Some(mut n) = self.project.find_clip(e).map(|(_, c)| c.clone()).filter(|c| text_of(c) != buf) {
                    if let ClipContent::Text(s) = &mut n.content {
                        s.text = buf;
                    }
                    ops.push(EditOp::ReplaceClip { clip: n });
                }
            }
        }
        ctx.data_mut(|d| d.insert_temp(eid, editing_now));
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

    /// New caption at the playhead (up to the next one, at most 2 s), the text field gets focus
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
        let clip = self.caption_clip(&crate::tr("Neuer Untertitel"), at, dur, Vec::new());
        if let Some(OpOutput::Clip(id)) = self.apply(EditOp::AddClip { track, clip }) {
            self.select_only(Some(id));
            self.caption_focus = Some(id);
        }
    }

    /// Re-split existing captions into lines of the chosen length
    fn resegment_captions(&mut self, items: &[Clip]) {
        let segs: Vec<cl_analyze::asr::Segment> = items
            .iter()
            .map(|c| cl_analyze::asr::Segment {
                start: c.start.as_secs_f64(),
                end: c.end().as_secs_f64(),
                text: text_of(c),
                words: karaoke_words(c).iter().map(|w| (c.start + *w).as_secs_f64()).collect(),
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

/// Captions sorted by start and the positions of those matching the search, see [`App::caption_list`]
#[derive(Clone, Default)]
pub struct CapList {
    epoch: Option<u64>,
    q: Option<String>,
    pub items: std::sync::Arc<Vec<Clip>>,
    pub shown: std::sync::Arc<Vec<usize>>,
}

impl App {
    /// The captions list without collecting, sorting and cloning all captions every frame: rebuilt
    /// when the project changes (`project_epoch` grows with every synced change, undo and new
    /// project included), the matches when the search changes.
    pub fn caption_list(&self, ctx: &egui::Context, q: &str) -> CapList {
        let id = egui::Id::new("cap-list-cache");
        let mut l: CapList = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        if l.epoch != Some(self.project_epoch) {
            l = CapList { epoch: Some(self.project_epoch), items: std::sync::Arc::new(self.caption_clips()), ..Default::default() };
        }
        if l.q.as_deref() != Some(q) {
            let hit = |c: &Clip| matches!(&c.content, ClipContent::Text(s) if s.text.contains(q));
            l.shown = std::sync::Arc::new((0..l.items.len()).filter(|&i| hit(&l.items[i])).collect());
            l.q = Some(q.to_string());
            ctx.data_mut(|d| d.insert_temp(id, l.clone()));
        }
        l
    }
}

/// Position in `shown` of the caption under the playhead (`items` sorted by start)
fn active_row(items: &[Clip], shown: &[usize], at: Time) -> Option<usize> {
    let i = items.partition_point(|c| c.start <= at).checked_sub(1)?;
    if !items[i].contains(at) {
        return None;
    }
    shown.binary_search(&i).ok()
}

/// Clips that „Stil auf alle“ and „Neu aufteilen“ change: only on tracks named „Untertitel“.
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

/// Start per word of a karaoke caption (saved or spread evenly), otherwise empty
fn karaoke_words(c: &Clip) -> Vec<Time> {
    match &c.content {
        ClipContent::Text(TextStyle { text, karaoke: Some(k), .. }) => k.starts(text, c.duration),
        _ => Vec::new(),
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

    thread_local! {
        /// Captions whose row the list built in this thread (test-only counter)
        pub(super) static BUILT: std::cell::RefCell<Vec<ClipId>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    #[test]
    fn style_for_all_leaves_plain_text_clips_alone() {
        let mut p = Project::new("x");
        let text = |s: &str| Clip::new(ClipContent::Text(TextStyle { text: s.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(1.0));
        let mut plain = Track::new(TrackKind::Text);
        plain.clips.push(text("DER FILM"));
        p.timeline.tracks.push(plain);
        assert!(bulk_targets(&p).is_empty(), "a normal text track is not a caption track");
        let mut caps = Track::new(TrackKind::Text);
        caps.name = Some(TRACK_NAME.into());
        caps.clips.push(text("Hallo"));
        p.timeline.tracks.push(caps);
        assert_eq!(bulk_targets(&p).iter().map(text_of).collect::<Vec<_>>(), ["Hallo"]);
        // the list in the captions tab uses the same selection: only the caption track
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
            assert_eq!(PRESETS[i].id, *id, "existing styles stay");
        }
        // readable on light and dark images: outline or area
        for p in PRESETS {
            let s = (p.style)();
            assert!(s.outline.is_some() || s.background.is_some() || p.id == "cinema", "{}", p.id);
        }
    }

    /// Recognized word times land relative to the clip start in the karaoke caption; other styles
    /// store none (the file stays without a new field).
    #[test]
    fn karaoke_captions_keep_the_word_times() {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("k"));
        let seg = cl_analyze::asr::Segment { start: 1.0, end: 3.0, text: "Hallo Welt".into(), words: vec![1.0, 2.2] };
        let words = |app: &App| -> Vec<Option<Vec<i64>>> {
            app.caption_clips()
                .iter()
                .map(|c| match &c.content {
                    ClipContent::Text(s) => s.karaoke.as_ref().map(|k| k.words.iter().map(|w| w.as_millis()).collect()),
                    _ => None,
                })
                .collect()
        };
        app.settings.caption_preset = "karaoke".into();
        assert_eq!(app.add_caption_segments(std::slice::from_ref(&seg), Time::from_secs_f64(10.0)), 1);
        assert_eq!(words(&app), [Some(vec![0, 1200])]);
        // Style on all (karaoke fill) keeps the times
        let c = app.caption_clips()[0].clone();
        app.settings.caption_preset = "karaoke-fill".into();
        let n = app.caption_clip(&text_of(&c), c.start, c.duration, karaoke_words(&c));
        let ClipContent::Text(s) = &n.content else { panic!() };
        assert_eq!(s.karaoke.as_ref().map(|k| (k.fill, k.words.len())), Some((true, 2)));
        app.set_doc(Project::new("k"));
        app.settings.caption_preset = "classic".into();
        app.add_caption_segments(&[seg], Time::ZERO);
        assert_eq!(words(&app), [None]);
        assert!(!app.project.to_json().contains("karaoke"));
    }

    /// Fast speech gives captions shorter than the duration field's 0.2 s minimum, ending where the
    /// next one starts. Showing the list clamped them to 0.2 s every frame: against the neighbour
    /// that failed with "Ungültige Dauer" ~20× per second, otherwise it edited them silently.
    #[test]
    fn captions_list_leaves_short_captions_alone() {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("k"));
        let seg = |a: f64, b: f64, t: &str| cl_analyze::asr::Segment { start: a, end: b, text: t.into(), words: vec![a] };
        app.add_caption_segments(&[seg(1.0, 1.08, "Ja."), seg(1.08, 2.0, "Genau so."), seg(5.0, 5.1, "Gut")], Time::ZERO);
        let steps = app.doc.lock().unwrap().undo_entries().len();
        let ctx = egui::Context::default();
        for _ in 0..5 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.captions_editor(ui));
            });
        }
        let errs: Vec<&str> = app.toasts.iter().filter(|t| t.2).map(|t| t.0.as_str()).collect();
        assert!(errs.is_empty(), "error toasts while only showing the list: {errs:?}");
        assert_eq!(app.doc.lock().unwrap().undo_entries().len(), steps, "showing the list must not add history steps");
        let durs: Vec<i64> = app.caption_clips().iter().map(|c| c.duration.as_millis()).collect();
        assert_eq!(durs, [80, 920, 100], "durations unchanged");
    }

    /// A long recognition (≈2,000 lines) lands complete and as ONE undo step (track included)
    #[test]
    fn many_captions_are_one_undo_step() {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("k"));
        let steps = app.doc.lock().unwrap().undo_entries().len();
        let before = app.doc.lock().unwrap().project().clone();
        let segs: Vec<cl_analyze::asr::Segment> = (0..2000)
            .map(|i| cl_analyze::asr::Segment { start: i as f64 * 0.75, end: i as f64 * 0.75 + 0.75, text: format!("Wort{i}"), words: Vec::new() })
            .collect();
        assert_eq!(app.add_caption_segments(&segs, Time::ZERO), 2000);
        assert_eq!(app.caption_clips().len(), 2000);
        assert_eq!(app.doc.lock().unwrap().undo_entries().len(), steps + 1, "one undo step");
        app.undo();
        assert!(app.doc.lock().unwrap().project() == &before, "undo removes the captions and their track");
    }

    /// A line whose length rounds to 0 µs must not fail the whole batch (no captions at all)
    #[test]
    fn caption_rounding_to_zero_is_skipped() {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("k"));
        let seg = |a: f64, b: f64| cl_analyze::asr::Segment { start: a, end: b, text: "x".into(), words: Vec::new() };
        assert_eq!(app.add_caption_segments(&[seg(1.0, 1.0000001), seg(2.0, 3.0)], Time::ZERO), 1);
        assert_eq!(app.caption_clips().len(), 1);
        assert!(!app.toasts.iter().any(|t| t.2), "no error");
    }

    /// App with `n` captions of 0.75 s each ("Wort0", "Wort1", ...)
    fn app_with_captions(n: usize) -> crate::saving::tests::TestApp {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("k"));
        let segs: Vec<cl_analyze::asr::Segment> = (0..n)
            .map(|i| cl_analyze::asr::Segment { start: i as f64 * 0.75, end: i as f64 * 0.75 + 0.75, text: format!("Wort{i}"), words: Vec::new() })
            .collect();
        assert_eq!(app.add_caption_segments(&segs, Time::ZERO), n);
        app
    }

    /// One headless frame of the captions list in a 400 × 760 px window (list ≈ 600 px high)
    fn list_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 760.0))), events, ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.captions_editor(ui));
        });
    }

    /// Rows built in one frame
    fn built(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<ClipId> {
        BUILT.with(|b| b.borrow_mut().clear());
        list_frame(app, ctx, events);
        BUILT.with(|b| b.take())
    }

    fn wheel(dy: f32) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos2(200.0, 500.0)),
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, dy), modifiers: Default::default() },
        ]
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    /// 1,991 captions made the whole UI sluggish: the list built every row every frame. With 5,000
    /// captions in a ≈600 px list only the visible rows are built; scrolling, search, editing and
    /// following the playhead keep working.
    #[test]
    fn captions_list_builds_only_visible_rows() {
        let mut app = app_with_captions(5000);
        let ctx = egui::Context::default();
        let ids: Vec<ClipId> = app.caption_clips().iter().map(|c| c.id).collect();
        for _ in 0..3 {
            list_frame(&mut app, &ctx, Vec::new());
        }
        let rows = built(&mut app, &ctx, Vec::new());
        assert!(rows.len() >= 5 && rows.len() <= 30, "rows built per frame with 5,000 captions: {}", rows.len());
        assert_eq!(rows[0], ids[0], "the list starts at the first caption");

        // scroll to the end: the last caption is shown
        list_frame(&mut app, &ctx, wheel(-1.0e7));
        let mut rows = Vec::new();
        for _ in 0..60 {
            rows = built(&mut app, &ctx, Vec::new());
        }
        assert!(rows.len() <= 30 && rows.contains(&ids[4999]), "after scrolling down: {} rows, last caption shown: {}", rows.len(), rows.contains(&ids[4999]));

        // search: only the matches
        app.asset_cat.insert("cap-q", "Wort123".into());
        let mut rows = Vec::new();
        for _ in 0..30 {
            rows = built(&mut app, &ctx, Vec::new());
        }
        let want: Vec<ClipId> = std::iter::once(123).chain(1230..1240).map(|i| ids[i]).collect();
        assert_eq!(rows, want, "search shows exactly the 11 matches");
        app.asset_cat.insert("cap-q", String::new());

        // the caption at the playhead comes into view while playing
        app.playing = true;
        app.playhead = Time::from_secs_f64(3000.0 * 0.75 + 0.1);
        let mut rows = Vec::new();
        for _ in 0..60 {
            rows = built(&mut app, &ctx, Vec::new());
        }
        assert!(rows.contains(&ids[3000]), "caption 3000 under the playhead is shown while playing");
        app.playing = false;

        // editing a visible row: one undo step, the right clip changes
        let steps = app.doc.lock().unwrap().undo_entries().len();
        app.caption_focus = Some(ids[3002]);
        for _ in 0..30 {
            list_frame(&mut app, &ctx, Vec::new());
        }
        list_frame(&mut app, &ctx, vec![egui::Event::Text("Neu".into())]);
        list_frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);
        list_frame(&mut app, &ctx, Vec::new());
        let texts: Vec<String> = app.caption_clips().iter().map(text_of).collect();
        assert_eq!(texts[3002], "Neu");
        assert_eq!((texts[3001].as_str(), texts[3003].as_str()), ("Wort3001", "Wort3003"), "neighbours unchanged");
        assert_eq!(app.doc.lock().unwrap().undo_entries().len(), steps + 1, "one undo step");

        // typing, then scrolling the row out of view keeps the text (egui drops the focus silently)
        app.caption_focus = Some(ids[3003]);
        for _ in 0..30 {
            list_frame(&mut app, &ctx, Vec::new());
        }
        list_frame(&mut app, &ctx, vec![egui::Event::Text("Weg".into())]);
        list_frame(&mut app, &ctx, wheel(1.0e7));
        for _ in 0..60 {
            list_frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(text_of(&app.caption_clips()[3003]), "Weg", "text typed before scrolling away is kept");
        assert_eq!(app.doc.lock().unwrap().undo_entries().len(), steps + 2);
        assert!(!app.toasts.iter().any(|t| t.2), "no error");
    }

    /// Measurement: time per frame of the captions list with 2,000 and 5,000 captions.
    /// `cargo test --release -p cl-app captions_list_measurement -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn captions_list_measurement() {
        for n in [2000, 5000] {
            let mut app = app_with_captions(n);
            let ctx = egui::Context::default();
            for _ in 0..3 {
                list_frame(&mut app, &ctx, Vec::new());
            }
            let frames = 30;
            let t = std::time::Instant::now();
            for _ in 0..frames {
                list_frame(&mut app, &ctx, Vec::new());
            }
            eprintln!("{n} captions: {:.2} ms per frame", t.elapsed().as_secs_f64() * 1000.0 / frames as f64);
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
