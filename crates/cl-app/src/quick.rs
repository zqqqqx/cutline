//! Quick actions with settings: remove pauses, filler words, ducking, zoom punch-in.
//! One shared window: set values, analyze first for pauses and filler words,
//! check hits (click jumps there, individually deselectable), „Apply“ cuts.
//! Esc closes, Enter triggers the main action.

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

/// A found cut in project time
pub struct Hit {
    pub track: TrackId,
    pub from: Time,
    pub to: Time,
    pub text: String,
    pub on: bool,
}

/// Window state and last used values (default = the former fixed values). The values
/// survive a restart (`last-used.json`), the window state does not.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Quick {
    #[serde(skip)]
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
    #[serde(skip)]
    pub hits: Option<Vec<Hit>>,
    /// Project state of the analysis: if the project changes, the list is outdated
    #[serde(skip)]
    hits_rev: u64,
    /// Analyzed clip ranges (track, from, to): outside them a track counts as silent
    #[serde(skip)]
    cover: Vec<(TrackId, Time, Time)>,
    #[serde(skip)]
    confirm_replace: bool,
}

/// Last used values from `text` (content of `last-used.json`): quick actions, safe area,
/// thumbnail and playback volume. If something is missing or the file is broken, the default applies to it.
pub fn last_used(text: &str) -> (Quick, bool, crate::thumb::Opts, crate::audio::Volume) {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    (
        serde_json::from_value(v["quick"].clone()).unwrap_or_default(),
        v["safe_area"].as_bool().unwrap_or(false),
        serde_json::from_value(v["thumb"].clone()).unwrap_or_default(),
        serde_json::from_value::<crate::audio::Volume>(v["volume"].clone())
            .ok()
            .filter(|v| (0.0..=100.0).contains(&v.percent))
            .unwrap_or_default(),
    )
}

fn last_used_path() -> PathBuf {
    crate::paths::data_dir().join("last-used.json")
}

pub fn load_last_used() -> (Quick, bool, crate::thumb::Opts, crate::audio::Volume) {
    last_used(&std::fs::read_to_string(last_used_path()).unwrap_or_default())
}

impl App {
    pub fn save_last_used(&self) {
        let text = serde_json::json!({ "quick": self.quick, "safe_area": self.safe_area, "thumb": self.thumb_opts, "volume": self.volume }).to_string();
        let _ = crate::paths::write_atomic(&last_used_path(), text);
    }

    fn close_quick(&mut self) {
        self.quick.open = None;
        self.save_last_used();
    }
}

/// Set values of the quick actions and the safe area were lost on restart
#[test]
fn last_used_values_survive_a_restart() {
    let q = Quick { thresh_db: -40.0, words: "also".into(), whole_track: true, open: Some(QuickKind::Pauses), ..Default::default() };
    let th = crate::thumb::Opts { person: true, bg: crate::thumb::Bg::Color, pos: 2, ..Default::default() };
    let vol = crate::audio::Volume { percent: 35.0, muted: true };
    let text = serde_json::json!({ "quick": q, "safe_area": true, "thumb": th, "volume": vol }).to_string();
    let (back, safe, th_back, vol_back) = last_used(&text);
    assert_eq!(vol_back, vol);
    assert!(safe && back.thresh_db == -40.0 && back.words == "also" && back.whole_track);
    assert_eq!(th_back, th);
    assert!(back.open.is_none(), "the window does not open again by itself");
    assert_eq!(back.min_pause, Quick::default().min_pause);
    // broken or empty file: default instead of a crash
    for bad in ["", "{", r#"{"quick": 7, "safe_area": "ja", "volume": "laut"}"#, r#"{"volume": {"percent": 500}}"#] {
        let (d, safe, _, vol) = last_used(bad);
        assert!(!safe && d.thresh_db == Quick::default().thresh_db, "{bad}");
        assert_eq!(vol, crate::audio::Volume { percent: 100.0, muted: false }, "{bad}");
    }
}

impl Default for Quick {
    fn default() -> Self {
        Self {
            open: None,
            thresh_db: -32.0,
            min_pause: 0.55,
            pad_before: 0.12,
            pad_after: 0.12,
            words: cl_analyze::audio::fillers().join(", "),
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

/// Cut ranges of a clip (clip time, s) in project time. Leftovers under 40 ms at the
/// clip edges are cut along, so no slivers remain.
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

/// Word list: split at comma, semicolon and any whitespace (also line breaks)
fn filler_words(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(norm).filter(|w| !w.is_empty()).collect()
}

/// Ranges `v` without the range `cut`
fn minus(v: Vec<(Time, Time)>, (c0, c1): (Time, Time)) -> Vec<(Time, Time)> {
    v.into_iter().flat_map(|(a, b)| [(a, b.min(c0)), (a.max(c1), b)]).filter(|(a, b)| b > a).collect()
}

/// Several tracks with „Alles mitziehen“: only cut where all analyzed tracks are
/// silent. A track speaks within its clips (`cover`) everywhere it has no hit.
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

/// Audio source of a target clip for the analysis
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
    format!("{}:{}", (s / 60.0) as u64, crate::theme::local_num(format!("{:04.1}", s % 60.0)))
}

impl App {
    pub fn quick_open(&mut self, kind: QuickKind) {
        self.quick.open = Some(kind);
        self.quick.hits = None;
        self.quick.confirm_replace = false;
    }

    /// Target clips with audio: the selection or (toggle) all clips of their tracks
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
        // Speed curves cannot be converted linearly: skip such clips
        let (curved, srcs): (Vec<Src>, Vec<Src>) = self.quick_sources().into_iter().partition(|s| s.curve);
        if !curved.is_empty() {
            self.toast_err("Clips with a speed curve are skipped");
        }
        if srcs.is_empty() {
            if curved.is_empty() {
                self.toast_err("No selected clip with audio");
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
            self.toast_err("Requires: Parakeet TDT 0.6B v3 (Settings → Models)");
            return;
        }
        let label = if kind == QuickKind::Pauses { "Looking for pauses …" } else { "Looking for filler words …" };
        self.tasks.spawn(ctx, "tool", label, move |p, cancel| {
            let mut hits = Vec::new();
            let n = srcs.len() as f32;
            for (i, s) in srcs.iter().enumerate() {
                let step = |f: f32| p((i as f32 + f) / n);
                let clip = |a: f64, b: f64| crate::creator::src_range_to_clip(a, b, s.span, s.sp, s.rev);
                // (from, to) in clip time and label
                let cuts: Vec<(f64, f64, String)> = if kind == QuickKind::Pauses {
                    step(0.3);
                    let env = match cl_analyze::audio::envelope(&s.path, s.src, Some(s.span)) {
                        Ok(e) => e,
                        Err(e) => return TaskDone::Failed(e),
                    };
                    // Pause (a, b): speech ends at a, resumes at b
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
                                (a, b, format!("“{}”", w.text.trim()))
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
                    // State at analysis start: if it has changed since, the dialog discards the list
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
                let label = if kind == QuickKind::Pauses { "Remove pauses" } else { "Remove filler words" };
                if self.ripple_ranges(ranges, label) {

                    self.toast(if kind == QuickKind::Pauses {
                        format!("{} ({})", crate::theme::count_text(n, "{} pause removed", "{} pauses removed"), crate::theme::secs(total as f64, 1))
                    } else {
                        crate::theme::count_text(n, "{} filler word removed", "{} filler words removed")
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

    /// The shared quick action window
    pub fn quick_dialog(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.quick.open else { return };
        if self.quick.hits.is_some() && self.quick.hits_rev != self.revision {
            self.quick.hits = None;
        }
        let analyzes = matches!(kind, QuickKind::Pauses | QuickKind::Fillers);
        let busy = self.tasks.running("tool").is_some();
        // Enter in a text field (word list, number field) is input, not the main action
        let (mut close, mut main) = crate::dialog::keys(ctx);
        // Filler words without a speech recognition model: model download instead of „Analyze“
        let no_model = kind == QuickKind::Fillers && self.models().parakeet().is_none();
        let multi = self.project.settings.ripple_all_tracks
            && self.quick.cover.iter().any(|c| self.quick.cover.first().is_some_and(|f| f.0 != c.0));
        let mut jump: Option<Time> = None;
        let mut x = false;
        let title = match kind {
            QuickKind::Pauses => "Remove pauses",
            QuickKind::Fillers => "Remove filler words",
            QuickKind::Ducking => "Lower music under speech",
            QuickKind::Punch => "Zoom punch-in",
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
                        theme::row(ui, "Threshold", |ui| {
                            changed |= Slider::new(&mut q.thresh_db, -60.0..=-10.0).default(-32.0).suffix(" dB").show(ui).changed;
                        });
                        theme::row(ui, "Minimum length", |ui| {
                            changed |= Slider::new(&mut q.min_pause, 0.2..=3.0).default(0.55).decimals(2).suffix(" s").show(ui).changed;
                        });
                        theme::row(ui, "Margin before", |ui| {
                            changed |= Slider::new(&mut q.pad_before, 0.0..=0.5).default(0.12).decimals(2).suffix(" s").show(ui).changed;
                        });
                        theme::row(ui, "Margin after", |ui| {
                            changed |= Slider::new(&mut q.pad_after, 0.0..=0.5).default(0.12).decimals(2).suffix(" s").show(ui).changed;
                        });
                    }
                    QuickKind::Fillers => {
                        ui.label(RichText::new(crate::tr("Words (comma-separated)")).size(theme::FS_UI).color(theme::TEXT_DIM()));
                        changed |= ui.add(egui::TextEdit::multiline(&mut q.words).desired_rows(2).desired_width(f32::INFINITY)).changed();
                        if no_model {
                            ui.label(RichText::new(crate::tr("No speech recognition model installed yet")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        }
                    }
                    QuickKind::Ducking => {
                        ui.label(RichText::new(crate::tr("Applies to the selected music clip")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        theme::row(ui, "Reduction", |ui| {
                            Slider::new(&mut q.duck_db, 1.0..=40.0).default(12.0).suffix(" dB").show(ui);
                        });
                        theme::row(ui, "Ramp", |ui| {
                            Slider::new(&mut q.ramp_ms, 0.0..=2000.0).default(300.0).suffix(" ms").show(ui);
                        });
                    }
                    QuickKind::Punch => {
                        theme::row(ui, "Strength", |ui| {
                            Slider::new(&mut q.zoom, 1.05..=2.0).default(1.25).percent().show(ui);
                        });
                        theme::row(ui, "Duration", |ui| {
                            Slider::new(&mut q.zoom_dur, 0.6..=5.0).default(1.7).decimals(1).suffix(" s").show(ui);
                        });
                    }
                }
                if analyzes {
                    changed |= widgets::check(ui, &mut q.whole_track, crate::tr("All clips on the track")).changed();
                    if changed {
                        q.hits = None;
                    }
                    if let Some(hits) = &mut q.hits {
                        let on: Vec<&Hit> = hits.iter().filter(|h| h.on).collect();
                        let secs: f64 = on.iter().map(|h| (h.to - h.from).as_secs_f64()).sum::<f64>().abs();
                        let sum = if kind == QuickKind::Pauses {
                            format!("{}, {secs:.0} s shorter", crate::theme::count_text(on.len(), "{} pause", "{} pauses"))
                        } else {
                            format!("{}, {secs:.0} s shorter", crate::theme::count_text(on.len(), "{} filler word", "{} filler words"))
                        };
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::tr(sum)).size(theme::FS_TEXT).strong());
                        if hits.is_empty() {
                            theme::empty_state(ui, icon::CHECK_CIRCLE, "Nothing found", "Adjust the values and analyze again");
                        }
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for h in hits.iter_mut() {
                                ui.horizontal(|ui| {
                                    widgets::toggle(ui, &mut h.on);
                                    let text = format!("{}   {}   {}", mmss(h.from), theme::secs((h.to - h.from).as_secs_f64(), 1), h.text);
                                    let color = if h.on { theme::TEXT() } else { theme::TEXT_FAINT() };
                                    let r = ui.add(egui::Label::new(RichText::new(text).size(theme::FS_UI).color(color)).sense(egui::Sense::click()));
                                    if r.on_hover_text(crate::tr("Jump to this result")).clicked() {
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
                    ui.label(RichText::new(crate::tr("The clip already has volume keyframes. Replace them?")).size(theme::FS_UI).color(theme::TEXT_DIM()));
                }
                let label = if no_model {
                    "Download model …"
                } else if analyzes && q.hits.is_none() {
                    "Analyze"
                } else if q.confirm_replace {
                    "Replace"
                } else {
                    "Apply"
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
            self.close_quick();
        } else if main && no_model {
            // as with captions: Settings → Models
            self.close_quick();
            self.settings_section = crate::settings::Section::Models;
            self.settings_open = true;
        } else if close {
            if self.quick.confirm_replace {
                self.quick.confirm_replace = false;
            } else {
                self.close_quick();
            }
        } else if main && !busy {
            if analyzes && self.quick.hits.is_none() {
                self.quick_analyze(ctx, kind);
            } else if self.quick_apply(ctx, kind) {
                self.close_quick();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_times_use_the_decimal_comma_in_german() {
        let _g = crate::paths::test_lock();
        cl_i18n::set_ui_lang(cl_i18n::Lang::De);
        assert_eq!(mmss(Time::from_secs_f64(65.3)), "1:05,3");
        cl_i18n::set_ui_lang(cl_i18n::Lang::En);
    }

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
        // A: clip 0–20, pauses 2–4 and 10–12. B: clip 0–15, pause 3–5 (speaks at 10–12)
        let hits = [(a, s(2.0), s(4.0)), (a, s(10.0), s(12.0)), (b, s(3.0), s(5.0))];
        let cover = [(a, s(0.0), s(20.0)), (b, s(0.0), s(15.0))];
        let r = common_cuts(&hits, &cover);
        assert_eq!(r, vec![(a, s(3.0), s(4.0)), (b, s(3.0), s(4.0))]);
        // B has no clip after 15 s: there B counts as silent
        let r = common_cuts(&[(a, s(16.0), s(18.0))], &cover);
        assert_eq!(r, vec![(a, s(16.0), s(18.0))]);
        // one track: unchanged
        assert_eq!(common_cuts(&hits[..2], &cover[..1]), hits[..2].to_vec());
    }
}
