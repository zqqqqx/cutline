//! Zeitleiste:
//! - kurzer Klick ins Leere setzt den Playhead (begrenzt aufs Projektende), gedrückt halten
//!   und ziehen zieht einen Auswahlrahmen auf; Lineal und Playhead-Griff scrubben
//! - Playhead und Clips rasten an Clipkanten (alle Spuren) und am Playhead ein
//! - Clips ziehen: auf andere Spuren derselben Art, ins Leere darüber/darunter = neue Spur
//! - Strg/Umschalt + Klick = Mehrfachauswahl, Rechtsklick-Menüs für Clip, Spur und Leerraum
//! - Übergänge als Block auf dem Schnitt, Keyframe-Rauten, Live-Waveform beim Einsprechen
//! - Mausrad scrollt die Spuren senkrecht, Umschalt+Rad waagerecht, Strg+Rad zoomt

use std::collections::{HashMap, HashSet};

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};

use crate::app::{App, DragAsset};
use crate::preview::WAVE_PER_SEC;
use crate::theme::{self, icon};
use crate::widgets::Slider;

const HEADER_W: f32 = 172.0;
const RULER_H: f32 = 22.0;
const ROW_H: f32 = 34.0;
const MAIN_ROW_H: f32 = 58.0;
const AUDIO_ROW_H: f32 = 42.0;
/// Höhe einer eingeklappten Spur
const COLLAPSED_H: f32 = 16.0;
const ROW_GAP: f32 = 5.0;
const EDGE_PX: f32 = 7.0;
const SNAP_PX: f32 = 9.0;
const BAND_H: f32 = 14.0;
const REC_ROW: usize = usize::MAX;

#[derive(Clone, PartialEq)]
enum Drag {
    None,
    Scrub,
    /// `ids`: alle bewegten Clips mit Startzeit; `grab` = Abstand Zeiger ↔ Clipanfang
    Move {
        clip: ClipId,
        grab: f64,
        ids: Vec<(ClipId, f64)>,
    },
    Trim {
        clip: ClipId,
        edge: Edge,
    },
    /// Alt+Kante zwischen zwei Clips: Schnitt verschieben
    Roll {
        left: ClipId,
        right: ClipId,
    },
    /// Alt+Clip: Quellausschnitt verschieben (`src` = Quellbeginn beim Start, `x` = Startposition)
    Slip {
        clip: ClipId,
        src: f64,
        x: f32,
    },
    Marquee {
        origin: Pos2,
        additive: bool,
    },
    /// Fade-Griff oder Lautstärkelinie auf einem Clip mit Ton (`value`: Sekunden bzw. dB)
    Audio {
        clip: ClipId,
        handle: AudioHandle,
        value: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum AudioHandle {
    FadeIn,
    FadeOut,
    Volume,
}

/// Lautstärkelinie: −30 dB unten bis +12 dB oben (0 dB bei gut zwei Dritteln)
const VOL_MIN: f32 = -30.0;
const VOL_MAX: f32 = 12.0;
fn vol_y(body: Rect, db: f32) -> f32 {
    let n = ((db - VOL_MIN) / (VOL_MAX - VOL_MIN)).clamp(0.0, 1.0);
    body.bottom() - 3.0 - n * (body.height() - 6.0)
}
fn y_vol(body: Rect, y: f32) -> f32 {
    let n = ((body.bottom() - 3.0 - y) / (body.height() - 6.0).max(1.0)).clamp(0.0, 1.0);
    let db = VOL_MIN + n * (VOL_MAX - VOL_MIN);
    // bei 0 dB leicht einrasten
    if db.abs() < 0.8 { 0.0 } else { (db * 2.0).round() / 2.0 }
}

/// Werkzeug in der Zeitleiste (wie in CapCut: A = Auswählen, B = Teilen)
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Tool {
    #[default]
    Select,
    Blade,
    SelectLeft,
    SelectRight,
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str, &'static str); 4] = [
        (Tool::Select, icon::CURSOR, "Auswählen", "A"),
        (Tool::Blade, icon::KNIFE, "Teilen", "B"),
        (Tool::SelectLeft, icon::ARROW_FAT_LINES_LEFT, "Alle Clips links auswählen", ""),
        (Tool::SelectRight, icon::ARROW_FAT_LINES_RIGHT, "Alle Clips rechts auswählen", ""),
    ];
    fn icon(self) -> &'static str {
        Self::ALL.iter().find(|t| t.0 == self).map(|t| t.1).unwrap_or(icon::CURSOR)
    }
}

/// Spurarten zum Hinzufügen (Symbol, Name)
pub const TRACK_KINDS: [(TrackKind, &str); 5] = [
    (TrackKind::Video, "Videospur"),
    (TrackKind::Text, "Textspur"),
    (TrackKind::Effect, "Effektspur"),
    (TrackKind::Filter, "Filterspur"),
    (TrackKind::Audio, "Audiospur"),
];

#[derive(Clone, Copy, PartialEq)]
enum CtxTarget {
    Clip(ClipId),
    Track(TrackId),
    Empty(Option<TrackId>),
    Transition(ClipId),
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Same,
    Track(usize),
    NewTop,
    NewBottom,
}

pub struct TimelineState {
    pub pps: f32,
    pub scroll: f64,
    pub vscroll: f32,
    drag: Drag,
    ctx: Option<CtxTarget>,
    /// Höhenfaktor je Spur (Rechtsklick → Spurhöhe)
    pub heights: HashMap<TrackId, f32>,
    /// Eingeklappte Spuren (schmal, bleiben aber sichtbar)
    pub collapsed: std::collections::HashSet<TrackId>,
    last_click: Option<Pos2>,
    /// Playhead im letzten Bild (Sprünge per Taste → Ansicht folgt)
    last_playhead: Option<Time>,
    pub tool: Tool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            pps: 60.0,
            scroll: 0.0,
            vscroll: 0.0,
            drag: Drag::None,
            ctx: None,
            heights: HashMap::new(),
            collapsed: Default::default(),
            last_click: None,
            last_playhead: None,
            tool: Tool::Select,
        }
    }
}

struct Row {
    track: usize,
    rect: Rect,
}

fn menu_item(ui: &mut egui::Ui, glyph: &str, text: &str, keys: &str) -> bool {
    let r = ui.add(
        theme::menu_row(crate::tr(format!("{glyph}   {text}")))
            .keys(keys)
            .min_width(210.0),
    );
    if r.clicked() {
        ui.close_menu();
        return true;
    }
    false
}

fn set_audio_handle(c: &mut Clip, h: AudioHandle, v: f32) {
    let dur = c.duration.as_secs_f64();
    match h {
        AudioHandle::FadeIn => {
            c.fade_in = Time::from_secs_f64((v as f64).clamp(0.0, (dur - c.fade_out.as_secs_f64()).max(0.0)))
        }
        AudioHandle::FadeOut => {
            c.fade_out = Time::from_secs_f64((v as f64).clamp(0.0, (dur - c.fade_in.as_secs_f64()).max(0.0)))
        }
        AudioHandle::Volume => c.volume_db = v,
    }
}

impl App {
    /// Fade/Lautstärke aus der Zeitleiste übernehmen – bei Mehrfachauswahl für alle Clips mit Ton.
    fn apply_audio_handle(&mut self, clip: ClipId, h: AudioHandle, v: f32) {
        let ids = if self.is_selected(clip) { self.selection() } else { vec![clip] };
        let ops: Vec<EditOp> = ids
            .iter()
            .filter_map(|id| self.project.find_clip(*id).map(|(_, c)| c.clone()))
            .filter(|c| match &c.content {
                ClipContent::Media { media, kind } => {
                    *kind == MediaKind::Audio || self.project.media(*media).is_some_and(|m| m.has_audio)
                }
                _ => false,
            })
            .map(|mut c| {
                set_audio_handle(&mut c, h, v);
                EditOp::ReplaceClip { clip: c }
            })
            .collect();
        self.apply_batch(ops);
    }

    fn row_height(&self, t: &Track) -> f32 {
        if self.timeline.collapsed.contains(&t.id) {
            return COLLAPSED_H;
        }
        let base = if t.main {
            MAIN_ROW_H
        } else if t.kind == TrackKind::Audio {
            AUDIO_ROW_H
        } else {
            ROW_H
        };
        base * self.timeline.heights.get(&t.id).copied().unwrap_or(1.0)
    }

    pub fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        self.timeline_toolbar(ui);
        ui.add_space(2.0);
        self.timeline_canvas(ui);
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.spacing_mut().item_spacing.x = 1.0;
            let (cu, cr) = {
                let d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
                (d.can_undo(), d.can_redo())
            };
            if theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, format!("{}  (Strg+Z)", self.t("undo")), cu, false).clicked() {
                self.undo();
            }
            if theme::icon_button(ui, icon::ARROW_CLOCKWISE, format!("{}  (Strg+Umschalt+Z)", self.t("redo")), cr, false).clicked() {
                self.redo();
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            // Werkzeug wählen (Auswählen, Teilen, alles links/rechts)
            let cur = self.timeline.tool;
            ui.menu_button(
                RichText::new(format!("{} {}", cur.icon(), icon::CARET_DOWN)).size(13.0),
                |ui| {
                    for (t, g, name, key) in Tool::ALL {
                        let mark = if cur == t { icon::CHECK } else { " " };
                        let r = ui.add(
                            theme::menu_row(format!("{}   {mark}", crate::tr(format!("{g}   {name}"))))
                                .keys(key)
                                .min_width(250.0),
                        );
                        if r.clicked() {
                            self.timeline.tool = t;
                            ui.close_menu();
                        }
                    }
                },
            )
            .response
            .on_hover_text(crate::tr("Werkzeug"));
            let tools = ui.menu_button(egui::RichText::new(crate::tr(format!("{}  Quick Actions", icon::LIGHTNING))).size(12.0), |ui| self.tools_menu(ui)).response;
            self.tour_rects.insert("tools", tools.rect);
            tools.on_hover_text(crate::tr("Pausen entfernen, Füllwörter, Ducking, Multicam, Hochkant …"));
            ui.menu_button(
                RichText::new(crate::tr(format!("{}  Spur", icon::PLUS))).size(12.0),
                |ui| {
                    for (k, name) in TRACK_KINDS {
                        if menu_item(ui, theme::track_icon(k), name, "") {
                            // ganz oben (Bildspuren) bzw. ganz unten (Ton)
                            let index = self.project.timeline.tracks.len();
                            self.apply(EditOp::InsertTrack { index, kind: k });
                        }
                    }
                },
            )
            .response
            .on_hover_text(crate::tr("Neue Spur hinzufügen"));
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            let has = !self.project.timeline.tracks.is_empty();
            let sel = self.selected.is_some() || !self.sel_extra.is_empty() || self.selected_transition.is_some();
            if theme::icon_button(ui, icon::SCISSORS, "Teilen  (Strg+B)", has, false).clicked() {
                self.split_at_playhead();
            }
            if theme::icon_button(ui, icon::ARROW_LINE_LEFT, "Links vom Playhead löschen  (Q)", has, false).clicked() {
                self.delete_side(true);
            }
            if theme::icon_button(ui, icon::ARROW_LINE_RIGHT, "Rechts vom Playhead löschen  (W)", has, false).clicked() {
                self.delete_side(false);
            }
            if theme::icon_button(ui, icon::TRASH, "Löschen  (Entf)", sel, false).clicked() {
                self.delete_selected();
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            let video_sel = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| matches!(c.content, ClipContent::Media { kind: MediaKind::Video, .. }));
            let visual_sel = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| c.kind() == TrackKind::Video);
            if theme::icon_button(ui, icon::SNOWFLAKE, "Standbild (3 s am Playhead)", has, false).clicked() {
                self.freeze_frame();
            }
            let rev = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| c.fx.reverse);
            if theme::icon_button(ui, icon::ARROWS_LEFT_RIGHT, "Rückwärts abspielen", video_sel, rev).clicked() {
                self.modify_selection(|c| c.fx.reverse = !c.fx.reverse);
            }
            if theme::icon_button(ui, icon::FLIP_HORIZONTAL, "Spiegeln", visual_sel, false).clicked() {
                self.modify_selection(|c| c.fx.flip_h = !c.fx.flip_h);
            }
            if theme::icon_button(ui, icon::ARROW_CLOCKWISE, "90° drehen", visual_sel, false).clicked() {
                self.modify_selection(|c| c.transform.rotation_deg = ((c.transform.rotation_deg + 90.0 + 180.0).rem_euclid(360.0)) - 180.0);
            }
            if theme::icon_button(ui, icon::WAVEFORM, "Ton extrahieren", video_sel, false).clicked() {
                if let Some(s) = self.selected {
                    self.extract_audio(s);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.add_space(4.0);
                if theme::icon_button(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, "Ganze Zeitleiste zeigen", true, false).clicked() {
                    let w = ui.ctx().screen_rect().width() * 0.6;
                    let d = self.project.duration().as_secs_f64().max(1.0);
                    self.timeline.pps = (w / d as f32).clamp(2.0, 400.0);
                    self.timeline.scroll = 0.0;
                }
                if theme::icon_button(ui, icon::PLUS, "Vergrößern (Strg+Mausrad)", true, false).clicked() {
                    self.timeline.pps = (self.timeline.pps * 1.25).min(400.0);
                }
                ui.allocate_ui(vec2(90.0, 20.0), |ui| {
                    let mut z = self.timeline.pps;
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let r = ui.allocate_ui(vec2(90.0, 18.0), |ui| {
                        let (rect, resp) = ui.allocate_exact_size(vec2(90.0, 18.0), Sense::click_and_drag());
                        let track = Rect::from_center_size(rect.center(), vec2(80.0, 2.0));
                        let n = ((z.ln() - 2f32.ln()) / (400f32.ln() - 2f32.ln())).clamp(0.0, 1.0);
                        if let Some(p) = resp.interact_pointer_pos() {
                            let nn = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
                            z = (2f32.ln() + nn * (400f32.ln() - 2f32.ln())).exp();
                        }
                        ui.painter().rect_filled(track, 1.0, theme::BG_HOVER());
                        ui.painter().rect_filled(Rect::from_min_max(track.min, pos2(track.left() + n * track.width(), track.max.y)), 1.0, theme::TEXT_DIM());
                        ui.painter().circle_filled(pos2(track.left() + n * track.width(), track.center().y), if resp.hovered() { 5.5 } else { 4.5 }, theme::ACCENT());
                    });
                    let _ = r;
                    self.timeline.pps = z;
                });
                if theme::icon_button(ui, icon::MINUS, "Verkleinern", true, false).clicked() {
                    self.timeline.pps = (self.timeline.pps / 1.25).max(2.0);
                }
                ui.add(egui::Separator::default().vertical().spacing(10.0));
                let s = self.project.settings.clone();
                if theme::icon_button(ui, icon::ARROWS_IN_LINE_HORIZONTAL, format!("{}  (N)", self.t("snapping")), true, s.snapping).clicked() {
                    let mut n = s.clone();
                    n.snapping = !n.snapping;
                    self.apply(EditOp::SetSettings { settings: n });
                }
                if theme::icon_button(ui, icon::MAGNET, format!("{}  (P)", self.t("magnet")), true, s.main_track_magnet).clicked() {
                    let mut n = s.clone();
                    n.main_track_magnet = !n.main_track_magnet;
                    self.apply(EditOp::SetSettings { settings: n });
                }
                ui.add(egui::Separator::default().vertical().spacing(10.0));
                let rec = self.voiceover.recorder.is_some();
                let mic = ui.add(
                    egui::Button::new(RichText::new(icon::MICROPHONE).size(14.0).color(if rec { theme::DANGER } else if self.voiceover.open { theme::ACCENT() } else { theme::TEXT_DIM() }))
                        .fill(if self.voiceover.open || rec { theme::overlay(22) } else { Color32::TRANSPARENT })
                        .corner_radius(5)
                        .min_size(vec2(24.0, 22.0)),
                );
                if mic.on_hover_text(crate::tr(format!("{}  (R)", self.t("voiceover")))).clicked() {
                    self.voiceover.open = !self.voiceover.open;
                }
            });
        });
    }

    fn timeline_canvas(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let x0 = rect.left() + HEADER_W;
        let pps = self.timeline.pps;
        let fps = self.project.settings.fps;
        let snapping = self.project.settings.snapping;
        let tracks = self.project.timeline.tracks.clone();
        let total_t = self.project.duration().as_secs_f64();
        let recording = self.voiceover.recorder.as_ref().map(|r| {
            (
                r.start_at.as_secs_f64(),
                r.elapsed().as_secs_f64(),
                r.peaks(),
            )
        });

        // ---------- Mausrad ----------
        let ruler = Rect::from_min_max(
            pos2(x0, rect.top()),
            pos2(rect.right(), rect.top() + RULER_H),
        );
        let lanes = Rect::from_min_max(pos2(rect.left(), ruler.bottom()), rect.max);
        let mut order: Vec<usize> = (0..tracks.len())
            .filter(|i| tracks[*i].kind != TrackKind::Audio)
            .rev()
            .collect();
        order.extend((0..tracks.len()).filter(|i| tracks[*i].kind == TrackKind::Audio));
        if recording.is_some() {
            order.push(REC_ROW);
        }
        let heights: Vec<f32> = order
            .iter()
            .map(|i| {
                if *i == REC_ROW {
                    AUDIO_ROW_H
                } else {
                    self.row_height(&tracks[*i])
                }
            })
            .collect();
        let content_h: f32 = heights.iter().map(|h| h + ROW_GAP).sum::<f32>() + 24.0;
        let view_h = lanes.height();
        let overflow = (content_h - view_h).max(0.0);
        if resp.hovered() || resp.dnd_hover_payload::<DragAsset>().is_some() {
            let (dy, dx, ctrl, shift, pointer) = ui.input(|i| {
                (
                    i.raw_scroll_delta.y,
                    i.raw_scroll_delta.x,
                    i.modifiers.command,
                    i.modifiers.shift,
                    i.pointer.hover_pos(),
                )
            });
            let xt0 = |x: f32| (((x - x0) / pps) as f64 + self.timeline.scroll).max(0.0);
            if ctrl && dy != 0.0 {
                let anchor = pointer.map(|p| xt0(p.x)).unwrap_or(self.timeline.scroll);
                self.timeline.pps = (pps * (1.0 + dy * 0.0015)).clamp(2.0, 400.0);
                self.timeline.scroll = (anchor
                    - ((pointer.map(|p| p.x).unwrap_or(x0) - x0) / self.timeline.pps) as f64)
                    .max(0.0);
            } else if shift || overflow <= 0.0 || pointer.is_some_and(|p| p.y < ruler.bottom()) {
                let d = if dx != 0.0 { dx } else { dy };
                if d != 0.0 {
                    self.timeline.scroll = (self.timeline.scroll - (d / pps) as f64).max(0.0);
                }
            } else {
                if dy != 0.0 {
                    self.timeline.vscroll = (self.timeline.vscroll - dy).clamp(0.0, overflow);
                }
                if dx != 0.0 {
                    self.timeline.scroll = (self.timeline.scroll - (dx / pps) as f64).max(0.0);
                }
            }
        }
        // Beim Ziehen (Playhead, Clips, Kanten) an den Rand: Zeitleiste rollt in diese Richtung,
        // je näher am Rand, desto schneller – auch wenn die Maus dort stillsteht.
        if resp.dragged() && !matches!(self.timeline.drag, Drag::None | Drag::Slip { .. }) {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                let edge = 48.0;
                let (l, r) = (x0, lanes.right());
                let v = if p.x < l + edge {
                    -((l + edge - p.x) / edge).min(3.0)
                } else if p.x > r - edge {
                    ((p.x - (r - edge)) / edge).min(3.0)
                } else {
                    0.0
                };
                if v != 0.0 {
                    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05) as f64;
                    let visible = ((r - x0) / pps) as f64;
                    self.timeline.scroll =
                        (self.timeline.scroll + v as f64 * visible * 0.8 * dt).max(0.0);
                    ui.ctx().request_repaint();
                }
            }
        }
        // Springt der Playhead (Pos1/Ende, Marker, Schnitte …) aus dem sichtbaren Bereich,
        // rollt die Ansicht hinterher.
        if !resp.dragged() && !self.playing && self.timeline.last_playhead != Some(self.playhead) {
            let t = self.playhead.as_secs_f64();
            let visible = ((lanes.right() - x0) / pps) as f64;
            if t < self.timeline.scroll || t > self.timeline.scroll + visible {
                self.timeline.scroll = (t - visible * 0.3).max(0.0);
            }
        }
        self.timeline.last_playhead = Some(self.playhead);
        self.timeline.vscroll = self.timeline.vscroll.clamp(0.0, overflow);
        let pps = self.timeline.pps;
        let scroll = self.timeline.scroll;
        let tx = |t: f64| x0 + ((t - scroll) as f32) * pps;
        let xt = |x: f32| (((x - x0) / pps) as f64 + scroll).max(0.0);

        // ---------- Zeilen ----------
        let mut y = if overflow > 0.0 {
            lanes.top() + 8.0 - self.timeline.vscroll
        } else {
            lanes.top() + 8.0 + ((view_h - content_h) * 0.4).max(0.0)
        };
        let mut rows = Vec::new();
        for (k, i) in order.iter().enumerate() {
            rows.push(Row {
                track: *i,
                rect: Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), heights[k])),
            });
            y += heights[k] + ROW_GAP;
        }
        let row_at = |p: Pos2| {
            rows.iter()
                .find(|r| {
                    p.y >= r.rect.top() - ROW_GAP / 2.0
                        && p.y < r.rect.bottom() + ROW_GAP / 2.0
                        && r.track != REC_ROW
                })
                .map(|r| r.track)
        };

        let lane_paint = painter.with_clip_rect(lanes);
        for row in &rows {
            let lane = Rect::from_min_max(pos2(x0, row.rect.top()), row.rect.max);
            let fill = if row.track != REC_ROW && tracks[row.track].main {
                theme::BG_RAISED()
            } else {
                theme::BG_INPUT()
            };
            lane_paint.rect_filled(lane, 5.0, fill);
        }

        // ---------- Treffer ----------
        let hit = |p: Pos2| -> Option<(ClipId, Option<Edge>, usize)> {
            if p.x < x0 || !lanes.contains(p) {
                return None;
            }
            for row in rows.iter().filter(|r| r.track != REC_ROW) {
                if !row.rect.contains(p) {
                    continue;
                }
                for c in &tracks[row.track].clips {
                    let (a, b) = (tx(c.start.as_secs_f64()), tx(c.end().as_secs_f64()));
                    if p.x >= a - 2.0 && p.x <= b + 2.0 {
                        let w = b - a;
                        let edge = if w > 3.0 * EDGE_PX && (p.x - a).abs() <= EDGE_PX {
                            Some(Edge::Start)
                        } else if (p.x - b).abs() <= EDGE_PX {
                            Some(Edge::End)
                        } else {
                            None
                        };
                        return Some((c.id, edge, row.track));
                    }
                }
            }
            None
        };
        // Übergangsblöcke
        let mut tblocks: Vec<(ClipId, Rect)> = Vec::new();
        for row in rows.iter().filter(|r| r.track != REC_ROW) {
            let tr = &tracks[row.track];
            for (i, c) in tr.clips.iter().enumerate() {
                let Some(t) = &c.transition_out else { continue };
                if !tr
                    .clips
                    .get(i + 1)
                    .is_some_and(|n| (n.start.0 - c.end().0).abs() < 2000)
                {
                    continue;
                }
                let w = (t.duration.as_secs_f64() as f32 * pps).clamp(16.0, 56.0);
                let r = Rect::from_center_size(
                    pos2(
                        tx(c.end().as_secs_f64()),
                        row.rect.center().y + BAND_H / 2.0,
                    ),
                    vec2(w, 18.0_f32.min(row.rect.height() - BAND_H - 4.0)),
                );
                tblocks.push((c.id, r));
            }
        }
        let hit_block = |p: Pos2| {
            tblocks
                .iter()
                .find(|(_, r)| r.expand(2.0).contains(p))
                .map(|(c, _)| *c)
        };

        let ph = self.playhead.as_secs_f64();
        let snap_cands = |ignore: &[ClipId]| -> Vec<f64> {
            let mut v = vec![0.0, ph];
            for tr in &tracks {
                for c in tr.clips.iter().filter(|c| !ignore.contains(&c.id)) {
                    v.push(c.start.as_secs_f64());
                    v.push(c.end().as_secs_f64());
                    v.extend(c.beat_times().into_iter().map(|b| b.as_secs_f64()));
                }
            }
            v
        };
        let snap_to = |t: f64, cands: &[f64]| -> Option<f64> {
            if !snapping {
                return None;
            }
            cands
                .iter()
                .copied()
                .filter(|c| ((c - t).abs() as f32) * pps < SNAP_PX)
                .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
        };
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let hovered_clip = resp.hover_pos().and_then(|p| hit(p).map(|h| h.0));
        let head_rect = Rect::from_center_size(pos2(tx(ph), rect.top() + 9.0), vec2(14.0, 18.0));
        // Clips mit Ton: Fade-Griffe oben an den Ecken, Lautstärkelinie quer über den Clip
        let audio_media: HashSet<MediaId> = self
            .project
            .media
            .iter()
            .filter(|m| m.has_audio || m.kind == MediaKind::Audio)
            .map(|m| m.id)
            .collect();
        let has_audio = |c: &Clip| matches!(&c.content, ClipContent::Media { media, .. } if audio_media.contains(media));
        let clip_body = |id: ClipId| -> Option<(Rect, Clip)> {
            for row in rows.iter().filter(|r| r.track != REC_ROW) {
                if let Some(c) = tracks[row.track].clips.iter().find(|c| c.id == id) {
                    let cr = Rect::from_min_max(
                        pos2(tx(c.start.as_secs_f64()), row.rect.top()),
                        pos2(tx(c.end().as_secs_f64()), row.rect.bottom()),
                    );
                    let body = Rect::from_min_max(pos2(cr.left(), cr.top() + BAND_H), cr.max);
                    return Some((body, c.clone()));
                }
            }
            None
        };
        let audio_hit = |p: Pos2| -> Option<(ClipId, AudioHandle, f32)> {
            // Fade-Griffe sitzen in den oberen Ecken und gehen dort dem Trimmen an der Kante vor
            let (id, edge, _) = hit(p)?;
            let (body, c) = clip_body(id)?;
            if !has_audio(&c) || body.width() < 24.0 || body.height() < 14.0 {
                return None;
            }
            let fi = pos2((body.left() + c.fade_in.as_secs_f64() as f32 * pps).max(body.left() + 5.0), body.top() + 5.0);
            let fo = pos2((body.right() - c.fade_out.as_secs_f64() as f32 * pps).min(body.right() - 5.0), body.top() + 5.0);
            if p.distance(fi) < 7.0 {
                return Some((id, AudioHandle::FadeIn, c.fade_in.as_secs_f64() as f32));
            }
            if p.distance(fo) < 7.0 {
                return Some((id, AudioHandle::FadeOut, c.fade_out.as_secs_f64() as f32));
            }
            if edge.is_none() && (p.y - vol_y(body, c.volume_db)).abs() < 4.0 {
                return Some((id, AudioHandle::Volume, c.volume_db));
            }
            None
        };

        if let Some(p) = resp.hover_pos() {
            if let Some((_, h, _)) = audio_hit(p) {
                ui.ctx().set_cursor_icon(if h == AudioHandle::Volume {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
            } else if matches!(hit(p), Some((_, Some(_), _))) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if head_rect.contains(p) || (p.x - tx(ph)).abs() < 4.0 && p.x >= x0 {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            }
        }

        // Klick in die Zeitleiste beendet Texteingaben (sonst landet Entf im Textfeld)
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            crate::app::release_text_focus(ui.ctx());
        }
        // ---------- Klicks ----------
        let mods = ui.input(|i| i.modifiers);
        let multi = mods.command || mods.shift;
        // Marker im Lineal: Klick springt hin, Doppelklick öffnet den Editor
        let markers = self.project.timeline.markers.clone();
        let marker_at = |p: Pos2| -> Option<(u64, Time)> {
            if p.y >= ruler.bottom() || p.x < x0 {
                return None;
            }
            markers
                .iter()
                .filter(|m| (tx(m.at.as_secs_f64()) - p.x).abs() <= 7.0)
                .min_by_key(|m| ((tx(m.at.as_secs_f64()) - p.x).abs() * 100.0) as i64)
                .map(|m| (m.id, m.at))
        };
        if resp.double_clicked() {
            if let Some((id, at)) = resp.interact_pointer_pos().and_then(marker_at) {
                self.stop_playback();
                self.playhead = at;
                self.marker_edit = Some(id);
            }
        }
        let tool = self.timeline.tool;
        let tool_click = tool != Tool::Select
            && resp
                .interact_pointer_pos()
                .or(resp.hover_pos())
                .is_some_and(|p| p.y >= ruler.bottom() && p.x >= x0);
        if tool_click {
            if let Some(p) = resp.hover_pos() {
                ui.ctx().set_cursor_icon(if tool == Tool::Blade {
                    egui::CursorIcon::Text
                } else {
                    egui::CursorIcon::PointingHand
                });
                // Vorschau: Schnittlinie bzw. Grenze der Auswahl
                let line_col = if tool == Tool::Blade { theme::ACCENT() } else { theme::overlay(160) };
                let (top, bottom) = match hit(p).and_then(|h| clip_body(h.0)) {
                    Some((b, _)) if tool == Tool::Blade => (b.top() - BAND_H, b.bottom()),
                    _ => (lanes.top(), lanes.bottom()),
                };
                painter.line_segment([pos2(p.x, top), pos2(p.x, bottom)], Stroke::new(1.5_f32, line_col));
            }
        }
        if resp.clicked() && tool_click {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = Time::from_secs_f64(xt(p.x)).snap_to_frame(fps);
                match tool {
                    Tool::Blade => {
                        if let Some((c, _, _)) = hit(p) {
                            if let Some(OpOutput::Clip(r)) = self.apply_batch(vec![EditOp::SplitClip { clip: c, at: t }]) {
                                self.select_only(Some(r));
                            }
                        }
                    }
                    _ => {
                        let left = tool == Tool::SelectLeft;
                        let ids: Vec<ClipId> = tracks
                            .iter()
                            .filter(|tr| !tr.locked)
                            .flat_map(|tr| tr.clips.iter())
                            .filter(|c| if left { c.start < t } else { c.end() > t })
                            .map(|c| c.id)
                            .collect();
                        self.select_only(None);
                        self.selected = ids.last().copied();
                        self.sel_extra = ids.into_iter().collect();
                    }
                }
            }
        } else if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((_, at)) = marker_at(p) {
                    self.stop_playback();
                    self.playhead = at;
                } else if p.y < ruler.bottom() && p.x >= x0 {
                    self.set_playhead_ui(Time::from_secs_f64(xt(p.x)), false);
                } else if let Some(b) = hit_block(p) {
                    self.select_only(None);
                    self.selected_transition = Some(b);
                } else if let Some((c, _, _)) = hit(p) {
                    if multi {
                        self.toggle_select(c);
                    } else {
                        self.select_only(Some(c));
                    }
                    self.expand_groups();
                } else if p.x >= x0 {
                    // Kurzer Klick ins Leere: Playhead dorthin (höchstens Projektende)
                    if !multi {
                        self.select_only(None);
                    }
                    let t = xt(p.x).min(total_t);
                    self.set_playhead_ui(Time::from_secs_f64(t), true);
                }
            }
        }
        // Doppelklick nur, wenn beide Klicks dieselbe Stelle treffen (egui prüft nur die Zeit)
        let same_spot = resp
            .interact_pointer_pos()
            .zip(self.timeline.last_click)
            .is_some_and(|(a, b)| a.distance(b) < 6.0);
        if resp.double_clicked() && same_spot {
            if let Some((c, _, _)) = resp.interact_pointer_pos().and_then(hit) {
                if self
                    .project
                    .find_clip(c)
                    .is_some_and(|(_, c)| matches!(c.content, ClipContent::Text(_)))
                {
                    self.clip_tab = crate::app::ClipTab::Text;
                    self.focus_text = true;
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.timeline.ctx = Some(if let Some(b) = hit_block(p) {
                    CtxTarget::Transition(b)
                } else if let Some((c, _, _)) = hit(p) {
                    if !self.is_selected(c) {
                        self.select_only(Some(c));
                    }
                    CtxTarget::Clip(c)
                } else if p.x < x0 {
                    match row_at(p) {
                        Some(t) => CtxTarget::Track(tracks[t].id),
                        None => CtxTarget::Empty(None),
                    }
                } else {
                    CtxTarget::Empty(row_at(p).map(|t| tracks[t].id))
                });
            }
        }

        if resp.clicked() {
            self.timeline.last_click = resp.interact_pointer_pos();
        }
        // ---------- Ziehen beginnen ----------
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
            {
                self.commit_live();
                self.timeline.drag = if (p.y < ruler.bottom() && p.x >= x0)
                    || head_rect.contains(p)
                    || ((p.x - tx(ph)).abs() < 4.0 && p.x >= x0 && hit(p).is_none())
                {
                    Drag::Scrub
                } else if self.timeline.tool != Tool::Select {
                    Drag::None
                } else if let Some((clip, handle, value)) = audio_hit(p) {
                    Drag::Audio { clip, handle, value }
                } else {
                    let alt = ui.input(|i| i.modifiers.alt);
                    // Nachbar, der an dieser Kante direkt anschließt
                    let neighbor = |c: ClipId, edge: Edge| -> Option<(ClipId, ClipId)> {
                        let (t, x) = self.project.find_clip(c)?;
                        match edge {
                            Edge::End => t
                                .clips
                                .iter()
                                .find(|o| o.id != c && (o.start.0 - x.end().0).abs() < 2000)
                                .map(|o| (c, o.id)),
                            Edge::Start => t
                                .clips
                                .iter()
                                .find(|o| o.id != c && (o.end().0 - x.start.0).abs() < 2000)
                                .map(|o| (o.id, c)),
                        }
                    };
                    match hit(p) {
                        Some((c, Some(edge), _)) if alt && neighbor(c, edge).is_some() => {
                            let (left, right) = neighbor(c, edge).unwrap();
                            self.select_only(Some(c));
                            Drag::Roll { left, right }
                        }
                        Some((c, Some(edge), _)) => {
                            self.select_only(Some(c));
                            Drag::Trim { clip: c, edge }
                        }
                        Some((c, None, _))
                            if alt
                                && self.project.find_clip(c).is_some_and(|(_, x)| {
                                    crate::edit_tools::source_limit(self, x).is_some()
                                }) =>
                        {
                            self.select_only(Some(c));
                            let src = self
                                .project
                                .find_clip(c)
                                .map(|(_, x)| x.source_in.as_secs_f64())
                                .unwrap_or(0.0);
                            Drag::Slip {
                                clip: c,
                                src,
                                x: p.x,
                            }
                        }
                        Some((c, None, _)) => {
                            if !self.is_selected(c) {
                                if multi {
                                    self.toggle_select(c);
                                } else {
                                    self.select_only(Some(c));
                                }
                            } else {
                                self.selected = Some(c);
                            }
                            self.expand_groups();
                            let start = self
                                .project
                                .find_clip(c)
                                .map(|(_, c)| c.start.as_secs_f64())
                                .unwrap_or(0.0);
                            let ids = self
                                .selection()
                                .into_iter()
                                .filter_map(|id| {
                                    self.project
                                        .find_clip(id)
                                        .map(|(_, x)| (id, x.start.as_secs_f64()))
                                })
                                .collect();
                            Drag::Move {
                                clip: c,
                                grab: xt(p.x) - start,
                                ids,
                            }
                        }
                        None if p.x >= x0 && hit_block(p).is_none() => Drag::Marquee {
                            origin: p,
                            additive: multi,
                        },
                        None => Drag::None,
                    }
                };
            }
        }

        // ---------- Ziehen ----------
        let mut ghost: Vec<(ClipId, f64, f64, Option<usize>)> = Vec::new();
        let mut new_track_hint: Option<Target> = None;
        let mut snap_line: Option<f64> = None;
        let dragging = resp.dragged() || resp.drag_stopped();
        if let (Some(p), true) = (pointer, dragging) {
            match self.timeline.drag.clone() {
                Drag::Scrub => {
                    if self.playing {
                        self.stop_playback();
                    }
                    let t = xt(p.x);
                    let s = snap_to(t, &snap_cands(&[])[2..].to_vec()).filter(|_| true);
                    if let Some(s) = s {
                        snap_line = Some(s);
                    }
                    self.playhead = Time::from_secs_f64(s.unwrap_or(t)).snap_to_frame(fps);
                }
                Drag::Move { clip, grab, ids } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    let dur = c.duration.as_secs_f64();
                    let kind = c.kind();
                    let moving: Vec<ClipId> = ids.iter().map(|x| x.0).collect();
                    let cands = snap_cands(&moving);
                    let raw = (xt(p.x) - grab).max(0.0);
                    let ss = snap_to(raw, &cands);
                    let se = snap_to(raw + dur, &cands).map(|e| e - dur);
                    let s = match (ss, se) {
                        (Some(a), Some(b)) => {
                            if (a - raw).abs() <= (b - raw).abs() {
                                snap_line = Some(a);
                                a
                            } else {
                                snap_line = Some(b + dur);
                                b
                            }
                        }
                        (Some(a), None) => {
                            snap_line = Some(a);
                            a
                        }
                        (None, Some(b)) => {
                            snap_line = Some(b + dur);
                            b
                        }
                        _ => raw,
                    }
                    .max(0.0);
                    let orig = ids.iter().find(|x| x.0 == clip).map(|x| x.1).unwrap_or(0.0);
                    let delta = s - orig;
                    // Ziel: andere Spur gleicher Art oder neue Spur im Leeren
                    let mut target = Target::Same;
                    if ids.len() <= 1 {
                        let first_top = rows
                            .iter()
                            .filter(|r| r.track != REC_ROW)
                            .map(|r| r.rect.top())
                            .fold(f32::INFINITY, f32::min);
                        let last_bottom = rows
                            .iter()
                            .filter(|r| r.track != REC_ROW)
                            .map(|r| r.rect.bottom())
                            .fold(f32::NEG_INFINITY, f32::max);
                        if p.y < first_top - ROW_GAP && kind != TrackKind::Audio {
                            target = Target::NewTop;
                        } else if p.y > last_bottom + ROW_GAP && kind == TrackKind::Audio {
                            target = Target::NewBottom;
                        } else if let Some(t) = row_at(p) {
                            if tracks[t].kind == kind {
                                target = Target::Track(t);
                            }
                        }
                    }
                    if matches!(target, Target::NewTop | Target::NewBottom) {
                        new_track_hint = Some(target);
                    }
                    for (id, st) in &ids {
                        let Some((_, x)) = self.project.find_clip(*id) else {
                            continue;
                        };
                        let ns = (st + delta).max(0.0);
                        let tr = match target {
                            Target::Track(t) if *id == clip => Some(t),
                            Target::NewTop | Target::NewBottom if *id == clip => {
                                Some(usize::MAX - 1)
                            }
                            _ => None,
                        };
                        ghost.push((*id, ns, ns + x.duration.as_secs_f64(), tr));
                    }
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        let fr = |t: f64| Time::from_secs_f64(t).snap_to_frame(fps);
                        let ops: Vec<EditOp> = match target {
                            Target::NewTop | Target::NewBottom => {
                                vec![EditOp::MoveClipToNewTrack {
                                    clip,
                                    index: tracks.len(),
                                    start: fr(s),
                                }]
                            }
                            Target::Track(t) => vec![EditOp::MoveClip {
                                clip,
                                track: Some(tracks[t].id),
                                start: fr(s),
                            }],
                            Target::Same => ids
                                .iter()
                                .map(|(id, st)| EditOp::MoveClip {
                                    clip: *id,
                                    track: None,
                                    start: fr((st + delta).max(0.0)),
                                })
                                .collect(),
                        };
                        let moved = ids.iter().any(|(_, st)| (st + delta - st).abs() > 1e-4)
                            || !matches!(target, Target::Same);
                        if moved {
                            self.apply_batch(ops);
                        }
                        ghost.clear();
                    }
                }
                Drag::Trim { clip, edge } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    let cands = snap_cands(&[clip]);
                    let raw = xt(p.x);
                    let t = snap_to(raw, &cands)
                        .inspect(|s| snap_line = Some(*s))
                        .unwrap_or(raw);
                    let (cs, ce) = (c.start.as_secs_f64(), c.end().as_secs_f64());
                    // Videos/Bilder: Grenzen der Quelle (Audio darf länger werden)
                    let (min_s, max_e) = match &c.content {
                        ClipContent::Media {
                            media,
                            kind: MediaKind::Video,
                        } => {
                            let len = self
                                .project
                                .media(*media)
                                .map(|m| m.duration.as_secs_f64())
                                .unwrap_or(f64::MAX);
                            let sp = c.speed.max(0.01) as f64;
                            (
                                cs - c.source_in.as_secs_f64() / sp,
                                cs + (len - c.source_in.as_secs_f64()) / sp,
                            )
                        }
                        _ => (0.0, f64::MAX),
                    };
                    let (s, e) = match edge {
                        Edge::Start => (t.clamp(min_s.max(0.0), ce - 0.05), ce),
                        Edge::End => (cs, t.clamp(cs + 0.05, max_e)),
                    };
                    ghost.push((clip, s, e, None));
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        let to = if edge == Edge::Start { s } else { e };
                        if (to - if edge == Edge::Start { cs } else { ce }).abs() > 1e-4 {
                            self.apply(EditOp::TrimClip {
                                clip,
                                edge,
                                to: Time::from_secs_f64(to).snap_to_frame(fps),
                            });
                        }
                        ghost.clear();
                    }
                }
                Drag::Roll { left, right } => {
                    let raw = xt(p.x);
                    let t = snap_to(raw, &snap_cands(&[left, right]))
                        .inspect(|s| snap_line = Some(*s))
                        .unwrap_or(raw);
                    if let (Some((_, l)), Some((_, r))) =
                        (self.project.find_clip(left), self.project.find_clip(right))
                    {
                        let t = t.clamp(l.start.as_secs_f64() + 0.04, r.end().as_secs_f64() - 0.04);
                        ghost.push((left, l.start.as_secs_f64(), t, None));
                        ghost.push((right, t, r.end().as_secs_f64(), None));
                        painter.text(
                            pos2(tx(t), ruler.bottom() + 2.0),
                            egui::Align2::CENTER_TOP,
                            crate::tr("Rollen"),
                            egui::FontId::proportional(10.0),
                            theme::ACCENT(),
                        );
                    }
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.roll_edit(left, right, t);
                        ghost.clear();
                    }
                }
                Drag::Slip { clip, src, x } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    // nach rechts ziehen = früherer Ausschnitt (wie in anderen Schnittprogrammen)
                    let lim = crate::edit_tools::source_limit(self, c).unwrap_or(0.0);
                    let v = (src - ((p.x - x) / pps) as f64 * c.speed as f64).clamp(0.0, lim);
                    let cx = tx(c.start.as_secs_f64());
                    painter.text(
                        pos2(cx + 4.0, ruler.bottom() + 2.0),
                        egui::Align2::LEFT_TOP,
                        crate::tr(format!("Slippen: Quelle ab {v:.2} s")),
                        egui::FontId::proportional(10.0),
                        theme::ACCENT(),
                    );
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.slip_edit(clip, v);
                    }
                }
                Drag::Marquee { origin, additive } => {
                    let r = Rect::from_two_pos(origin, p);
                    painter.rect_filled(r, 2.0, theme::overlay(14));
                    painter.rect_stroke(
                        r,
                        2.0,
                        Stroke::new(1.0_f32, theme::overlay(120)),
                        egui::StrokeKind::Inside,
                    );
                    let mut inside = Vec::new();
                    for row in rows.iter().filter(|r| r.track != REC_ROW) {
                        for c in &tracks[row.track].clips {
                            let cr = Rect::from_min_max(
                                pos2(tx(c.start.as_secs_f64()), row.rect.top()),
                                pos2(tx(c.end().as_secs_f64()), row.rect.bottom()),
                            );
                            if cr.intersects(r) {
                                inside.push(c.id);
                            }
                        }
                    }
                    if !additive {
                        self.sel_extra.clear();
                        self.selected = None;
                    }
                    for c in &inside {
                        self.sel_extra.insert(*c);
                    }
                    if let Some(l) = inside.last() {
                        self.selected = Some(*l);
                        self.sel_extra.remove(l);
                    }
                    self.selected_transition = None;
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                    }
                }
                Drag::Audio { clip, handle, .. } => {
                    if let Some((body, c)) = clip_body(clip) {
                        let dur = c.duration.as_secs_f64() as f32;
                        let value = match handle {
                            AudioHandle::FadeIn => ((p.x - body.left()) / pps).clamp(0.0, dur),
                            AudioHandle::FadeOut => ((body.right() - p.x) / pps).clamp(0.0, dur),
                            AudioHandle::Volume => y_vol(body, p.y),
                        };
                        self.timeline.drag = Drag::Audio { clip, handle, value };
                        if resp.drag_stopped() {
                            self.apply_audio_handle(clip, handle, value);
                        }
                    }
                }
                Drag::None => {}
            }
            if resp.drag_stopped() {
                self.timeline.drag = Drag::None;
            }
        }

        // ---------- Asset-Ablage (Hervorhebung) ----------
        let drop_hover = resp.dnd_hover_payload::<DragAsset>();
        let mut drop_clip: Option<ClipId> = None;
        let mut drop_cut: Option<ClipId> = None;
        if let (Some(payload), Some(p)) = (&drop_hover, pointer) {
            match &**payload {
                DragAsset::Effect(_) | DragAsset::Filter(_) => drop_clip = hit(p).map(|h| h.0),
                DragAsset::Transition(_) => {
                    if let Some(t) = row_at(p) {
                        let tr = &tracks[t];
                        drop_cut = tr
                            .clips
                            .iter()
                            .enumerate()
                            .filter(|(i, c)| {
                                tr.clips
                                    .get(i + 1)
                                    .is_some_and(|n| (n.start.0 - c.end().0).abs() < 2000)
                            })
                            .map(|(_, c)| c)
                            .filter(|c| (tx(c.end().as_secs_f64()) - p.x).abs() < 50.0)
                            .min_by(|a, b| {
                                (tx(a.end().as_secs_f64()) - p.x)
                                    .abs()
                                    .total_cmp(&(tx(b.end().as_secs_f64()) - p.x).abs())
                            })
                            .map(|c| c.id);
                    }
                }
                _ => {}
            }
        }

        // ---------- Ruler ----------
        let step = [
            0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
        ]
        .into_iter()
        .find(|s| *s as f32 * pps >= 70.0)
        .unwrap_or(1200.0);
        let minor = step / 5.0;
        let mut t = (scroll / minor).floor() * minor;
        let rp = painter.with_clip_rect(Rect::from_min_max(
            pos2(x0, rect.top()),
            pos2(rect.right(), ruler.bottom()),
        ));
        while tx(t) < rect.right() {
            let x = tx(t);
            if x >= x0 - 1.0 {
                let major = ((t / step).round() * step - t).abs() < 1e-6;
                let hh = if major { 7.0 } else { 3.0 };
                rp.line_segment(
                    [pos2(x, ruler.bottom() - hh), pos2(x, ruler.bottom())],
                    Stroke::new(
                        1.0_f32,
                        if major {
                            theme::TEXT_FAINT()
                        } else {
                            theme::BORDER()
                        },
                    ),
                );
                if major {
                    let secs = t.round() as i64;
                    let label = if step < 1.0 {
                        format!(
                            "{:02}:{:02}.{:01}",
                            secs / 60,
                            secs % 60,
                            ((t.fract()) * 10.0).round() as i64 % 10
                        )
                    } else {
                        format!("{:02}:{:02}", secs / 60, secs % 60)
                    };
                    rp.text(
                        pos2(x + 3.0, ruler.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        crate::tr(label),
                        egui::FontId::proportional(9.5),
                        theme::TEXT_FAINT(),
                    );
                }
            }
            t += minor;
        }
        painter.line_segment(
            [
                pos2(rect.left(), ruler.bottom()),
                pos2(rect.right(), ruler.bottom()),
            ],
            Stroke::new(1.0_f32, theme::BORDER()),
        );
        // In/Out-Bereich
        if self.range_in.is_some() || self.range_out.is_some() {
            let a = self.range_in.map(|t| tx(t.as_secs_f64())).unwrap_or(x0);
            let b = self
                .range_out
                .map(|t| tx(t.as_secs_f64()))
                .unwrap_or(rect.right());
            let r = Rect::from_min_max(
                pos2(a.max(x0), ruler.top()),
                pos2(b.min(rect.right()), ruler.bottom()),
            );
            if r.width() > 0.0 {
                // dezentes Band unten im Lineal (Schleife: blau)
                let band = Rect::from_min_max(pos2(r.left(), ruler.bottom() - 5.0), r.max);
                rp.rect_filled(
                    band,
                    0.0,
                    if self.loop_range {
                        Color32::from_rgb(0x4A, 0x78, 0xC8)
                    } else {
                        Color32::from_rgb(0x5A, 0x5A, 0x64)
                    },
                );
                let lp = painter.with_clip_rect(lanes);
                for x in [r.left(), r.right()] {
                    if x > x0 && x < rect.right() - 1.0 {
                        lp.line_segment(
                            [pos2(x, ruler.bottom()), pos2(x, rect.bottom())],
                            Stroke::new(1.0_f32, Color32::from_rgb(0x4A, 0x4A, 0x55)),
                        );
                    }
                }
            }
            for (t, glyph) in [(self.range_in, "["), (self.range_out, "]")] {
                if let Some(t) = t {
                    let x = tx(t.as_secs_f64());
                    if x >= x0 {
                        rp.line_segment(
                            [pos2(x, ruler.top()), pos2(x, ruler.bottom())],
                            Stroke::new(1.5_f32, theme::ACCENT()),
                        );
                        rp.text(
                            pos2(x, ruler.bottom() - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            glyph,
                            egui::FontId::monospace(11.0),
                            theme::ACCENT(),
                        );
                    }
                }
            }
        }
        // Marker
        for m in self.project.timeline.markers.clone() {
            let x = tx(m.at.as_secs_f64());
            if x < x0 - 6.0 || x > rect.right() + 6.0 {
                continue;
            }
            let col = crate::navigate::marker_color(m.color);
            if m.duration > Time::ZERO {
                let x2 = tx((m.at + m.duration).as_secs_f64());
                rp.rect_filled(
                    Rect::from_min_max(
                        pos2(x.max(x0), ruler.bottom() - 4.0),
                        pos2(x2.min(rect.right()), ruler.bottom()),
                    ),
                    0.0,
                    col.gamma_multiply(0.6),
                );
            }
            let top = ruler.top() + 2.0;
            let pts = vec![
                pos2(x - 5.0, top),
                pos2(x + 5.0, top),
                pos2(x + 5.0, top + 7.0),
                pos2(x, top + 12.0),
                pos2(x - 5.0, top + 7.0),
            ];
            rp.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
            let hit = Rect::from_center_size(pos2(x, top + 6.0), vec2(12.0, 14.0));
            let r = ui.interact(hit, ui.id().with(("marker", m.id)), Sense::hover());
            let tip = if m.name.is_empty() {
                crate::navigate::chapter_time(m.at)
            } else {
                format!("{}  {}", crate::navigate::chapter_time(m.at), m.name)
            };
            let r = if m.note.is_empty() {
                r.on_hover_text(tip)
            } else {
                r.on_hover_text(format!("{tip}\n{}", m.note))
            };
            let _ = r;
        }

        if tracks.is_empty() && recording.is_none() {
            let r = Rect::from_center_size(
                pos2((x0 + rect.right()) / 2.0, lanes.center().y),
                vec2((rect.right() - x0 - 40.0).max(200.0), 58.0),
            );
            painter.add(egui::Shape::dashed_line(
                &[
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ],
                Stroke::new(1.0_f32, theme::TEXT_FAINT()),
                6.0,
                4.0,
            ));
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                crate::tr(format!(
                    "{}   {}",
                    icon::FILM_STRIP,
                    self.t("timeline-empty")
                )),
                egui::FontId::proportional(12.5),
                theme::TEXT_DIM(),
            );
        }

        // ---------- Clips ----------
        let lanes_clip = Rect::from_min_max(pos2(x0, lanes.top()), rect.max);
        let p = painter.with_clip_rect(lanes_clip);
        let ghost_row_rect = |t: Option<usize>, default: Rect| -> Rect {
            match t {
                Some(x) if x == usize::MAX - 1 => match new_track_hint {
                    Some(Target::NewTop) => {
                        let top = rows
                            .iter()
                            .map(|r| r.rect.top())
                            .fold(f32::INFINITY, f32::min);
                        Rect::from_min_size(
                            pos2(default.left(), top - default.height() - ROW_GAP),
                            default.size(),
                        )
                    }
                    _ => {
                        let bottom = rows
                            .iter()
                            .map(|r| r.rect.bottom())
                            .fold(f32::NEG_INFINITY, f32::max);
                        Rect::from_min_size(pos2(default.left(), bottom + ROW_GAP), default.size())
                    }
                },
                Some(x) => rows
                    .iter()
                    .find(|r| r.track == x)
                    .map(|r| r.rect)
                    .unwrap_or(default),
                None => default,
            }
        };
        if let Some(hint) = new_track_hint {
            let any = rows.first().map(|r| r.rect).unwrap_or(lanes);
            let gr = ghost_row_rect(
                Some(usize::MAX - 1),
                Rect::from_min_size(any.min, vec2(any.width(), ROW_H)),
            );
            let lane = Rect::from_min_max(pos2(x0, gr.top()), gr.max);
            p.rect_stroke(
                lane,
                5.0,
                Stroke::new(1.0_f32, theme::overlay(80)),
                egui::StrokeKind::Inside,
            );
            let _ = hint;
            painter.text(
                pos2(rect.left() + 8.0, gr.center().y),
                egui::Align2::LEFT_CENTER,
                crate::tr(format!("{}  Neue Spur", icon::PLUS)),
                egui::FontId::proportional(10.5),
                theme::TEXT_DIM(),
            );
        }
        for row in rows.iter().filter(|r| r.track != REC_ROW) {
            let tr = &tracks[row.track];
            for c0 in &tr.clips {
                let mut c = self.clip_ref(c0).clone();
                let audio_drag = match &self.timeline.drag {
                    Drag::Audio { clip, handle, value }
                        if has_audio(&c)
                            && (*clip == c.id || (self.is_selected(*clip) && self.is_selected(c.id))) =>
                    {
                        set_audio_handle(&mut c, *handle, *value);
                        Some((*handle, *value))
                    }
                    _ => None,
                };
                let (mut s, mut e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
                let mut r = row.rect;
                let mut is_ghost = false;
                if let Some((_, gs, ge, gt)) = ghost.iter().find(|g| g.0 == c.id) {
                    s = *gs;
                    e = *ge;
                    r = ghost_row_rect(*gt, row.rect);
                    is_ghost = true;
                }
                let cr = Rect::from_min_max(pos2(tx(s), r.top()), pos2(tx(e), r.bottom()));
                if cr.right() < x0 || cr.left() > rect.right() || cr.width() < 1.0 {
                    continue;
                }
                self.draw_clip(&p, &c, tr, cr, lanes_clip, rect.right(), x0, is_ghost);
                if has_audio(&c)
                    && !is_ghost
                    && cr.width() >= 24.0
                    && (hovered_clip == Some(c.id) || audio_drag.is_some())
                {
                    let body = Rect::from_min_max(pos2(cr.left(), cr.top() + BAND_H), cr.max);
                    let ap = p.with_clip_rect(body.intersect(lanes_clip));
                    let vy = vol_y(body, c.volume_db);
                    let under = pointer.and_then(audio_hit);
                    let hot = |h: AudioHandle| {
                        audio_drag.is_some_and(|d| d.0 == h) || under.is_some_and(|x| x.0 == c.id && x.1 == h)
                    };
                    let strong = hot(AudioHandle::Volume);
                    ap.line_segment(
                        [pos2(body.left(), vy), pos2(body.right(), vy)],
                        Stroke::new(
                            if strong { 2.0_f32 } else { 1.2 },
                            if strong { Color32::WHITE } else { Color32::from_white_alpha(170) },
                        ),
                    );
                    for (h, x) in [
                        (
                            AudioHandle::FadeIn,
                            (body.left() + c.fade_in.as_secs_f64() as f32 * pps).max(body.left() + 5.0),
                        ),
                        (
                            AudioHandle::FadeOut,
                            (body.right() - c.fade_out.as_secs_f64() as f32 * pps).min(body.right() - 5.0),
                        ),
                    ] {
                        ap.circle(
                            pos2(x, body.top() + 5.0),
                            if hot(h) { 5.0 } else { 3.8 },
                            Color32::WHITE,
                            Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
                        );
                    }
                    let own = matches!(&self.timeline.drag, Drag::Audio { clip, .. } if *clip == c.id);
                    if let Some((h, v)) = audio_drag.filter(|_| own) {
                        let txt = match h {
                            AudioHandle::Volume => format!("{v:+.1} dB"),
                            _ => format!("{v:.1} s"),
                        };
                        let at = pointer.unwrap_or(body.center()) + vec2(10.0, -18.0);
                        let g = p.layout_no_wrap(txt, egui::FontId::proportional(10.5), Color32::WHITE);
                        let bg = Rect::from_min_size(at, g.size()).expand2(vec2(5.0, 2.0));
                        p.rect_filled(bg, 4.0, Color32::from_black_alpha(210));
                        p.galley(at, g, Color32::WHITE);
                    }
                }
                let sel = self.is_selected(c.id);
                if hovered_clip == Some(c.id) && !sel {
                    p.rect_stroke(
                        cr,
                        5.0,
                        Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
                        egui::StrokeKind::Inside,
                    );
                }
                if drop_clip == Some(c.id) {
                    p.rect_stroke(
                        cr,
                        5.0,
                        Stroke::new(2.0_f32, theme::ACCENT()),
                        egui::StrokeKind::Inside,
                    );
                }
                if sel {
                    p.rect_stroke(
                        cr,
                        5.0,
                        Stroke::new(
                            if self.selected == Some(c.id) {
                                2.0_f32
                            } else {
                                1.5
                            },
                            Color32::WHITE,
                        ),
                        egui::StrokeKind::Inside,
                    );
                    if self.selected == Some(c.id) && self.sel_extra.is_empty() {
                        for gx in [cr.left() + 3.5, cr.right() - 3.5] {
                            let g = Rect::from_center_size(
                                pos2(gx, cr.center().y),
                                vec2(4.0, (cr.height() * 0.45).min(20.0)),
                            );
                            p.rect_filled(g, 2.0, Color32::WHITE);
                        }
                        // Keyframes
                        for kt in c.keyframes.times() {
                            let kx = tx(s + kt.as_secs_f64());
                            if kx >= cr.left() && kx <= cr.right() {
                                let here = (self.playhead - c.start - kt).0.abs() < 20_000;
                                theme::diamond(
                                    &p,
                                    pos2(kx, cr.bottom() - 6.0),
                                    4.0,
                                    Some(if here {
                                        theme::ACCENT()
                                    } else {
                                        Color32::from_white_alpha(170)
                                    }),
                                    Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
                                );
                            }
                        }
                    }
                }
            }
        }
        // Übergänge
        for (cid, r) in &tblocks {
            let sel = self.selected_transition == Some(*cid);
            let hov = pointer.is_some_and(|pp| r.contains(pp));
            p.rect_filled(
                *r,
                5.0,
                if sel {
                    theme::ACCENT()
                } else if hov {
                    theme::TEXT_FAINT()
                } else {
                    theme::BG_HOVER()
                },
            );
            p.rect_stroke(
                *r,
                5.0,
                Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
                egui::StrokeKind::Outside,
            );
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                icon::INTERSECT,
                egui::FontId::proportional(11.0),
                if sel { theme::ACCENT_INK() } else { theme::TEXT() },
            );
        }
        if let Some(cid) = drop_cut {
            if let Some((_, c)) = self.project.find_clip(cid) {
                let x = tx(c.end().as_secs_f64());
                if let Some(row) = rows.iter().find(|r| {
                    r.track != REC_ROW && tracks[r.track].clips.iter().any(|x| x.id == cid)
                }) {
                    p.rect_filled(
                        Rect::from_center_size(
                            pos2(x, row.rect.center().y),
                            vec2(22.0, row.rect.height() - 6.0),
                        ),
                        5.0,
                        Color32::from_white_alpha(70),
                    );
                }
            }
        }
        // Aufnahme läuft: wachsender Clip mit Live-Waveform
        if let (Some((start, elapsed, peaks)), Some(row)) =
            (&recording, rows.iter().find(|r| r.track == REC_ROW))
        {
            let cr = Rect::from_min_max(
                pos2(tx(*start), row.rect.top()),
                pos2(tx(start + elapsed), row.rect.bottom()),
            );
            p.rect_filled(cr, 5.0, Color32::from_rgb(0x5A, 0x22, 0x26));
            let base = cr.bottom() - 3.0;
            let wh = cr.height() - BAND_H - 6.0;
            let mut x = cr.left().max(x0);
            while x < cr.right().min(rect.right()) {
                let ts = ((x - cr.left()) / pps) as f64;
                let i = (ts * WAVE_PER_SEC as f64) as usize;
                let j = (((ts + 2.0 / pps as f64) * WAVE_PER_SEC as f64) as usize).max(i + 1);
                let v = peaks
                    .get(i..j.min(peaks.len()))
                    .map(|s| s.iter().copied().fold(0.0f32, f32::max))
                    .unwrap_or(0.0);
                let hh = (v.sqrt() * wh).max(1.0);
                p.line_segment(
                    [pos2(x, base), pos2(x, base - hh)],
                    Stroke::new(1.5_f32, Color32::from_rgb(0xFF, 0x9A, 0x9A)),
                );
                x += 2.0;
            }
            p.text(
                cr.left_top() + vec2(6.0, 2.0),
                egui::Align2::LEFT_TOP,
                crate::tr(format!("{}  Aufnahme {:.1} s", icon::RECORD, elapsed)),
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            );
            if self.playing && cr.right() > rect.right() - 60.0 {
                self.timeline.scroll = (start + elapsed) - ((rect.right() - x0) / pps * 0.5) as f64;
            }
        }

        // ---------- Spurköpfe (über allem, damit verschobene Clips darunter laufen) ----------
        let head_bg = Rect::from_min_max(
            pos2(rect.left(), lanes.top()),
            pos2(x0 - 4.0, rect.bottom()),
        );
        painter.rect_filled(head_bg, 0.0, theme::BG_CARD());
        let hp = painter.with_clip_rect(head_bg);
        for row in &rows {
            let head = Rect::from_min_size(row.rect.min, vec2(HEADER_W - 8.0, row.rect.height()));
            if !head.intersects(head_bg) {
                continue;
            }
            if row.track == REC_ROW {
                hp.text(
                    head.left_center() + vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    icon::MICROPHONE,
                    egui::FontId::proportional(13.0),
                    theme::DANGER,
                );
                continue;
            }
            let tr = &tracks[row.track];
            if tr.main {
                hp.rect_filled(
                    head.shrink2(vec2(2.0, 1.0)),
                    5.0,
                    theme::BG_RAISED(),
                );
            }
            hp.text(
                head.left_center() + vec2(9.0, 0.0),
                egui::Align2::LEFT_CENTER,
                theme::track_icon(tr.kind),
                egui::FontId::proportional(13.0),
                if tr.main {
                    theme::TEXT()
                } else {
                    theme::TEXT_DIM()
                },
            );
            let mut toggles = vec![(
                0usize,
                if tr.locked {
                    icon::LOCK_SIMPLE
                } else {
                    icon::LOCK_SIMPLE_OPEN
                },
                tr.locked,
                "Sperren",
            )];
            if tr.kind != TrackKind::Audio {
                toggles.push((
                    1,
                    if tr.hidden {
                        icon::EYE_SLASH
                    } else {
                        icon::EYE
                    },
                    tr.hidden,
                    "Spur ausblenden",
                ));
            }
            if matches!(tr.kind, TrackKind::Video | TrackKind::Audio) {
                toggles.push((
                    2,
                    if tr.muted {
                        icon::SPEAKER_SLASH
                    } else {
                        icon::SPEAKER_HIGH
                    },
                    tr.muted,
                    "Stumm",
                ));
                toggles.push((
                    3,
                    icon::HEADPHONES,
                    self.solo.contains(&tr.id),
                    "Solo (nur diese Spur hören)",
                ));
            }
            // eingeklappt: nur Symbol und Name, die Schalter hätten keinen Platz
            if self.timeline.collapsed.contains(&tr.id) {
                toggles.clear();
            }
            // Spurname (eigener Name oder Art + Nummer)
            let name = tr.name.clone().unwrap_or_else(|| {
                let n = tracks
                    .iter()
                    .filter(|t| t.kind == tr.kind)
                    .position(|t| t.id == tr.id)
                    .unwrap_or(0)
                    + 1;
                let k = match tr.kind {
                    TrackKind::Audio => "Audio",
                    TrackKind::Text => "Text",
                    TrackKind::Effect => "Effekte",
                    TrackKind::Filter => "Filter",
                    _ => "Video",
                };
                format!("{} {n}", crate::tr(k))
            });
            let name_x = head.left() + 32.0 + toggles.len() as f32 * 19.0 + 4.0;
            if name_x < head.right() - 16.0 {
                let g = hp.layout_no_wrap(
                    name,
                    egui::FontId::proportional(10.5),
                    if tr.name.is_some() {
                        theme::TEXT_DIM()
                    } else {
                        theme::TEXT_FAINT()
                    },
                );
                let clip = Rect::from_min_max(
                    pos2(name_x, head.top()),
                    pos2(head.right() - 6.0, head.bottom()),
                );
                hp.with_clip_rect(clip.intersect(head_bg)).galley(
                    pos2(name_x, head.center().y - g.size().y / 2.0),
                    g,
                    theme::TEXT_DIM(),
                );
            }
            for (n, (k, glyph, on, tip)) in toggles.iter().enumerate() {
                let c = pos2(head.left() + 32.0 + n as f32 * 19.0, head.center().y);
                let r = Rect::from_center_size(c, vec2(18.0, 18.0));
                if !head_bg.contains(c) {
                    continue;
                }
                let br = ui
                    .interact(r, ui.id().with(("trk", tr.id, *k)), Sense::click())
                    .on_hover_text(crate::tr(*tip));
                if br.hovered() {
                    hp.rect_filled(r, 4.0, theme::BG_RAISED());
                }
                hp.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    crate::tr(*glyph),
                    egui::FontId::proportional(12.0),
                    if *on {
                        theme::ACCENT()
                    } else {
                        theme::TEXT_FAINT()
                    },
                );
                if br.clicked() && *k == 3 {
                    if !self.solo.remove(&tr.id) {
                        self.solo.insert(tr.id);
                    }
                    if self.playing {
                        self.restart_audio();
                    }
                } else if br.clicked() {
                    let (mut l, mut h, mut m) = (tr.locked, tr.hidden, tr.muted);
                    match k {
                        0 => l = !l,
                        1 => h = !h,
                        _ => m = !m,
                    }
                    self.apply(EditOp::SetTrackFlags {
                        track: tr.id,
                        locked: l,
                        hidden: h,
                        muted: m,
                    });
                }
            }
        }
        // Senkrechte Bildlaufleiste
        if overflow > 0.0 {
            let bar = Rect::from_min_max(
                pos2(rect.right() - 4.0, lanes.top() + 2.0),
                pos2(rect.right() - 1.0, rect.bottom() - 2.0),
            );
            let frac = view_h / content_h;
            let h = (bar.height() * frac).max(18.0);
            let y = bar.top() + (bar.height() - h) * (self.timeline.vscroll / overflow);
            painter.rect_filled(
                Rect::from_min_size(pos2(bar.left(), y), vec2(bar.width(), h)),
                2.0,
                theme::overlay(60),
            );
        }

        // ---------- Ablegen ----------
        if let Some(payload) = resp.dnd_release_payload::<DragAsset>() {
            let at = pointer.map(|p| {
                let t = xt(p.x);
                Time::from_secs_f64(snap_to(t, &snap_cands(&[])).unwrap_or(t)).snap_to_frame(fps)
            });
            let at = at.unwrap_or(self.playhead);
            let kind = match &*payload {
                DragAsset::Media(m) => self.project.media(*m).map(|m| {
                    if m.kind == MediaKind::Audio {
                        TrackKind::Audio
                    } else {
                        TrackKind::Video
                    }
                }),
                DragAsset::Text(_) => Some(TrackKind::Text),
                DragAsset::Sticker(_) | DragAsset::OwnSticker(_) => Some(TrackKind::Video),
                DragAsset::Sfx(_) | DragAsset::File(_) => Some(TrackKind::Audio),
                DragAsset::Effect(_) => Some(TrackKind::Effect),
                DragAsset::Filter(_) => Some(TrackKind::Filter),
                _ => None,
            };
            let mut track = pointer
                .and_then(row_at)
                .filter(|t| Some(tracks[*t].kind) == kind && !tracks[*t].main)
                .map(|t| tracks[t].id);
            // Ins Leere über den Spuren (bzw. unter Audio) = neue Spur
            let first_top = rows
                .iter()
                .filter(|r| r.track != REC_ROW)
                .map(|r| r.rect.top())
                .fold(f32::INFINITY, f32::min);
            let last_bottom = rows
                .iter()
                .filter(|r| r.track != REC_ROW)
                .map(|r| r.rect.bottom())
                .fold(f32::NEG_INFINITY, f32::max);
            if let (Some(p), Some(k)) = (pointer, kind) {
                let above = p.y < first_top - ROW_GAP && k != TrackKind::Audio;
                let below = p.y > last_bottom + ROW_GAP && k == TrackKind::Audio;
                if (above || below) && !tracks.is_empty() && track.is_none() {
                    if let Some(OpOutput::Track(t)) = self.apply(EditOp::InsertTrack {
                        index: tracks.len(),
                        kind: k,
                    }) {
                        track = Some(t);
                    }
                }
            }
            let on = match &*payload {
                DragAsset::Transition(_) => drop_cut,
                _ => pointer.and_then(|p| hit(p).map(|h| h.0)),
            };
            if matches!(&*payload, DragAsset::Transition(_)) && on.is_none() {
                self.toast_err("Lege den Übergang auf einen Schnitt zwischen zwei Clips");
            } else {
                self.drop_asset(&payload, at, track, on);
            }
        } else if let (Some(payload), Some(p)) = (&drop_hover, pointer) {
            if !matches!(
                &**payload,
                DragAsset::Effect(_) | DragAsset::Filter(_) | DragAsset::Transition(_)
            ) {
                painter.line_segment(
                    [pos2(p.x, ruler.bottom()), pos2(p.x, rect.bottom())],
                    Stroke::new(1.5_f32, theme::ACCENT()),
                );
            }
        }

        // Einrast-Linie
        if let Some(s) = snap_line {
            let x = tx(s);
            painter.line_segment(
                [pos2(x, ruler.bottom()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, theme::SNAP),
            );
        }

        // ---------- Playhead ----------
        let px = tx(self.playhead.as_secs_f64());
        if px >= x0 - 1.0 && px <= rect.right() {
            painter.line_segment(
                [pos2(px, rect.top() + 12.0), pos2(px, rect.bottom())],
                Stroke::new(1.2_f32, theme::TEXT()),
            );
            let hr = Rect::from_center_size(pos2(px, rect.top() + 7.0), vec2(10.0, 12.0));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    hr.left_top(),
                    hr.right_top(),
                    pos2(hr.right(), hr.bottom() - 4.0),
                    pos2(px, hr.bottom() + 1.0),
                    pos2(hr.left(), hr.bottom() - 4.0),
                ],
                theme::TEXT(),
                Stroke::NONE,
            ));
        }
        if self.playing && recording.is_none() && (px > rect.right() - 40.0 || px < x0) {
            self.timeline.scroll = (self.playhead.as_secs_f64() - 1.0).max(0.0);
        }

        // ---------- Kontextmenüs ----------
        resp.context_menu(|ui| self.timeline_menu(ui));
    }

    /// Playhead setzen (Klick); `clamp` begrenzt aufs Projektende.
    fn set_playhead_ui(&mut self, t: Time, clamp: bool) {
        let fps = self.project.settings.fps;
        if self.playing {
            self.stop_playback();
        }
        let t = if clamp {
            t.min(self.project.duration())
        } else {
            t
        };
        self.playhead = t.snap_to_frame(fps);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_clip(
        &self,
        p: &egui::Painter,
        c: &Clip,
        tr: &Track,
        cr: Rect,
        lanes_clip: Rect,
        right: f32,
        x0: f32,
        ghost: bool,
    ) {
        let pps = self.timeline.pps;
        let kind = c.kind();
        let mut col = theme::clip_color(kind);
        if tr.hidden || (tr.muted && kind == TrackKind::Audio) || c.disabled {
            col = col.gamma_multiply(0.4);
        }
        if ghost {
            col = col.gamma_multiply(0.85);
        }
        p.rect_filled(cr, 5.0, col);
        let band = Rect::from_min_max(
            cr.min,
            pos2(cr.right(), (cr.top() + BAND_H).min(cr.bottom())),
        );
        if c.group.is_some() {
            p.rect_stroke(
                cr.shrink(0.5),
                5.0,
                Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(l) = c.label {
            // Farbmarkierung als Kopfband
            p.rect_filled(
                band,
                egui::CornerRadius {
                    nw: 5,
                    ne: 5,
                    sw: 0,
                    se: 0,
                },
                crate::navigate::marker_color(l).gamma_multiply(0.85),
            );
        }
        let body = Rect::from_min_max(pos2(cr.left(), band.bottom()), cr.max);
        let inner = p.with_clip_rect(body.intersect(lanes_clip));
        if let ClipContent::Media { media, kind: mk } = &c.content {
            if matches!(mk, MediaKind::Video | MediaKind::Image) {
                if let Some(tex) = self.thumbs.get(media) {
                    let ts = tex.size_vec2();
                    let th = body.height();
                    let tw = (ts.x / ts.y * th).max(8.0);
                    let mut x = body.left().max(x0 - tw);
                    // auf Kachelraster ausrichten
                    x = body.left() + ((x - body.left()) / tw).floor() * tw;
                    while x < body.right().min(right) {
                        let uv = if c.fx.flip_h {
                            Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0))
                        } else {
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
                        };
                        inner.image(
                            tex.id(),
                            Rect::from_min_size(pos2(x, body.top()), vec2(tw, th)),
                            uv,
                            Color32::from_white_alpha(if c.disabled { 90 } else { 215 }),
                        );
                        x += tw;
                    }
                }
            }
            let src_len = self
                .project
                .media(*media)
                .map(|m| m.duration.as_secs_f64())
                .unwrap_or(f64::MAX);
            if let Some(peaks) = self.waves.get(media).filter(|w| !w.is_empty()) {
                let audio = *mk == MediaKind::Audio;
                let wave_h = if audio {
                    body.height() - 4.0
                } else {
                    (body.height() * 0.32).min(13.0)
                };
                let base = body.bottom() - 2.0;
                let wcol = if audio {
                    Color32::from_rgb(0x8F, 0xE3, 0xB8)
                } else {
                    Color32::from_rgba_unmultiplied(0xC8, 0xF0, 0xF2, 160)
                };
                if !audio {
                    inner.rect_filled(
                        Rect::from_min_max(
                            pos2(body.left(), base - wave_h - 1.0),
                            pos2(body.right(), base + 2.0),
                        ),
                        0.0,
                        Color32::from_black_alpha(100),
                    );
                }
                let mut x = body.left().max(x0);
                let x_end = body.right().min(right);
                while x < x_end {
                    let rel = Time::from_secs_f64(((x - body.left()) / pps) as f64);
                    let t_src = c.source_time(c.start + rel).as_secs_f64();
                    if t_src < src_len {
                        let i = (t_src * WAVE_PER_SEC as f64) as usize;
                        let j =
                            (((t_src + 2.0 * c.speed as f64 / pps as f64) * WAVE_PER_SEC as f64)
                                as usize)
                                .max(i + 1);
                        let v = peaks
                            .get(i..j.min(peaks.len()))
                            .map(|s| s.iter().copied().fold(0.0f32, f32::max))
                            .unwrap_or(0.0);
                        let h = (v.sqrt() * wave_h * 10f32.powf(c.volume_db.min(6.0) / 40.0))
                            .clamp(1.0, wave_h);
                        inner.line_segment(
                            [pos2(x, base), pos2(x, base - h)],
                            Stroke::new(1.5_f32, wcol),
                        );
                    } else {
                        // verlängert: Stille
                        inner.line_segment(
                            [pos2(x, base), pos2(x, base - 1.0)],
                            Stroke::new(1.0_f32, Color32::from_white_alpha(50)),
                        );
                    }
                    x += 2.0;
                }
            }
            // Stille-Bereich bei verlängertem Ton schraffieren
            if *mk == MediaKind::Audio {
                let real_end = c.start.as_secs_f64()
                    + (src_len - c.source_in.as_secs_f64()) / c.speed.max(0.01) as f64;
                let xe = x0 + ((real_end - self.timeline.scroll) as f32) * pps;
                if xe < cr.right() {
                    let sil = Rect::from_min_max(pos2(xe.max(cr.left()), body.top()), body.max);
                    inner.rect_filled(sil, 0.0, Color32::from_black_alpha(70));
                    let mut hx = sil.left() - sil.height();
                    while hx < sil.right() {
                        inner.line_segment(
                            [pos2(hx, sil.bottom()), pos2(hx + sil.height(), sil.top())],
                            Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
                        );
                        hx += 7.0;
                    }
                }
            }
        }
        if let ClipContent::Text(st) = &c.content {
            inner.text(
                body.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &st.text,
                egui::FontId::new(11.5, self.font_family(st.font_family.as_deref())),
                Color32::from_white_alpha(230),
            );
        }
        // Beat-Markierungen
        for b in c.beat_times() {
            let bx = x0 + (((b.as_secs_f64() - self.timeline.scroll) as f32) * pps)
                - (cr.left()
                    - (x0 + ((c.start.as_secs_f64() - self.timeline.scroll) as f32) * pps));
            if bx >= cr.left() && bx <= cr.right() {
                inner.circle_filled(
                    pos2(bx, cr.bottom() - 4.0),
                    2.2,
                    Color32::from_rgb(0xFF, 0xD8, 0x4D),
                );
            }
        }
        // Titelband
        p.rect_filled(
            band,
            egui::CornerRadius {
                nw: 5,
                ne: 5,
                sw: 0,
                se: 0,
            },
            Color32::from_black_alpha(80),
        );
        let mut tags = String::new();
        if c.speed != 1.0 || c.fx.speed_curve.is_some() {
            tags.push_str(&format!(" {} {:.2}x", icon::GAUGE, c.speed));
        }
        if c.fx.reverse {
            tags.push_str(&format!(" {}", icon::ARROWS_LEFT_RIGHT));
        }
        if !c.fx.effects.is_empty() || c.fx.filter.is_some() || !c.fx.adjust.is_identity() {
            tags.push_str(&format!(" {}", icon::SPARKLE));
        }
        if c.fx.mask.is_some() {
            tags.push_str(&format!(" {}", icon::CIRCLE_DASHED));
        }
        if !c.anim.is_empty() {
            tags.push_str(&format!(" {}", icon::FILM_REEL));
        }
        if c.disabled {
            tags.push_str(&format!(" {}", icon::EYE_SLASH));
        }
        let label = match &c.content {
            ClipContent::Text(_) => format!("{}{tags}", icon::TEXT_T),
            ClipContent::Effect(r) => format!(
                "{}  {}{tags}",
                icon::SPARKLE,
                crate::tr(cl_fx::fx_name(&r.id))
            ),
            ClipContent::Filter(r) => format!(
                "{}  {}{tags}",
                icon::CIRCLE_HALF,
                crate::tr(cl_fx::fx_name(&r.id))
            ),
            _ => format!(
                "{}{tags}  {:.1}s",
                c.label(&self.project),
                c.duration.as_secs_f64()
            ),
        };
        p.with_clip_rect(band.shrink2(vec2(4.0, 0.0)).intersect(lanes_clip))
            .text(
                pos2(band.left().max(x0) + 6.0, band.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(10.0),
                Color32::from_white_alpha(230),
            );
        // Ton-Fades als Rampen
        if c.fade_in > Time::ZERO || c.fade_out > Time::ZERO {
            let fi = c.fade_in.as_secs_f64() as f32 * pps;
            let fo = c.fade_out.as_secs_f64() as f32 * pps;
            let st = Stroke::new(1.0_f32, Color32::from_white_alpha(150));
            if fi > 1.0 {
                inner.line_segment(
                    [
                        pos2(body.left(), body.bottom()),
                        pos2(body.left() + fi, body.top()),
                    ],
                    st,
                );
            }
            if fo > 1.0 {
                inner.line_segment(
                    [
                        pos2(body.right() - fo, body.top()),
                        pos2(body.right(), body.bottom()),
                    ],
                    st,
                );
            }
        }
    }

    fn timeline_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(230.0);
        let Some(target) = self.timeline.ctx else {
            ui.close_menu();
            return;
        };
        match target {
            CtxTarget::Clip(id) => {
                let Some((tr, c)) = self
                    .project
                    .find_clip(id)
                    .map(|(t, c)| (t.clone(), c.clone()))
                else {
                    ui.close_menu();
                    return;
                };
                let video = matches!(
                    c.content,
                    ClipContent::Media {
                        kind: MediaKind::Video,
                        ..
                    }
                );
                let visual = c.kind() == TrackKind::Video;
                if menu_item(ui, icon::COPY, "Kopieren", "Strg+C") {
                    self.copy_selection();
                }
                if menu_item(ui, icon::SCISSORS, "Ausschneiden", "Strg+X") {
                    self.cut_selection();
                }
                if !self.clipboard.is_empty()
                    && menu_item(ui, icon::CLIPBOARD_TEXT, "Einfügen", "Strg+V")
                {
                    self.paste();
                }
                if menu_item(ui, icon::COPY_SIMPLE, "Duplizieren", "Strg+D") {
                    self.duplicate_selection();
                }
                if menu_item(ui, icon::TRASH, "Löschen", "Entf") {
                    self.delete_selected();
                }
                ui.separator();
                if menu_item(ui, icon::SCISSORS, "Teilen", "Strg+B") {
                    self.split_at_playhead();
                }
                if menu_item(ui, icon::ARROW_LINE_LEFT, "Links löschen", "Q") {
                    self.delete_side(true);
                }
                if menu_item(ui, icon::ARROW_LINE_RIGHT, "Rechts löschen", "W") {
                    self.delete_side(false);
                }
                ui.separator();
                if visual {
                    ui.menu_button(
                        crate::tr(format!("{}   Bearbeiten", icon::PENCIL_SIMPLE)),
                        |ui| {
                            ui.set_min_width(200.0);
                            if video && menu_item(ui, icon::SNOWFLAKE, "Standbild", "") {
                                self.freeze_frame();
                            }
                            if video
                                && menu_item(
                                    ui,
                                    icon::ARROWS_LEFT_RIGHT,
                                    if c.fx.reverse {
                                        "Vorwärts abspielen"
                                    } else {
                                        "Rückwärts"
                                    },
                                    "",
                                )
                            {
                                self.modify_selection(|c| c.fx.reverse = !c.fx.reverse);
                            }
                            if menu_item(ui, icon::FLIP_HORIZONTAL, "Horizontal spiegeln", "") {
                                self.modify_selection(|c| c.fx.flip_h = !c.fx.flip_h);
                            }
                            if menu_item(ui, icon::FLIP_VERTICAL, "Vertikal spiegeln", "") {
                                self.modify_selection(|c| c.fx.flip_v = !c.fx.flip_v);
                            }
                            if menu_item(ui, icon::ARROW_CLOCKWISE, "90° drehen", "") {
                                self.modify_selection(|c| {
                                    c.transform.rotation_deg =
                                        ((c.transform.rotation_deg + 90.0 + 180.0)
                                            .rem_euclid(360.0))
                                            - 180.0
                                });
                            }
                            if menu_item(
                                ui,
                                icon::ARROW_COUNTER_CLOCKWISE,
                                "Transformation zurücksetzen",
                                "",
                            ) {
                                self.modify_selection(|c| {
                                    c.transform = Transform::default();
                                    for p in [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation] {
                                        c.keyframes.0.remove(&p);
                                    }
                                });
                            }
                        },
                    );
                }
                if video && menu_item(ui, icon::WAVEFORM, "Ton extrahieren", "") {
                    self.extract_audio(id);
                }
                if matches!(
                    c.content,
                    ClipContent::Media {
                        kind: MediaKind::Video | MediaKind::Audio,
                        ..
                    }
                ) {
                    ui.menu_button(
                        crate::tr(format!("{}   Geschwindigkeit", icon::GAUGE)),
                        |ui| {
                            for s in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0] {
                                if ui
                                    .selectable_label(
                                        (c.speed - s).abs() < 1e-3,
                                        crate::tr(format!("{s}×")),
                                    )
                                    .clicked()
                                {
                                    self.set_speed_selection(s);
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                }
                if self.next_adjacent(id).is_some() {
                    if c.transition_out.is_some() {
                        if menu_item(ui, icon::INTERSECT, "Übergang entfernen", "") {
                            self.set_transition(id, None);
                        }
                    } else if menu_item(ui, icon::INTERSECT, "Übergang einfügen (Überblenden)", "")
                    {
                        self.set_transition(id, Some("fade"));
                    }
                }
                if menu_item(
                    ui,
                    if c.disabled {
                        icon::EYE
                    } else {
                        icon::EYE_SLASH
                    },
                    if c.disabled {
                        "Clip aktivieren"
                    } else {
                        "Clip deaktivieren"
                    },
                    "V",
                ) {
                    self.toggle_disabled();
                }
                ui.menu_button(crate::tr(format!("{}   Farbe", icon::PALETTE)), |ui| {
                    ui.set_min_width(150.0);
                    if ui.button(crate::tr("Keine")).clicked() {
                        self.modify_selection(|x| x.label = None);
                        ui.close_menu();
                    }
                    for (i, (n, _)) in cl_core::MARKER_COLORS.iter().enumerate() {
                        let col = crate::navigate::marker_color(i as u8);
                        if ui
                            .button(RichText::new(crate::tr(format!("●  {n}"))).color(col))
                            .clicked()
                        {
                            self.modify_selection(|x| x.label = Some(i as u8));
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if menu_item(ui, icon::INFO, "Eigenschaften", "") {
                    self.props = Some(crate::props::Props::Clip(id));
                }
                let _ = tr;
            }
            CtxTarget::Transition(id) => {
                if menu_item(ui, icon::TRASH, "Übergang entfernen", "Entf") {
                    self.set_transition(id, None);
                }
            }
            CtxTarget::Track(tid) => {
                let Some((idx, tr)) = self
                    .project
                    .timeline
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.id == tid)
                    .map(|(i, t)| (i, t.clone()))
                else {
                    ui.close_menu();
                    return;
                };
                let audio = tr.kind == TrackKind::Audio;
                let (above, below) = if audio {
                    (idx, idx + 1)
                } else {
                    (idx + 1, idx)
                };
                if menu_item(ui, icon::PENCIL_SIMPLE, "Spur umbenennen …", "") {
                    self.track_rename = Some((tr.id, tr.name.clone().unwrap_or_default()));
                }
                for (label, glyph, index, show) in [
                    ("Spur darüber einfügen", icon::ROWS_PLUS_TOP, above, true),
                    ("Spur darunter einfügen", icon::ROWS_PLUS_BOTTOM, below, !tr.main),
                ] {
                    if !show {
                        continue;
                    }
                    ui.menu_button(crate::tr(format!("{glyph}   {label}")), |ui| {
                        for (k, name) in TRACK_KINDS {
                            // Ton liegt immer unter den Bildspuren: nur passende Arten anbieten
                            if (k == TrackKind::Audio) != audio {
                                continue;
                            }
                            if menu_item(ui, theme::track_icon(k), name, "") {
                                self.apply(EditOp::InsertTrack { index, kind: k });
                            }
                        }
                    });
                }
                if !tr.main {
                    if menu_item(ui, icon::ARROW_UP, "Spur nach oben", "") {
                        self.apply(EditOp::MoveTrack { track: tid, up: true });
                    }
                    if menu_item(ui, icon::ARROW_DOWN, "Spur nach unten", "") {
                        self.apply(EditOp::MoveTrack { track: tid, up: false });
                    }
                }
                ui.separator();
                if menu_item(
                    ui,
                    if tr.locked {
                        icon::LOCK_SIMPLE_OPEN
                    } else {
                        icon::LOCK_SIMPLE
                    },
                    if tr.locked { "Entsperren" } else { "Sperren" },
                    "",
                ) {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: !tr.locked,
                        hidden: tr.hidden,
                        muted: tr.muted,
                    });
                }
                if !audio
                    && menu_item(
                        ui,
                        if tr.hidden {
                            icon::EYE
                        } else {
                            icon::EYE_SLASH
                        },
                        if tr.hidden {
                            "Spur einblenden"
                        } else {
                            "Spur ausblenden"
                        },
                        "",
                    )
                {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: tr.locked,
                        hidden: !tr.hidden,
                        muted: tr.muted,
                    });
                }
                if matches!(tr.kind, TrackKind::Video | TrackKind::Audio)
                    && menu_item(
                        ui,
                        if tr.muted {
                            icon::SPEAKER_HIGH
                        } else {
                            icon::SPEAKER_SLASH
                        },
                        if tr.muted { "Ton an" } else { "Stummschalten" },
                        "",
                    )
                {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: tr.locked,
                        hidden: tr.hidden,
                        muted: !tr.muted,
                    });
                }
                ui.separator();
                if menu_item(ui, icon::SELECTION_ALL, "Alle Clips der Spur auswählen", "") {
                    self.select_only(None);
                    self.sel_extra = tr.clips.iter().map(|c| c.id).collect();
                    self.selected = tr.clips.last().map(|c| c.id);
                }
                if menu_item(ui, icon::ARROWS_IN_LINE_HORIZONTAL, "Lücken schließen", "") {
                    self.apply(EditOp::CloseGaps { track: Some(tid) });
                }
                let folded = self.timeline.collapsed.contains(&tid);
                let (g, label) = if folded {
                    (icon::CARET_DOWN, "Spur ausklappen")
                } else {
                    (icon::CARET_UP, "Spur einklappen")
                };
                if menu_item(ui, g, label, "") && !self.timeline.collapsed.remove(&tid) {
                    self.timeline.collapsed.insert(tid);
                }
                ui.menu_button(
                    crate::tr(format!("{}   Spurhöhe", icon::ARROWS_VERTICAL)),
                    |ui| {
                        let cur = self.timeline.heights.get(&tid).copied().unwrap_or(1.0);
                        for (n, f) in [
                            ("Klein", 0.7f32),
                            ("Normal", 1.0),
                            ("Groß", 1.5),
                            ("Sehr groß", 2.2),
                        ] {
                            if ui
                                .selectable_label((cur - f).abs() < 0.01, crate::tr(n))
                                .clicked()
                            {
                                self.timeline.heights.insert(tid, f);
                                ui.close_menu();
                            }
                        }
                    },
                );
                if menu_item(ui, icon::INFO, "Eigenschaften", "") {
                    self.props = Some(crate::props::Props::Track(tid));
                }
                ui.separator();
                let label = if tr.main {
                    "Hauptspur leeren"
                } else {
                    "Spur löschen"
                };
                if menu_item(ui, icon::TRASH, label, "") {
                    self.apply(EditOp::RemoveTrack { track: tid });
                }
            }
            CtxTarget::Empty(track) => {
                if menu_item(ui, icon::CLIPBOARD_TEXT, "Einfügen", "Strg+V") {
                    self.paste();
                }
                if menu_item(ui, icon::SELECTION_ALL, "Alle auswählen", "Strg+A") {
                    self.select_all();
                }
                if let Some(t) = track {
                    if menu_item(
                        ui,
                        icon::ARROWS_IN_LINE_HORIZONTAL,
                        "Lücken dieser Spur schließen",
                        "",
                    ) {
                        self.apply(EditOp::CloseGaps { track: Some(t) });
                    }
                }
                if menu_item(
                    ui,
                    icon::ARROWS_IN_LINE_HORIZONTAL,
                    "Alle Lücken schließen",
                    "",
                ) {
                    self.apply(EditOp::CloseGaps { track: None });
                }
                ui.separator();
                let n = self.project.timeline.tracks.len();
                ui.menu_button(crate::tr(format!("{}   Spur hinzufügen", icon::PLUS)), |ui| {
                    for (k, name) in TRACK_KINDS {
                        if menu_item(ui, theme::track_icon(k), name, "") {
                            self.apply(EditOp::InsertTrack { index: n, kind: k });
                        }
                    }
                });
                if menu_item(ui, icon::TEXT_AA, "Text am Playhead", "") {
                    self.add_text_at(self.playhead, None, None);
                }
            }
        }
        let _ = Slider::new;
    }
}
