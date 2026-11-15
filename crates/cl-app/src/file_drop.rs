//! Dateien aus dem Explorer direkt in die Zeitleiste ziehen.
//!
//! winit 0.30 meldet beim Ziehen von außen nur „Datei schwebt über dem Fenster“ und „Datei
//! abgelegt“ – ohne Mausposition. Die Position kommt deshalb vom Betriebssystem (Windows:
//! `GetCursorPos` relativ zum Fensterinhalt); anderswo gibt es keine, dann gilt [`decide`].
//! Geprüft (ffprobe) wird im Hintergrund als eine Aufgabe; bis dahin zeigt die Zeitleiste
//! Platzhalter, die ihre echte Länge bekommen, sobald die Datei geprüft ist. Übernommen wird
//! alles zusammen als ein Verlaufsschritt, wenn die letzte Datei fertig ist.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Mutex;
use std::time::Duration;

use cl_core::{
    fits, gap_index, new_track_gap, EditOp, MediaKind, MediaRef, OpOutput, Time, Track, TrackId, TrackKind,
};
use eframe::egui::{self, pos2, Pos2, Rect};

use crate::app::App;
use crate::tasks::TaskDone;

/// Wohin die Clips einer Art (Bild/Video bzw. Ton) kommen
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// vorhandene, passende Spur
    Track(TrackId),
    /// neue Spur: Index für `InsertTrack`, Lücke in der Anzeige (für die Einfüge-Linie)
    New { index: usize, gap: usize },
    /// wie ohne Ziel: Bild/Video auf die Hauptspur, Ton auf die erste freie Tonspur
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub at: Time,
    pub video: Lane,
    pub audio: Lane,
}

/// Datei in einem Ablegevorgang (Vorschau oder Platzhalter)
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub audio: bool,
    /// Länge auf der Zeitleiste; `None` = noch unbekannt
    pub dur: Option<Time>,
}

pub enum State {
    Probing,
    Ready(MediaRef),
    Failed,
}

pub struct Entry {
    pub path: PathBuf,
    pub audio: bool,
    pub state: State,
}

/// Abgelegte Dateien, die gerade geprüft werden
pub struct Pending {
    pub target: Option<Target>,
    pub entries: Vec<Entry>,
    rx: Receiver<(usize, Result<MediaRef, String>)>,
}

/// Zustand des Ablegens von außen (in [`App`])
#[derive(Default)]
pub struct Drops {
    /// Zeiger in egui-Punkten, solange Dateien über dem Fenster schweben oder abgelegt werden
    pub pos: Option<Pos2>,
    /// schwebende Mediendateien (nach Namen sortiert)
    pub hover: Option<Vec<Item>>,
    /// Ziel, das die Zeitleiste gerade zeigt
    pub target: Option<Target>,
    /// Fläche der Zeitleiste im letzten Bild
    pub timeline_rect: Option<Rect>,
    /// war der Mauszeiger zuletzt (im Fenster) über der Zeitleiste?
    pub over_timeline: bool,
    pub list: Vec<Pending>,
    next: u64,
}

// ------------------------------------------------------------------ reine Funktionen

/// Bildschirmpixel → egui-Punkte im Fenster: `origin` = linke obere Ecke des Fensterinhalts in
/// Bildschirmpixeln, `ppp` = Pixel je Punkt (Anzeigeskalierung mal Zoom).
pub fn screen_to_points(cursor: (i32, i32), origin: (i32, i32), ppp: f32) -> Pos2 {
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    pos2((cursor.0 - origin.0) as f32 / ppp, (cursor.1 - origin.1) as f32 / ppp)
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Mediendatei nach Endung (alles andere – Schriften, Untertitel, Projekte – läuft wie bisher)
pub fn is_media(p: &Path) -> bool {
    cl_media::all_media_extensions().contains(&ext(p).as_str())
}

/// Vermutung vor der Prüfung: Ton?
pub fn is_audio(p: &Path) -> bool {
    cl_media::AUDIO_EXTENSIONS.contains(&ext(p).as_str())
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Namensreihenfolge (ohne Groß/Klein), doppelte Pfade einmal
pub fn sort_by_name(paths: &mut Vec<PathBuf>) {
    paths.sort_by_key(|p| (file_name(p).to_lowercase(), p.clone()));
    paths.dedup();
}

/// Startzeiten: je Art hintereinander ab `at`; unbekannte Länge zählt als `default`.
pub fn layout(items: &[Item], at: Time, default: Time) -> Vec<Time> {
    let mut next = [at, at];
    items
        .iter()
        .map(|i| {
            let k = usize::from(i.audio);
            let s = next[k];
            next[k] = s + i.dur.unwrap_or(default);
            s
        })
        .collect()
}

/// Spur für Elemente der Art `k` beim Ablegen an `p` (Dateien von außen und aus dem Medien-Menü):
/// passende Spur → dort (Hauptspur: wie ohne Ziel), unpassende oder gesperrte → neue Spur an
/// dieser Stelle, über bzw. unter allen Spuren → neue Spur. `rows` = Zeilen in
/// Anzeige-Reihenfolge (Spurindex, Fläche), `lanes` = sichtbarer Spurbereich (Zeilen unter dem
/// Lineal zählen nicht), `row_gap` = Abstand zwischen zwei Zeilen.
pub fn lane_at(tracks: &[Track], rows: &[(usize, Rect)], lanes: Rect, p: Pos2, k: TrackKind, row_gap: f32) -> Lane {
    let row = rows
        .iter()
        .position(|(_, r)| p.y >= r.top() - row_gap / 2.0 && p.y < r.bottom() + row_gap / 2.0)
        .filter(|_| lanes.contains(p));
    if let Some(r) = row {
        let t = &tracks[rows[r].0];
        if !fits(t, k) {
            let gap = new_track_gap(tracks, r, p.y < rows[r].1.center().y, k);
            return Lane::New { index: gap_index(tracks, gap, k), gap };
        }
        return if t.main { Lane::Auto } else { Lane::Track(t.id) };
    }
    let top = rows.iter().map(|r| r.1.top()).fold(f32::INFINITY, f32::min);
    let bottom = rows.iter().map(|r| r.1.bottom()).fold(f32::NEG_INFINITY, f32::max);
    let audio = k == TrackKind::Audio;
    if !tracks.is_empty() && ((p.y < top - row_gap && !audio) || (p.y > bottom + row_gap && audio)) {
        Lane::New { index: tracks.len(), gap: if audio { rows.len() } else { 0 } }
    } else {
        Lane::Auto
    }
}

/// Ziel beim Ablegen: mit Zeigerposition das angezeigte (keins = nur importieren); ohne nur
/// dann am Playhead, wenn der Zeiger zuletzt über der Zeitleiste war – sonst importieren.
pub fn decide(pos_known: bool, shown: Option<Target>, over_timeline: bool, playhead: Time) -> Option<Target> {
    if pos_known {
        shown
    } else {
        over_timeline.then_some(Target { at: playhead, video: Lane::Auto, audio: Lane::Auto })
    }
}

// ------------------------------------------------------------------ Zeigerposition

/// Klicktests: Ablegestelle aus `devinput` (egui-Punkte)
pub static DEV_POS: Mutex<Option<Pos2>> = Mutex::new(None);

/// Zeiger in egui-Punkten, während Dateien von außen gezogen werden
fn cursor_pos(ctx: &egui::Context, frame: &eframe::Frame) -> Option<Pos2> {
    if let Some(p) = *DEV_POS.lock().unwrap_or_else(|e| e.into_inner()) {
        return Some(p);
    }
    os_cursor(frame).map(|(c, o)| screen_to_points(c, o, ctx.pixels_per_point()))
}

/// (Zeiger, Ecke des Fensterinhalts) in Bildschirmpixeln
#[cfg(windows)]
fn os_cursor(frame: &eframe::Frame) -> Option<((i32, i32), (i32, i32))> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(p: *mut Point) -> i32;
        fn ClientToScreen(hwnd: isize, p: *mut Point) -> i32;
    }
    let RawWindowHandle::Win32(h) = frame.window_handle().ok()?.as_raw() else {
        return None;
    };
    let (mut c, mut o) = (Point { x: 0, y: 0 }, Point { x: 0, y: 0 });
    // SAFETY: Fenster-Handle von eframe (lebt so lange wie das Fenster), Zeiger auf lokale Werte
    let ok = unsafe { GetCursorPos(&mut c) != 0 && ClientToScreen(h.hwnd.get(), &mut o) != 0 };
    ok.then_some(((c.x, c.y), (o.x, o.y)))
}

#[cfg(not(windows))]
fn os_cursor(_: &eframe::Frame) -> Option<((i32, i32), (i32, i32))> {
    None
}

// ------------------------------------------------------------------ App

/// Gleichzeitige Prüfungen (ffprobe-Prozesse)
const WORKERS: usize = 8;

impl Pending {
    /// Platzhalter in Ablegereihenfolge (gescheiterte fehlen)
    pub fn items(&self, image: Time) -> Vec<Item> {
        self.entries
            .iter()
            .filter_map(|e| {
                let dur = match &e.state {
                    State::Failed => return None,
                    State::Probing => None,
                    State::Ready(m) => Some(clip_len(m, image)),
                };
                Some(Item { name: file_name(&e.path), audio: e.audio, dur })
            })
            .collect()
    }
}

/// Länge des Clips wie bei [`App::add_media_at`]
fn clip_len(m: &MediaRef, image: Time) -> Time {
    if m.kind == MediaKind::Image || m.duration <= Time::ZERO {
        image
    } else {
        m.duration
    }
}

impl App {
    fn image_len(&self) -> Time {
        Time::from_secs_f64(self.settings.image_duration as f64)
    }

    /// Je Bild vor dem Zeichnen: Zeigerposition und schwebende Dateien erfassen.
    pub fn drops_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let (hovered, dropped, pointer) = ctx.input(|i| {
            let h: Vec<PathBuf> = i.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect();
            (h, !i.raw.dropped_files.is_empty(), i.pointer.hover_pos())
        });
        if let Some(p) = pointer {
            self.drops.over_timeline = self.drops.timeline_rect.is_some_and(|r| r.contains(p));
        }
        self.drops.pos = (!hovered.is_empty() || dropped).then(|| cursor_pos(ctx, frame)).flatten();
        if !dropped {
            self.drops.target = None;
        }
        let mut media: Vec<PathBuf> = hovered.into_iter().filter(|p| is_media(p)).collect();
        sort_by_name(&mut media);
        let image = self.image_len();
        self.drops.hover = (!media.is_empty()).then(|| {
            media
                .iter()
                .map(|p| match self.project.media.iter().find(|m| m.path == *p) {
                    Some(m) => Item { name: file_name(p), audio: m.kind == MediaKind::Audio, dur: Some(clip_len(m, image)) },
                    None => Item { name: file_name(p), audio: is_audio(p), dur: None },
                })
                .collect()
        });
        // während des Ziehens von außen kommen keine Mausereignisse: selbst nachsehen
        if self.drops.hover.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    /// Ziel für gerade abgelegte Dateien (siehe [`decide`])
    pub fn drop_target(&mut self) -> Option<Target> {
        decide(self.drops.pos.is_some(), self.drops.target.take(), self.drops.over_timeline, self.playhead)
    }

    /// Mediendateien im Hintergrund prüfen; mit Ziel danach an der Stelle einfügen.
    pub fn start_file_drop(&mut self, ctx: &egui::Context, mut paths: Vec<PathBuf>, target: Option<Target>) {
        sort_by_name(&mut paths);
        let entries: Vec<Entry> = paths
            .into_iter()
            .map(|path| match self.project.media.iter().find(|m| m.path == path) {
                Some(m) => Entry { audio: m.kind == MediaKind::Audio, state: State::Ready(m.clone()), path },
                None => Entry { audio: is_audio(&path), state: State::Probing, path },
            })
            .collect();
        let todo: Vec<(usize, PathBuf)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.state, State::Probing))
            .map(|(i, e)| (i, e.path.clone()))
            .collect();
        let (tx, rx) = channel();
        let id = self.drops.next;
        self.drops.next += 1;
        if !todo.is_empty() {
            let label = self.i18n.t_num("files-importing", "count", todo.len() as i64);
            self.tasks.spawn(ctx, format!("drop:{id}"), label, move |report, cancel| {
                probe_all(&todo, |i, r| drop(tx.send((i, r))), report, cancel);
                TaskDone::Run(Box::new(|_| {}))
            });
        }
        self.drops.list.push(Pending { target, entries, rx });
        self.poll_file_drops();
    }

    /// Je Bild: Prüfergebnisse übernehmen; ist alles geprüft, einfügen.
    pub fn poll_file_drops(&mut self) {
        let mut i = 0;
        while i < self.drops.list.len() {
            let mut failed = Vec::new();
            let d = &mut self.drops.list[i];
            let gone = loop {
                match d.rx.try_recv() {
                    Ok((k, r)) => {
                        let Some(e) = d.entries.get_mut(k) else { continue };
                        e.state = match r {
                            Ok(m) => {
                                e.audio = m.kind == MediaKind::Audio;
                                State::Ready(m)
                            }
                            Err(err) => {
                                failed.push((file_name(&e.path), err));
                                State::Failed
                            }
                        };
                    }
                    Err(TryRecvError::Empty) => break false,
                    Err(TryRecvError::Disconnected) => break true,
                }
            };
            let done = !d.entries.iter().any(|e| matches!(e.state, State::Probing));
            for (file, error) in failed {
                self.toast_err(self.i18n.t_args("media-probe-error", &[("file", file.into()), ("error", error.into())]));
            }
            if done {
                let d = self.drops.list.remove(i);
                self.commit_file_drop(d);
            } else if gone {
                // abgebrochen: nichts einfügen
                self.drops.list.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Geprüfte Dateien übernehmen (Medien + Clips) – alles ein Verlaufsschritt.
    pub fn commit_file_drop(&mut self, d: Pending) {
        let ready: Vec<MediaRef> = d
            .entries
            .into_iter()
            .filter_map(|e| match e.state {
                State::Ready(m) => Some(m),
                _ => None,
            })
            .collect();
        if ready.is_empty() {
            return;
        }
        let n = self.one_step(|s| {
            let mut ids = Vec::new();
            for m in ready {
                // schon im Projekt (auch inzwischen importiert)? Dann wiederverwenden
                if let Some(have) = s.project.media.iter().find(|x| x.path == m.path) {
                    ids.push(have.id);
                } else {
                    let id = m.id;
                    if s.apply(EditOp::ImportMedia { media: m }).is_some() {
                        ids.push(id);
                    }
                }
            }
            if let Some(t) = d.target {
                s.place_dropped(&ids, t);
            }
            ids.len()
        });
        if d.target.is_none() && n > 0 {
            self.toast(self.i18n.t_num("media-imported", "count", n as i64));
        }
    }

    /// Medien ab `t.at` hintereinander einfügen: Bild/Video in eine Spur, Ton in eine Tonspur.
    fn place_dropped(&mut self, ids: &[cl_core::MediaId], t: Target) {
        let lanes = [t.video, t.audio];
        // je Art: Spur (sobald bekannt) und nächste Startzeit
        let mut track: [Option<Option<TrackId>>; 2] = [None, None];
        let mut next = [t.at, t.at];
        // neue Spuren vorab, höchster Index zuerst: sonst verschiebt die erste den Index der zweiten
        let used = |k: usize| ids.iter().any(|&id| self.project.media(id).is_some_and(|m| usize::from(m.kind == MediaKind::Audio) == k));
        let mut new: Vec<(usize, usize)> = (0..2)
            .filter_map(|k| match lanes[k] {
                Lane::New { index, .. } if used(k) => Some((index, k)),
                _ => None,
            })
            .collect();
        new.sort_by_key(|n| std::cmp::Reverse(n.0));
        for (index, k) in new {
            let kind = if k == 1 { TrackKind::Audio } else { TrackKind::Video };
            if let Some(OpOutput::Track(tid)) = self.apply(EditOp::InsertTrack { index, kind }) {
                self.timeline.reveal_track(tid);
                track[k] = Some(Some(tid));
            }
        }
        for &id in ids {
            let Some(m) = self.project.media(id) else { continue };
            let k = usize::from(m.kind == MediaKind::Audio);
            let tr = match (track[k], lanes[k]) {
                (Some(tr), _) => tr,
                (None, Lane::Track(tid)) if self.project.timeline.tracks.iter().any(|x| x.id == tid) => Some(tid),
                _ => None,
            };
            if let Some(c) = self.add_media_at(id, next[k], tr) {
                if let Some((x, c)) = self.project.find_clip(c) {
                    next[k] = c.end();
                    track[k] = Some(Some(x.id));
                }
            }
        }
    }
}

/// Dateien parallel prüfen; `send(i, Ergebnis)` je Datei, sofort.
fn probe_all(
    todo: &[(usize, PathBuf)],
    send: impl Fn(usize, Result<MediaRef, String>),
    report: &dyn Fn(f32),
    cancel: &std::sync::atomic::AtomicBool,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let (tx, rx) = channel();
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(todo.len()).max(1) {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some((i, p)) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else { return };
                let _ = tx.send((*i, cl_media::probe(p).map_err(|e| e.to_string())));
            });
        }
        drop(tx);
        // Ergebnisse und Fortschritt aus diesem Thread weiterreichen
        for (n, (i, r)) in rx.iter().enumerate() {
            send(i, r);
            report((n + 1) as f32 / todo.len() as f32);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{Clip, ClipContent, MediaId, Project};
    use eframe::egui::vec2;

    fn item(audio: bool, secs: Option<f64>) -> Item {
        Item { name: String::new(), audio, dur: secs.map(Time::from_secs_f64) }
    }

    #[test]
    fn screen_to_window_with_scaling() {
        assert_eq!(screen_to_points((500, 300), (100, 100), 1.0), pos2(400.0, 200.0));
        // 150 % Anzeigeskalierung
        assert_eq!(screen_to_points((400, 250), (100, 100), 1.5), pos2(200.0, 100.0));
        // Fenster auf einem linken Bildschirm (negative Koordinaten), Zeiger links davon
        assert_eq!(screen_to_points((-1950, 10), (-1920, 0), 2.0), pos2(-15.0, 5.0));
        assert_eq!(screen_to_points((10, 10), (0, 0), 0.0), pos2(10.0, 10.0));
    }

    #[test]
    fn files_by_name_and_kind() {
        let mut v: Vec<PathBuf> = ["b/Zeta.mp4", "a/alpha.WAV", "c/Beta.png", "a/alpha.WAV"].iter().map(PathBuf::from).collect();
        sort_by_name(&mut v);
        let names: Vec<String> = v.iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["alpha.WAV", "Beta.png", "Zeta.mp4"]);
        assert!(is_audio(Path::new("x/alpha.WAV")) && !is_audio(Path::new("Beta.png")));
        assert!(is_media(Path::new("a.gif")) && !is_media(Path::new("a.srt")) && !is_media(Path::new("a.ttf")));
    }

    #[test]
    fn sequence_per_kind_and_shift_when_lengths_arrive() {
        let at = Time::from_secs_f64(10.0);
        let d = Time::from_secs_f64(5.0);
        let s = |v: Vec<Time>| v.iter().map(|t| t.as_secs_f64()).collect::<Vec<_>>();
        // Video, Ton, Bild, Ton: Bild/Video hintereinander, Ton für sich
        let mut items = vec![item(false, None), item(true, None), item(false, Some(3.0)), item(true, None)];
        assert_eq!(s(layout(&items, at, d)), [10.0, 10.0, 15.0, 15.0]);
        // erste Länge bekannt → Nachfolger der Art rücken nach, die andere Art bleibt
        items[0].dur = Some(Time::from_secs_f64(20.0));
        assert_eq!(s(layout(&items, at, d)), [10.0, 10.0, 30.0, 15.0]);
        // gescheiterte Datei fällt weg (Platzhalter verschwindet) → Rest rückt auf
        items.remove(1);
        assert_eq!(s(layout(&items, at, d)), [10.0, 30.0, 10.0]);
    }

    #[test]
    fn without_position_only_over_timeline() {
        let ph = Time::from_secs_f64(4.0);
        let shown = Some(Target { at: Time::from_secs_f64(1.0), video: Lane::Auto, audio: Lane::Auto });
        assert_eq!(decide(true, shown, false, ph), shown);
        assert_eq!(decide(true, None, true, ph), None);
        assert_eq!(decide(false, shown, false, ph), None);
        assert_eq!(decide(false, None, true, ph).map(|t| t.at), Some(ph));
    }

    /// Spuren: Text (oben), Video, Hauptspur, Ton – mit eingeklappter Textspur (schmal)
    fn tracks() -> (Vec<Track>, Vec<(usize, Rect)>) {
        let mut main = Track::new(TrackKind::Video);
        main.main = true;
        let tracks = vec![main, Track::new(TrackKind::Video), Track::new(TrackKind::Text), Track::new(TrackKind::Audio)];
        let order = cl_core::display_order(&tracks);
        let mut y = 100.0;
        let rows = order
            .iter()
            .map(|&i| {
                let h = if i == 2 { 12.0 } else { 34.0 };
                let r = Rect::from_min_size(pos2(0.0, y), vec2(800.0, h));
                y += h + 5.0;
                (i, r)
            })
            .collect();
        (tracks, rows)
    }

    #[test]
    fn pointer_to_lane() {
        let (t, rows) = tracks();
        let at = |r: usize, dy: f32| pos2(300.0, rows[r].1.top() + dy);
        let v = TrackKind::Video;
        let a = TrackKind::Audio;
        // Zeile 0 = eingeklappte Textspur: Video passt nicht → neue Spur darüber/darunter
        assert!(matches!(lane_at(&t, &rows, Rect::EVERYTHING, at(0, 2.0), v, 5.0), Lane::New { gap: 0, .. }));
        assert!(matches!(lane_at(&t, &rows, Rect::EVERYTHING, at(0, 10.0), v, 5.0), Lane::New { gap: 1, .. }));
        // Zeile 1 = Videospur: dort; Hauptspur: wie ohne Ziel
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, at(1, 10.0), v, 5.0), Lane::Track(t[1].id));
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, at(2, 10.0), v, 5.0), Lane::Auto);
        // Ton über der Videospur → neue Tonspur (nur unter den Bildspuren erlaubt)
        assert!(matches!(lane_at(&t, &rows, Rect::EVERYTHING, at(1, 10.0), a, 5.0), Lane::New { gap: 3, .. }));
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, at(3, 10.0), a, 5.0), Lane::Track(t[3].id));
        // gesperrte passende Spur → neue
        let mut l = t.clone();
        l[3].locked = true;
        assert!(matches!(lane_at(&l, &rows, Rect::EVERYTHING, at(3, 10.0), a, 5.0), Lane::New { .. }));
        // über allen Spuren: neues Bild oben; unter allen: neuer Ton unten; sonst wie ohne Ziel
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, pos2(0.0, 50.0), v, 5.0), Lane::New { index: 4, gap: 0 });
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, pos2(0.0, 50.0), a, 5.0), Lane::Auto);
        assert_eq!(lane_at(&t, &rows, Rect::EVERYTHING, pos2(0.0, 400.0), a, 5.0), Lane::New { index: 4, gap: 4 });
    }

    /// Senkrecht gerollt: die Textspur liegt halb unter dem Lineal. Über dem Lineal gilt sie nicht
    /// (Video wie ohne Ziel auf die Hauptspur, wie beim Ablegen aus dem Medien-Menü), im sichtbaren
    /// Teil schon (Video passt nicht → neue Spur darunter).
    #[test]
    fn rows_under_the_ruler_do_not_count() {
        let (t, rows) = tracks();
        let lanes = Rect::from_min_max(pos2(0.0, 106.0), pos2(800.0, 400.0));
        let v = TrackKind::Video;
        assert_eq!(lane_at(&t, &rows, lanes, pos2(300.0, 103.0), v, 5.0), Lane::Auto);
        assert!(matches!(lane_at(&t, &rows, lanes, pos2(300.0, 109.0), v, 5.0), Lane::New { gap: 1, .. }));
    }

    fn media(kind: MediaKind, secs: f64, name: &str) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: PathBuf::from(name),
            kind,
            duration: Time::from_secs_f64(secs),
            width: 64,
            height: 36,
            fps: None,
            has_audio: kind == MediaKind::Audio,
            audio_streams: u32::from(kind == MediaKind::Audio),
            video_codec: None,
            audio_codec: None,
        }
    }

    fn test_app() -> (App, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let app = App::new(&eframe::CreationContext::_new_kittest(egui::Context::default()), None);
        (app, home, g)
    }

    fn pending(target: Option<Target>, states: Vec<State>) -> Pending {
        let (_tx, rx) = channel();
        let entries = states
            .into_iter()
            .enumerate()
            .map(|(i, state)| Entry { path: PathBuf::from(format!("f{i}")), audio: false, state })
            .collect();
        Pending { target, entries, rx }
    }

    /// Drei Dateien, eine scheitert: Medien + neue Tonspur + Clips sind EIN Schritt, Rückgängig
    /// stellt das Projekt genau wieder her; eine Datei, die schon im Projekt ist, wird genutzt.
    #[test]
    fn drop_is_one_undo_step_even_with_a_failure() {
        let (mut app, _home, _g) = test_app();
        let mut p = Project::new("Ablegen");
        let have = media(MediaKind::Video, 4.0, "schon.mp4");
        let mut main = Track::new(TrackKind::Video);
        main.main = true;
        main.clips.push(Clip::new(ClipContent::Media { media: have.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0)));
        p.timeline.tracks.push(main);
        p.media.push(have.clone());
        app.set_doc(p);
        let before = app.project.clone();

        let mut again = media(MediaKind::Video, 4.0, "schon.mp4");
        again.id = MediaId::new();
        let video = media(MediaKind::Video, 6.0, "b.mp4");
        let tone = media(MediaKind::Audio, 3.0, "c.wav");
        let at = Time::from_secs_f64(4.0);
        let target = Target { at, video: Lane::Auto, audio: Lane::New { index: 1, gap: 1 } };
        let d = pending(Some(target), vec![State::Ready(again), State::Failed, State::Ready(video.clone()), State::Ready(tone.clone())]);
        app.commit_file_drop(d);

        let p = &app.project;
        assert_eq!(p.media.len(), 3, "kein doppelter Medien-Eintrag");
        assert_eq!(p.timeline.tracks.len(), 2, "neue Tonspur");
        let main = &p.timeline.tracks[0];
        let starts: Vec<f64> = main.clips.iter().map(|c| c.start.as_secs_f64()).collect();
        assert_eq!(starts, [0.0, 4.0, 8.0], "Video hintereinander ab der Ablegestelle");
        let audio = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap();
        assert_eq!(audio.clips.len(), 1);
        assert_eq!(audio.clips[0].start, at);

        // ein Rückgängig nimmt alles zurück = ein Verlaufsschritt
        app.undo();
        app.sync_project();
        assert_eq!(app.project.media, before.media);
        assert_eq!(app.project.timeline.tracks, before.timeline.tracks);
    }

    /// Ohne Ziel nur importieren; alles gescheitert → nichts geändert
    #[test]
    fn drop_without_target_imports_only() {
        let (mut app, _home, _g) = test_app();
        app.set_doc(Project::new("Import"));
        let before = app.project.clone();
        app.commit_file_drop(pending(None, vec![State::Failed]));
        assert_eq!(app.project, before);
        app.commit_file_drop(pending(None, vec![State::Ready(media(MediaKind::Image, 0.0, "a.png")), State::Ready(media(MediaKind::Audio, 2.0, "b.wav"))]));
        assert_eq!(app.project.media.len(), 2);
        assert!(app.project.timeline.tracks.iter().all(|t| t.clips.is_empty()));
        app.undo();
        app.sync_project();
        assert_eq!(app.project, before);
    }

    /// Eine abgelegte Schrift wird übernommen: Textclips der Zeitleiste werden neu gesetzt, nicht
    /// erst nach der nächsten Änderung.
    #[test]
    fn new_fonts_redraw_text_clips() {
        let (mut app, _home, _g) = test_app();
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            app.register_fonts(ctx);
            let before = app.project_epoch;
            app.apply_pending_fonts(ctx);
            assert!(app.project_epoch > before);
        });
    }

    /// Spuren T, M, A1 (gesperrt), A2; Video und Ton auf die untere Hälfte von A1: neue Videospur
    /// über M, neue Tonspur direkt unter A1 – wie die Vorschau sie zeigt.
    #[test]
    fn two_new_tracks_land_where_shown() {
        let (mut app, _home, _g) = test_app();
        let mut p = Project::new("Zwei neue");
        let mut main = Track::new(TrackKind::Video);
        main.main = true;
        let mut a1 = Track::new(TrackKind::Audio);
        a1.locked = true;
        p.timeline.tracks = vec![main, Track::new(TrackKind::Text), a1, Track::new(TrackKind::Audio)];
        app.set_doc(p);
        let tracks = app.project.timeline.tracks.clone();
        let rows: Vec<(usize, Rect)> = cl_core::display_order(&tracks)
            .iter()
            .enumerate()
            .map(|(n, &i)| (i, Rect::from_min_size(pos2(0.0, 100.0 + n as f32 * 40.0), vec2(800.0, 34.0))))
            .collect();
        let p = pos2(300.0, rows[2].1.bottom() - 4.0);
        let target = Target {
            at: Time::ZERO,
            video: lane_at(&tracks, &rows, Rect::EVERYTHING, p, TrackKind::Video, 6.0),
            audio: lane_at(&tracks, &rows, Rect::EVERYTHING, p, TrackKind::Audio, 6.0),
        };
        assert!(matches!((target.video, target.audio), (Lane::New { .. }, Lane::New { .. })));
        let d = pending(Some(target), vec![State::Ready(media(MediaKind::Video, 2.0, "a.mp4")), State::Ready(media(MediaKind::Audio, 2.0, "b.wav"))]);
        app.commit_file_drop(d);
        let t = &app.project.timeline.tracks;
        let shown: Vec<(TrackKind, bool, usize)> =
            cl_core::display_order(t).iter().map(|&i| (t[i].kind, t[i].locked, t[i].clips.len())).collect();
        use TrackKind::{Audio, Text, Video};
        assert_eq!(shown, [(Text, false, 0), (Video, false, 1), (Video, false, 0), (Audio, true, 0), (Audio, false, 1), (Audio, false, 0)]);
    }

    /// Ablage und Aufgabe laufen noch, als „Zur Startseite“ gedrückt wird: ihr Ergebnis darf das
    /// verlassene Projekt nicht mehr ändern (es würde später ohne Sperre gespeichert).
    #[test]
    fn results_after_going_home_do_not_touch_the_left_project() {
        let (mut app, _home, _g) = test_app();
        app.set_doc(Project::new("Verlassen"));
        let (tx, rx) = channel();
        let entry = Entry { path: PathBuf::from("spät.mp4"), audio: false, state: State::Probing };
        app.drops.list.push(Pending { target: None, entries: vec![entry], rx });
        let (go_tx, go_rx) = channel::<()>();
        app.tasks.spawn(&egui::Context::default(), "spät", "Spät", move |_, _| {
            let _ = go_rx.recv();
            TaskDone::Run(Box::new(|app: &mut App| {
                app.apply(EditOp::ImportMedia { media: media(MediaKind::Image, 0.0, "aufgabe.png") });
            }))
        });
        app.go_home();
        // mit der Korrektur ist der Empfänger schon weg
        let _ = tx.send((0, Ok(media(MediaKind::Video, 2.0, "spät.mp4"))));
        go_tx.send(()).unwrap();
        app.poll_file_drops();
        while !app.tasks.list.is_empty() {
            app.poll_tasks();
            std::thread::yield_now();
        }
        app.sync_project();
        assert!(app.project.media.is_empty(), "{:?}", app.project.media);
    }
}
