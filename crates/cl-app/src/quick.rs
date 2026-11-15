//! Schnellaktionen mit Einstellungen: Pausen entfernen, Füllwörter, Ducking, Zoom-Punch-in.
//! Ein gemeinsames Fenster: Werte einstellen, bei Pausen und Füllwörtern erst analysieren,
//! Treffer prüfen (Klick springt hin, einzeln abwählbar), „Anwenden“ schneidet.
//! Esc schließt, Enter löst die Hauptaktion aus.

use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets::{self, Slider};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuickKind {
    Pauses,
    Fillers,
    Ducking,
    Punch,
}

/// Ein gefundener Schnitt in Projektzeit
pub struct Hit {
    pub track: TrackId,
    pub from: Time,
    pub to: Time,
    pub text: String,
    pub on: bool,
}

/// Fensterzustand und zuletzt benutzte Werte (Standard = die früheren festen Werte)
pub struct Quick {
    pub open: Option<QuickKind>,
    pub thresh_db: f32,
    pub min_pause: f32,
    pub pad_before: f32,
    pub pad_after: f32,
    pub words: String,
    pub duck_db: f32,
    pub ramp_ms: f32,
    pub zoom: f32,
    pub zoom_dur: f32,
    pub whole_track: bool,
    pub hits: Option<Vec<Hit>>,
    /// Projektstand der Analyse: ändert sich das Projekt, ist die Liste veraltet
    hits_rev: u64,
    /// Analysierte Clipbereiche (Spur, von, bis): außerhalb gilt eine Spur als still
    cover: Vec<(TrackId, Time, Time)>,
    confirm_replace: bool,
}

impl Default for Quick {
    fn default() -> Self {
        Self {
            open: None,
            thresh_db: -32.0,
            min_pause: 0.55,
            pad_before: 0.12,
            pad_after: 0.12,
            words: cl_analyze::audio::FILLERS.join(", "),
            duck_db: 12.0,
            ramp_ms: 300.0,
            zoom: 1.25,
            zoom_dur: 1.7,
            whole_track: false,
            hits: None,
            hits_rev: 0,
            cover: Vec::new(),
            confirm_replace: false,
        }
    }
}

/// Schnittbereiche eines Clips (Clipzeit, s) in Projektzeit. Reste unter 40 ms an den
/// Clipkanten werden mitgeschnitten, damit keine Splitter stehen bleiben.
pub fn clip_cuts(start: f64, dur: f64, cuts: &[(f64, f64)]) -> Vec<(Time, Time)> {
    cuts.iter()
        .map(|&(a, b)| (a.max(0.0), b.min(dur)))
        .map(|(a, b)| (if a < 0.04 { 0.0 } else { a }, if dur - b < 0.04 { dur } else { b }))
        .filter(|(a, b)| b - a > 0.02)
        .map(|(a, b)| (Time::from_secs_f64(start + a), Time::from_secs_f64(start + b)))
        .collect()
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase()
}

/// Wortliste: getrennt an Komma, Semikolon und jedem Leerraum (auch Zeilenumbruch)
fn filler_words(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(norm).filter(|w| !w.is_empty()).collect()
}

/// Bereiche `v` ohne den Bereich `cut`
fn minus(v: Vec<(Time, Time)>, (c0, c1): (Time, Time)) -> Vec<(Time, Time)> {
    v.into_iter().flat_map(|(a, b)| [(a, b.min(c0)), (a.max(c1), b)]).filter(|(a, b)| b > a).collect()
}

/// Mehrere Spuren mit „Alles mitziehen“: nur dort schneiden, wo alle analysierten Spuren still
/// sind. Eine Spur spricht innerhalb ihrer Clips (`cover`) überall, wo sie keinen Treffer hat.
pub fn common_cuts(hits: &[(TrackId, Time, Time)], cover: &[(TrackId, Time, Time)]) -> Vec<(TrackId, Time, Time)> {
    let speaking: Vec<(TrackId, (Time, Time))> = cover
        .iter()
        .flat_map(|&(t, a, b)| {
            let own = hits.iter().filter(|h| h.0 == t).fold(vec![(a, b)], |v, h| minus(v, (h.1, h.2)));
            own.into_iter().map(move |r| (t, r))
        })
        .collect();
    hits.iter()
        .flat_map(|&(t, a, b)| {
            let rest = speaking.iter().filter(|s| s.0 != t).fold(vec![(a, b)], |v, s| minus(v, s.1));
            rest.into_iter().map(move |(a, b)| (t, a, b))
        })
        .collect()
}

/// Ton-Quelle eines Ziel-Clips für die Analyse
struct Src {
    track: TrackId,
    start: f64,
    dur: f64,
    path: PathBuf,
    src: f64,
    span: f64,
    sp: f64,
    rev: bool,
    curve: bool,
}

fn mmss(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0);
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}

impl App {
    pub fn quick_open(&mut self, kind: QuickKind) {
        self.quick.open = Some(kind);
        self.quick.hits = None;
        self.quick.confirm_replace = false;
    }

    /// Ziel-Clips mit Ton: Auswahl oder (Schalter) alle Clips ihrer Spuren
    fn quick_sources(&self) -> Vec<Src> {
        let sel = self.selection();
        let whole = self.quick.whole_track;
        self.project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .filter(|t| !whole || t.clips.iter().any(|c| sel.contains(&c.id)))
            .flat_map(|t| t.clips.iter().map(move |c| (t.id, c)))
            .filter(|(_, c)| whole || sel.contains(&c.id))
            .filter_map(|(track, c)| {
                crate::creator::clip_audio(self, c).map(|(path, src, span, sp)| Src {
                    track,
                    start: c.start.as_secs_f64(),
                    dur: c.duration.as_secs_f64(),
                    path,
                    src,
                    span,
                    sp,
                    rev: c.fx.reverse,
                    curve: c.fx.speed_curve.is_some(),
                })
            })
            .collect()
    }

    fn quick_analyze(&mut self, ctx: &egui::Context, kind: QuickKind) {
        // Tempo-Kurven lassen sich nicht linear umrechnen: solche Clips überspringen
        let (curved, srcs): (Vec<Src>, Vec<Src>) = self.quick_sources().into_iter().partition(|s| s.curve);
        if !curved.is_empty() {
            self.toast_err("Clips mit Tempo-Kurve werden übersprungen");
        }
        if srcs.is_empty() {
            if curved.is_empty() {
                self.toast_err("Kein ausgewählter Clip mit Ton");
            }
            return;
        }
        let cover: Vec<(TrackId, Time, Time)> = srcs
            .iter()
            .map(|s| (s.track, Time::from_secs_f64(s.start), Time::from_secs_f64(s.start + s.dur)))
            .collect();
        let rev = self.revision;
        let q = &self.quick;
        let (db, min, before, after) = (q.thresh_db, q.min_pause as f64, q.pad_before as f64, q.pad_after as f64);
        let words = filler_words(&q.words);
        let models = self.models();
        if kind == QuickKind::Fillers && models.parakeet().is_none() {
            self.toast_err("Benötigt: Parakeet TDT 0.6B v3 (Einstellungen → Modelle)");
            return;
        }
        let label = if kind == QuickKind::Pauses { "Pausen werden gesucht …" } else { "Füllwörter werden gesucht …" };
        self.tasks.spawn(ctx, "tool", label, move |p, cancel| {
            let mut hits = Vec::new();
            let n = srcs.len() as f32;
            for (i, s) in srcs.iter().enumerate() {
                let step = |f: f32| p((i as f32 + f) / n);
                let clip = |a: f64, b: f64| crate::creator::src_range_to_clip(a, b, s.span, s.sp, s.rev);
                // (von, bis) in Clipzeit und Bezeichnung
                let cuts: Vec<(f64, f64, String)> = if kind == QuickKind::Pauses {
                    step(0.3);
                    let env = match cl_analyze::audio::envelope(&s.path, s.src, Some(s.span)) {
                        Ok(e) => e,
                        Err(e) => return TaskDone::Failed(e),
                    };
                    // Pause (a, b): Sprache endet bei a, setzt bei b wieder ein
                    cl_analyze::audio::silences(&env, db, min)
                        .into_iter()
                        .map(|(a, b)| {
                            let (a, b) = clip(a + after, b - before);
                            (a, b, String::new())
                        })
                        .collect()
                } else {
                    match cl_analyze::asr::parakeet(&models, &s.path, s.src, s.span, &step, cancel) {
                        Ok(ws) => ws
                            .iter()
                            .filter(|w| words.contains(&norm(&w.text)))
                            .map(|w| {
                                let (a, b) = clip((w.start - 0.03).max(0.0), w.end + 0.03);
                                (a, b, format!("„{}“", w.text.trim()))
                            })
                            .collect(),
                        Err(e) => return TaskDone::Failed(e),
                    }
                };
                for (a, b, text) in cuts {
                    for (from, to) in clip_cuts(s.start, s.dur, &[(a, b)]) {
                        hits.push(Hit { track: s.track, from, to, text: text.clone(), on: true });
                    }
                }
            }
            hits.sort_by_key(|h| h.from);
            TaskDone::Run(Box::new(move |app: &mut App| {
                if app.quick.open == Some(kind) {
                    app.quick.hits = Some(hits);
                    // Stand vom Analysestart: wurde inzwischen geändert, verwirft der Dialog die Liste
                    app.quick.hits_rev = rev;
                    app.quick.cover = cover;
                }
            }))
        });
    }

    fn quick_apply(&mut self, ctx: &egui::Context, kind: QuickKind) -> bool {
        match kind {
            QuickKind::Pauses | QuickKind::Fillers => {
                let Some(hits) = self.quick.hits.take() else {
                    return false;
                };
                let mut ranges: Vec<(TrackId, Time, Time)> = hits.iter().filter(|h| h.on).map(|h| (h.track, h.from, h.to)).collect();
                if self.project.settings.ripple_all_tracks {
                    ranges = common_cuts(&ranges, &self.quick.cover);
                }
                let total: f64 = ranges.iter().map(|r| (r.2 - r.1).as_secs_f64()).sum();
                let n = ranges.len();
                let label = if kind == QuickKind::Pauses { "Pausen entfernen" } else { "Füllwörter entfernen" };
                if self.ripple_ranges(ranges, label) {

                    self.toast(if kind == QuickKind::Pauses {
                        format!("{} ({total:.1} s)", crate::theme::count_text(n, "{} Pause entfernt", "{} Pausen entfernt"))
                    } else {
                        crate::theme::count_text(n, "{} Füllwort entfernt", "{} Füllwörter entfernt")
                    });
                }
                true
            }
            QuickKind::Ducking => {
                let has_kf = self
                    .selected
                    .and_then(|s| self.project.find_clip(s))
                    .is_some_and(|(_, c)| c.keyframes.0.get(&Prop::Volume).is_some_and(|k| !k.is_empty()));
                if has_kf && !self.quick.confirm_replace {
                    self.quick.confirm_replace = true;
                    return false;
                }
                self.tool_ducking(ctx, self.quick.duck_db, self.quick.ramp_ms / 1000.0);
                true
            }
            QuickKind::Punch => {
                self.tool_punch_in(self.quick.zoom, self.quick.zoom_dur);
                true
            }
        }
    }

    /// Das gemeinsame Quick-Action-Fenster
    pub fn quick_dialog(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.quick.open else { return };
        if self.quick.hits.is_some() && self.quick.hits_rev != self.revision {
            self.quick.hits = None;
        }
        let analyzes = matches!(kind, QuickKind::Pauses | QuickKind::Fillers);
        let busy = self.tasks.running("tool").is_some();
        // Enter im Textfeld (Wortliste, Zahlenfeld) ist Eingabe, keine Hauptaktion
        let (mut close, mut main) = crate::dialog::keys(ctx);
        // Füllwörter ohne Spracherkennungsmodell: statt „Analysieren“ zum Modell-Download
        let no_model = kind == QuickKind::Fillers && self.models().parakeet().is_none();
        let multi = self.project.settings.ripple_all_tracks
            && self.quick.cover.iter().any(|c| self.quick.cover.first().is_some_and(|f| f.0 != c.0));
        let mut jump: Option<Time> = None;
        let mut x = false;
        let title = match kind {
            QuickKind::Pauses => "Pausen entfernen",
            QuickKind::Fillers => "Füllwörter entfernen",
            QuickKind::Ducking => "Musik unter Sprache absenken",
            QuickKind::Punch => "Zoom-Punch-in",
        };
        crate::dialog::window(ctx, crate::tr(title), "quick-action")
            .default_width(340.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
            .show(ctx, |ui| {
                x = crate::dialog::title(ui, title);
                let q = &mut self.quick;
                let mut changed = false;
                match kind {
                    QuickKind::Pauses => {
                        theme::row(ui, "Schwelle", |ui| {
                            changed |= Slider::new(&mut q.thresh_db, -60.0..=-10.0).default(-32.0).suffix(" dB").show(ui).changed;
                        });
                        theme::row(ui, "Mindestlänge", |ui| {
                            changed |= Slider::new(&mut q.min_pause, 0.2..=3.0).default(0.55).decimals(2).suffix(" s").show(ui).changed;
                        });
                        theme::row(ui, "Rand davor", |ui| {
                            changed |= Slider::new(&mut q.pad_before, 0.0..=0.5).default(0.12).decimals(2).suffix(" s").show(ui).changed;
                        });
                        theme::row(ui, "Rand danach", |ui| {
                            changed |= Slider::new(&mut q.pad_after, 0.0..=0.5).default(0.12).decimals(2).suffix(" s").show(ui).changed;
                        });
                    }
                    QuickKind::Fillers => {
                        ui.label(RichText::new(crate::tr("Wörter (durch Komma getrennt)")).size(theme::FS_UI).color(theme::TEXT_DIM()));
                        changed |= ui.add(egui::TextEdit::multiline(&mut q.words).desired_rows(2).desired_width(f32::INFINITY)).changed();
                        if no_model {
                            ui.label(RichText::new(crate::tr("Noch kein Spracherkennungsmodell installiert")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        }
                    }
                    QuickKind::Ducking => {
                        ui.label(RichText::new(crate::tr("Wirkt auf den ausgewählten Musik-Clip")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        theme::row(ui, "Absenkung", |ui| {
                            Slider::new(&mut q.duck_db, 1.0..=40.0).default(12.0).suffix(" dB").show(ui);
                        });
                        theme::row(ui, "Rampe", |ui| {
                            Slider::new(&mut q.ramp_ms, 0.0..=2000.0).default(300.0).suffix(" ms").show(ui);
                        });
                    }
                    QuickKind::Punch => {
                        theme::row(ui, "Stärke", |ui| {
                            Slider::new(&mut q.zoom, 1.05..=2.0).default(1.25).percent().show(ui);
                        });
                        theme::row(ui, "Dauer", |ui| {
                            Slider::new(&mut q.zoom_dur, 0.6..=5.0).default(1.7).decimals(1).suffix(" s").show(ui);
                        });
                    }
                }
                if analyzes {
                    changed |= widgets::check(ui, &mut q.whole_track, crate::tr("Alle Clips der Spur")).changed();
                    if changed {
                        q.hits = None;
                    }
                    if let Some(hits) = &mut q.hits {
                        let on: Vec<&Hit> = hits.iter().filter(|h| h.on).collect();
                        let secs: f64 = on.iter().map(|h| (h.to - h.from).as_secs_f64()).sum::<f64>().abs();
                        let sum = if kind == QuickKind::Pauses {
                            format!("{}, {secs:.0} s kürzer", crate::theme::count_text(on.len(), "{} Pause", "{} Pausen"))
                        } else {
                            format!("{}, {secs:.0} s kürzer", crate::theme::count_text(on.len(), "{} Füllwort", "{} Füllwörter"))
                        };
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::tr(sum)).size(theme::FS_TEXT).strong());
                        if hits.is_empty() {
                            theme::empty_state(ui, icon::CHECK_CIRCLE, "Nichts gefunden", "Werte anpassen und erneut analysieren");
                        }
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for h in hits.iter_mut() {
                                ui.horizontal(|ui| {
                                    widgets::toggle(ui, &mut h.on);
                                    let text = format!("{}   {:.1} s   {}", mmss(h.from), (h.to - h.from).as_secs_f64(), h.text);
                                    let color = if h.on { theme::TEXT() } else { theme::TEXT_FAINT() };
                                    let r = ui.add(egui::Label::new(RichText::new(text).size(theme::FS_UI).color(color)).sense(egui::Sense::click()));
                                    if r.on_hover_text(crate::tr("Zum Treffer springen")).clicked() {
                                        jump = Some(h.from);
                                    }
                                });
                            }
                        });
                        if multi {
                        }
                    }
                }
                if kind == QuickKind::Ducking && q.confirm_replace {
                    ui.add_space(6.0);
                    ui.label(RichText::new(crate::tr("Der Clip hat schon Lautstärke-Keyframes. Ersetzen?")).size(theme::FS_UI).color(theme::TEXT_DIM()));
                }
                let label = if no_model {
                    "Modell laden …"
                } else if analyzes && q.hits.is_none() {
                    "Analysieren"
                } else if q.confirm_replace {
                    "Ersetzen"
                } else {
                    "Anwenden"
                };
                let (m, c) = crate::dialog::buttons(ui, theme::primary(label), !busy);
                main |= m;
                close |= c;
            });
        if let Some(t) = jump {
            self.stop_playback();
            self.playhead = t;
        }
        if x {
            self.quick.confirm_replace = false;
            self.quick.open = None;
        } else if main && no_model {
            // wie bei den Untertiteln: Einstellungen → Modelle
            self.quick.open = None;
            self.settings_section = crate::settings::Section::Models;
            self.settings_open = true;
        } else if close {
            if self.quick.confirm_replace {
                self.quick.confirm_replace = false;
            } else {
                self.quick.open = None;
            }
        } else if main && !busy {
            if analyzes && self.quick.hits.is_none() {
                self.quick_analyze(ctx, kind);
            } else if self.quick_apply(ctx, kind) {
                self.quick.open = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_snap_to_clip_edges_and_drop_slivers() {
        let s = Time::from_secs_f64;
        let r = clip_cuts(10.0, 5.0, &[(0.03, 1.0), (2.0, 2.01), (3.0, 4.98), (-1.0, -0.5)]);
        assert_eq!(r, vec![(s(10.0), s(11.0)), (s(13.0), s(15.0))]);
        assert_eq!(norm("Ähm,"), "ähm");
    }

    #[test]
    fn filler_words_split_on_any_whitespace() {
        assert_eq!(filler_words("ähm\nalso, äh;uh\tum"), vec!["ähm", "also", "äh", "uh", "um"]);
    }

    #[test]
    fn multi_track_cuts_only_where_all_are_silent() {
        let s = Time::from_secs_f64;
        let (a, b) = (TrackId::new(), TrackId::new());
        // A: Clip 0–20, Pausen 2–4 und 10–12. B: Clip 0–15, Pause 3–5 (spricht bei 10–12)
        let hits = [(a, s(2.0), s(4.0)), (a, s(10.0), s(12.0)), (b, s(3.0), s(5.0))];
        let cover = [(a, s(0.0), s(20.0)), (b, s(0.0), s(15.0))];
        let r = common_cuts(&hits, &cover);
        assert_eq!(r, vec![(a, s(3.0), s(4.0)), (b, s(3.0), s(4.0))]);
        // B hat nach 15 s keinen Clip: dort gilt B als still
        let r = common_cuts(&[(a, s(16.0), s(18.0))], &cover);
        assert_eq!(r, vec![(a, s(16.0), s(18.0))]);
        // eine Spur: unverändert
        assert_eq!(common_cuts(&hits[..2], &cover[..1]), hits[..2].to_vec());
    }
}
