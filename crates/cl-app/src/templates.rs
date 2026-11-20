//! Project templates: save the open project as a template (intro, lower thirds, outro,
//! end card, format), create new projects from it on the start page, rename
//! and remove templates. Storage: `<data>/Templates/<name>.cutline`.

use std::path::PathBuf;

use cl_core::{AnimRef, Clip, ClipContent, ClipId, Document, EditOp, OpSource, Project, TextStyle, Time, TrackId, TrackKind};
use eframe::egui::{self, RichText};

use crate::app::{App, Screen};
use crate::paths;
use crate::theme::{self, icon};

fn dir() -> PathBuf {
    paths::data_dir().join("Templates")
}

pub fn list() -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
        .map(|p| {
            (
                p.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                p,
            )
        })
        .collect();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// Templates are named like their file: punctuation stays (existing names stay the same)
fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    paths::fit_name(&s)
}

#[derive(Clone)]
pub enum TplDialog {
    Save { name: String },
    Rename { path: PathBuf, name: String },
    /// „Aus Vorlage einfügen“: list of all templates (loaded on open) with insert position
    Insert(Vec<(String, Project)>),
}

/// Bundled templates (not deletable), generated in code.
const BUILTIN: [&str; 3] = ["Intro-Titel", "Endkarte", "Kapitel-Titel"];

fn builtin(i: usize) -> Project {
    // (text, size, bold, background, y, fade-in in s)
    type Part = (&'static str, f32, bool, Option<[u8; 4]>, f32, f64);
    let (secs, parts): (f64, Vec<Part>) = match i {
        0 => (3.0, vec![("Dein Titel", 120.0, true, None, 0.0, 1.0), ("Untertitel", 56.0, false, None, 110.0, 1.0)]),
        1 => (
            8.0,
            vec![
                ("Danke fürs Zuschauen", 110.0, true, Some([0, 0, 0, 200]), -60.0, 1.0),
                ("Abonnieren nicht vergessen", 56.0, false, Some([200, 30, 30, 255]), 100.0, 1.0),
            ],
        ),
        _ => (2.0, vec![("Kapitel 1", 100.0, true, Some([0, 0, 0, 160]), 0.0, 0.4)]),
    };
    let mut d = Document::new(Project::new(BUILTIN[i]));
    for (text, size, bold, background, y, fade) in parts {
        let style = TextStyle { text: text.into(), size, bold, background, ..TextStyle::default() };
        let mut c = Clip::new(ClipContent::Text(style), Time::ZERO, Time::from_secs_f64(secs));
        c.transform.y = y;
        c.anim.intro = Some(AnimRef { id: "fade".into(), duration: Time::from_secs_f64(fade) });
        let _ = d.apply(EditOp::AddClip { track: None, clip: c }, OpSource::Ui);
    }
    d.project().clone()
}

/// All readable templates for the insert dialog: bundled first, then own ones.
pub fn load_all() -> Vec<(String, Project)> {
    let own = list().into_iter().filter_map(|(n, p)| {
        let s = std::fs::read_to_string(p).ok()?;
        Some((n, Project::from_json(&s).ok()?))
    });
    (0..BUILTIN.len()).map(|i| (BUILTIN[i].to_string(), builtin(i))).chain(own).collect()
}

/// Side track for the template's image overlays: a free one, otherwise any unlocked video track
/// except the main track (`AddClip` creates a track above it itself when occupied).
fn overlay_track(target: &Project, start: Time, dur: Time) -> Option<TrackId> {
    let mut cands = target.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.main && !t.locked);
    let first = cands.clone().next().map(|t| t.id);
    cands.find(|t| t.is_free(start, dur, None)).map(|t| t.id).or(first)
}

/// Does inserting need a new overlay track first (template has image overlays, target no side track)?
pub fn needs_overlay_track(tpl: &Project, target: &Project) -> bool {
    let overlays = tpl.timeline.tracks.iter().filter(|t| !t.main).flat_map(|t| &t.clips).any(|c| c.kind() == TrackKind::Video);
    overlays && overlay_track(target, Time::ZERO, Time::ZERO).is_none()
}

/// Convert template + target project + time -> ops (for one batch): add media,
/// push content from `at` back by the template length, place template clips.
/// `AddClip` creates missing tracks itself.
/// Templates without main track clips (title, end card) are overlays: they move nothing.
/// shortcut: clips spanning `at` stay in place; the main track with magnet only moves
/// through inserted main track clips.
pub fn insert_ops(tpl: &Project, target: &Project, at: Time) -> Vec<EditOp> {
    let len = tpl.duration();
    let pushes = tpl.timeline.tracks.iter().any(|t| t.main && !t.clips.is_empty());
    let mut ops: Vec<EditOp> = tpl.media.iter().map(|m| EditOp::ImportMedia { media: m.clone() }).collect();
    for t in target.timeline.tracks.iter().filter(|t| pushes && !t.locked && !(t.main && target.settings.main_track_magnet)) {
        let mut v: Vec<&Clip> = t.clips.iter().filter(|c| c.start >= at).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.start)); // last first, so nothing collides
        ops.extend(v.into_iter().map(|c| EditOp::MoveClip { clip: c.id, track: Some(t.id), start: c.start + len }));
    }
    // Groups new per insertion: the same template twice gives two groups
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1);
    let mut next = target.timeline.tracks.iter().flat_map(|t| &t.clips).filter_map(|c| c.group).max().unwrap_or(0).max(now);
    let mut groups = std::collections::HashMap::new();
    for t in &tpl.timeline.tracks {
        for c in &t.clips {
            let mut n = c.clone();
            n.id = ClipId::new();
            n.start = c.start + at;
            n.group = c.group.map(|g| {
                *groups.entry(g).or_insert_with(|| {
                    next += 1;
                    next
                })
            });
            // only the template's main track clips go to the main track, image overlays to a side track
            let track = if !t.main && n.kind() == TrackKind::Video { overlay_track(target, n.start, n.duration) } else { None };
            ops.push(EditOp::AddClip { track, clip: n });
        }
    }
    ops
}

/// Multicam segment: (disabled, opacity factor, muted). The audio source always plays
/// (not active: only the picture disappears), the other angles are muted.
pub fn cam_piece(angle: usize, audio: usize, active: bool) -> (bool, f32, bool) {
    if angle == audio {
        (false, if active { 1.0 } else { 0.0 }, false)
    } else {
        (!active, 1.0, true)
    }
}

/// Entry in the template list on the start page: left-aligned like the column's other entries
/// (add_sized centers), a long name on one line shortened with „…“, in full in the tooltip.
fn template_row(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let name = crate::tr(name);
    let full = format!("{}  {name}", icon::COPY_SIMPLE);
    let font = egui::FontId::proportional(theme::FS_UI);
    let max = ui.available_width() - 2.0 * ui.spacing().button_padding.x;
    let shown = theme::elide(&full, max, |t| theme::text_width(ui.ctx(), t, font.clone()));
    let r = ui
        .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
            ui.add(theme::ghost(shown.clone()).min_size(egui::vec2(0.0, 26.0)))
        })
        .inner;
    if shown != full {
        r.on_hover_text(name)
    } else {
        r
    }
}

impl App {
    /// Start page: list of templates under „Neues Projekt“.
    pub fn home_templates(&mut self, ui: &mut egui::Ui) {
        let list = list();
        ui.add_space(8.0);
        ui.label(
            RichText::new(crate::tr("Aus Vorlage"))
                .small()
                .color(theme::TEXT_DIM()),
        );
        let mut open: Option<PathBuf> = None;
        let mut rename: Option<(PathBuf, String)> = None;
        let mut remove: Option<PathBuf> = None;
        let row = template_row;
        for i in 0..BUILTIN.len() {
            if row(ui, BUILTIN[i]).clicked() {
                self.new_from_project(builtin(i));
            }
        }
        for (name, p) in list {
            let r = row(ui, &name);
            if r.clicked() {
                open = Some(p.clone());
            }
            r.context_menu(|ui| {
                if crate::timeline::menu_item(ui, icon::PENCIL_SIMPLE, "Umbenennen", "") {
                    rename = Some((p.clone(), name.clone()));
                }
                if crate::timeline::menu_item(ui, icon::TRASH, "Entfernen", "") {
                    remove = Some(p.clone());
                }
            });
        }
        if let Some(p) = open {
            self.new_from_template(&p);
        }
        if let Some((path, name)) = rename {
            self.tpl_dialog = Some(TplDialog::Rename { path, name });
        }
        if let Some(p) = remove {
            let trash = paths::data_dir().join("Papierkorb");
            let _ = std::fs::create_dir_all(&trash);
            // unique name: an earlier removed template of the same name stays kept
            let to = crate::library::unique(&trash, p.file_name().unwrap_or_default().to_string_lossy().into_owned());
            if std::fs::rename(&p, to).is_ok() {
                self.toast("Vorlage in den Papierkorb verschoben");
            }
        }
    }

    pub fn new_from_template(&mut self, p: &std::path::Path) {
        match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()))
        {
            Ok(proj) => self.new_from_project(proj),
            Err(e) => self.toast_err(e),
        }
    }

    fn new_from_project(&mut self, mut proj: Project) {
        if !self.new_project() {
            return;
        }
        proj.name = crate::home::next_project_name();
        self.set_doc(proj);
        self.screen = Screen::Editor;
        self.toast("Neues Projekt aus Vorlage");
    }

    /// Insert a template into the open project (`mode`: 0 start, 1 playhead, 2 end), one undo step.
    fn insert_template(&mut self, tpl: &Project, mode: u8) {
        let at = match mode {
            0 => Time::ZERO,
            1 => self.playhead,
            _ => self.project.duration(),
        };
        // shortcut: if an overlay track is missing, creating it is a separate undo step (track IDs
        // only arise when applying); extend once a batch can name new tracks
        let added = needs_overlay_track(tpl, &self.project)
            && self.apply(EditOp::InsertTrack { index: self.project.timeline.tracks.len(), kind: TrackKind::Video }).is_some();
        let ops = insert_ops(tpl, &self.project, at);
        if !ops.is_empty() && self.apply(EditOp::Batch { label: "Vorlage einfügen".into(), ops }).is_some() {
            self.toast("Vorlage eingefügt");
        } else if added {
            // Insert failed: remove the empty track again
            let _ = self.doc.lock().unwrap_or_else(|e| e.into_inner()).undo();
            self.sync_project();
        }
    }

    fn tpl_insert_dialog(&mut self, ctx: &egui::Context) {
        let Some(TplDialog::Insert(list)) = &self.tpl_dialog else {
            return;
        };
        let mut pick: Option<(Project, u8)> = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        crate::dialog::window(ctx, crate::tr("Aus Vorlage einfügen"), "tpl-insert").anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0)).show(ctx, |ui| {
            close |= crate::dialog::title(ui, "Aus Vorlage einfügen");
            for (name, proj) in list {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::tr(name.as_str())).size(theme::FS_UI));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (mode, t) in [(2u8, "Am Ende"), (1, "Am Playhead"), (0, "Am Anfang")] {
                            if ui.add(theme::secondary(t)).clicked() {
                                pick = Some((proj.clone(), mode));
                            }
                        }
                    });
                });
            }
        });
        if let Some((proj, mode)) = pick {
            self.insert_template(&proj, mode);
            close = true;
        }
        if close {
            self.tpl_dialog = None;
        }
    }

    /// Multicam hint window: explanation and audio source while the mode is active.
    fn multicam_hint(&mut self, ctx: &egui::Context) {
        let Some(n) = self.multicam.as_ref().map(|m| m.0.len()) else {
            return;
        };
        let mut sel = self.multicam_audio.min(n - 1);
        let mut cancel = false;
        crate::dialog::window(ctx, crate::tr("Multicam"), "multicam-hint").anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 60.0)).show(ctx, |ui| {
            ui.set_max_width(300.0);
            cancel |= crate::dialog::title(ui, "Multicam");
            ui.label(RichText::new(crate::tr("Multicam: Wiedergabe starten und mit 1–9 den Blickwinkel wählen, Stopp übernimmt den Schnitt")).size(theme::FS_UI));
            ui.add_space(6.0);
            ui.label(RichText::new(crate::tr("Ton von")).small().color(theme::TEXT_DIM()));
            let names: Vec<String> = (1..=n).map(|i| format!("{} {i}", crate::tr("Kamera"))).collect();
            let items: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            crate::widgets::segmented(ui, &items, &mut sel);
            cancel |= crate::dialog::footer(ui, |ui| ui.add(theme::secondary("Abbrechen")).clicked());
        });
        self.multicam_audio = sel;
        if cancel {
            self.multicam = None;
        }
    }

    /// „Als Vorlage speichern“ / „Vorlage umbenennen“ dialog.
    pub fn template_dialog(&mut self, ctx: &egui::Context) {
        self.multicam_hint(ctx);
        if matches!(self.tpl_dialog, Some(TplDialog::Insert(_))) {
            self.tpl_insert_dialog(ctx);
            return;
        }
        let Some(d) = &mut self.tpl_dialog else {
            return;
        };
        let (mut ok, mut close) = (false, false);
        let err = self.dialog_error.clone();
        let mut edited = false;
        let title = match d {
            TplDialog::Save { .. } => "Als Vorlage speichern",
            TplDialog::Rename { .. } => "Vorlage umbenennen",
            TplDialog::Insert(_) => return,
        };
        let modal = crate::dialog::modal(ctx, "tpl-dialog").show(ctx, |ui| {
            close |= crate::dialog::title(ui, title);
            let name = match d {
                TplDialog::Save { name } | TplDialog::Rename { name, .. } => name,
                TplDialog::Insert(_) => return,
            };
            let r = ui.add(egui::TextEdit::singleline(name).hint_text(crate::tr("z. B. Folge mit Intro und Endcard")).desired_width(280.0));
            r.request_focus();
            edited |= r.changed();
            theme::field_error(ui, err.as_deref());
            // the name field always has focus: Enter confirms the input
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            let (m, c) = crate::dialog::buttons(ui, theme::primary("OK"), true);
            ok |= m;
            close |= c;
        });
        if modal.should_close() {
            close = true;
        }
        if edited {
            self.dialog_error = None;
        }
        let backup = self.tpl_dialog.clone();
        if ok {
            self.dialog_error = None;
            match self.tpl_dialog.take() {
                Some(TplDialog::Save { name }) if dir().join(format!("{}.cutline", safe(&name))).exists() => {
                    self.tpl_dialog = backup;
                    self.dialog_error = Some("Diesen Namen gibt es schon".into());
                }
                Some(TplDialog::Save { name }) if !safe(&name).is_empty() => {
                    let _ = std::fs::create_dir_all(dir());
                    let mut p = self.project.clone();
                    p.name = name.trim().to_string();
                    match paths::write_atomic(
                        &dir().join(format!("{}.cutline", safe(&name))),
                        p.to_json(),
                    ) {
                        Ok(_) => self.toast(format!("Vorlage „{}“ gespeichert", name.trim())),
                        Err(e) => {
                            self.tpl_dialog = backup;
                            self.dialog_error = Some(e.to_string());
                        }
                    }
                }
                Some(TplDialog::Rename { path, name }) if !safe(&name).is_empty() => {
                    let to = dir().join(format!("{}.cutline", safe(&name)));
                    // do not replace an existing template (only changing upper/lower case works)
                    let same = to.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
                    if to.exists() && !same {
                        self.tpl_dialog = backup;
                        self.dialog_error = Some("Diesen Namen gibt es schon".into());
                    } else if let Err(e) = std::fs::rename(&path, &to) {
                        self.tpl_dialog = backup;
                        self.dialog_error = Some(e.to_string());
                    }
                }
                _ => {
                    self.tpl_dialog = backup;
                    self.dialog_error = Some("Bitte einen Namen eingeben".into());
                }
            }
        } else if close {
            self.tpl_dialog = None;
            self.dialog_error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{MediaId, MediaKind, MediaRef, Track};

    #[test]
    fn template_names_stay_and_long_or_reserved_ones_fit() {
        assert_eq!(safe("  Intro (v2) & Co. – Ä:1 "), "Intro (v2) & Co. – Ä_1");
        assert_eq!(safe(&"x".repeat(300)).len(), 100);
        assert_eq!(safe("con"), "con_");
    }

    #[test]
    fn long_template_name_stays_on_one_line() {
        let ctx = egui::Context::default();
        let mut h = (0.0, 0.0);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.allocate_ui(egui::vec2(185.0, 400.0), |ui| {
                    h.0 = template_row(ui, "Kurz").rect.height();
                    h.1 = template_row(ui, &"Vorlage mit sehr langem Namen ".repeat(4)).rect.height();
                });
            });
        });
        assert_eq!(h.0, h.1, "langer Name bricht um");
    }

    fn text_track(starts: &[f64]) -> Track {
        let mut t = Track::new(TrackKind::Text);
        for s in starts {
            t.clips.push(Clip::new(ClipContent::Text(TextStyle::default()), Time::from_secs_f64(*s), Time::from_secs_f64(1.0)));
        }
        t
    }

    #[test]
    fn overlay_template_inserts_in_one_step_without_shifting() {
        let mut tpl = Project::new("t");
        let mut tt = text_track(&[]);
        tt.clips.push(Clip::new(ClipContent::Text(TextStyle::default()), Time::ZERO, Time::from_secs_f64(2.0)));
        tpl.timeline.tracks.push(tt);
        let mut target = Project::new("p");
        target.timeline.tracks.push(text_track(&[0.0, 5.0]));
        let ops = insert_ops(&tpl, &target, Time::from_secs_f64(3.0));
        let mut d = Document::new(target);
        let n = d.history().count();
        d.apply(EditOp::Batch { label: String::new(), ops }, OpSource::Ui).unwrap();
        assert_eq!(d.history().count(), n + 1);
        let mut starts: Vec<f64> = d.project().timeline.tracks.iter().flat_map(|t| &t.clips).map(|c| c.start.as_secs_f64()).collect();
        starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(starts, vec![0.0, 3.0, 5.0]);
    }

    fn media(kind: MediaKind) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: "m".into(),
            kind,
            duration: Time::from_secs_f64(60.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        }
    }

    fn clip(m: &MediaRef, start: f64, dur: f64) -> Clip {
        Clip::new(ClipContent::Media { media: m.id, kind: m.kind }, Time::from_secs_f64(start), Time::from_secs_f64(dur))
    }

    /// like `insert_template`: the overlay track first if needed, then the batch
    fn insert(d: &mut Document, tpl: &Project, at: f64) -> cl_core::Result<()> {
        if needs_overlay_track(tpl, d.project()) {
            let n = d.project().timeline.tracks.len();
            d.apply(EditOp::InsertTrack { index: n, kind: TrackKind::Video }, OpSource::Ui)?;
        }
        let ops = insert_ops(tpl, d.project(), Time::from_secs_f64(at));
        d.apply(EditOp::Batch { label: String::new(), ops }, OpSource::Ui).map(|_| ())
    }

    /// Target with a magnet main track (0–5, 5–10) and a template with an intro video (main track) + PNG (track 2)
    fn main_and_overlay() -> (Project, Project, MediaId) {
        let (v, png) = (media(MediaKind::Video), media(MediaKind::Image));
        let mut target = Project::new("p");
        target.settings.main_track_magnet = true;
        target.media.push(v.clone());
        let mut main = Track::new(TrackKind::Video);
        main.main = true;
        main.clips = vec![clip(&v, 0.0, 5.0), clip(&v, 5.0, 5.0)];
        target.timeline.tracks.push(main);
        let mut tpl = Project::new("t");
        tpl.media = vec![v.clone(), png.clone()];
        let mut tmain = Track::new(TrackKind::Video);
        tmain.main = true;
        tmain.clips.push(clip(&v, 0.0, 2.0));
        let mut over = Track::new(TrackKind::Video);
        over.clips.push(clip(&png, 0.0, 2.0));
        tpl.timeline.tracks = vec![tmain, over];
        (target, tpl, png.id)
    }

    #[test]
    fn template_overlay_image_stays_off_the_main_track() {
        let (target, tpl, png) = main_and_overlay();
        let mut d = Document::new(target);
        insert(&mut d, &tpl, 5.0).unwrap();
        let p = d.project();
        let main = p.timeline.tracks.iter().find(|t| t.main).unwrap();
        assert_eq!(main.clips.len(), 3);
        assert_eq!(main.end(), Time::from_secs_f64(12.0));
        let is_png = |c: &Clip| matches!(c.content, ClipContent::Media { media, .. } if media == png);
        assert!(!main.clips.iter().any(is_png));
        let (t, c) = p.timeline.tracks.iter().flat_map(|t| t.clips.iter().map(move |c| (t, c))).find(|(_, c)| is_png(c)).unwrap();
        assert!(!t.main && t.kind == TrackKind::Video);
        assert_eq!(c.start, Time::from_secs_f64(5.0));
        // the second insert uses the existing side track
        let n = d.project().timeline.tracks.len();
        insert(&mut d, &tpl, 0.0).unwrap();
        assert!(d.project().timeline.tracks.len() <= n + 1);
    }

    #[test]
    fn template_into_locked_main_track_fails() {
        let (mut target, tpl, _) = main_and_overlay();
        target.timeline.tracks[0].locked = true;
        let mut d = Document::new(target.clone());
        assert!(insert(&mut d, &tpl, 5.0).is_err());
        assert_eq!(d.project().timeline.tracks[0].clips, target.timeline.tracks[0].clips);
    }

    #[test]
    fn each_template_insert_gets_its_own_group() {
        let mut tpl = Project::new("t");
        let mut tt = text_track(&[0.0, 1.0]);
        for c in &mut tt.clips {
            c.group = Some(7);
        }
        tpl.timeline.tracks.push(tt);
        let mut d = Document::new(Project::new("p"));
        insert(&mut d, &tpl, 0.0).unwrap();
        insert(&mut d, &tpl, 10.0).unwrap();
        let mut g: Vec<(f64, u64)> = d.project().timeline.tracks.iter().flat_map(|t| &t.clips).map(|c| (c.start.as_secs_f64(), c.group.unwrap())).collect();
        g.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(g[0].1, g[1].1);
        assert_eq!(g[2].1, g[3].1);
        assert_ne!(g[0].1, g[2].1);
    }

    #[test]
    fn builtins_have_content_and_cam_audio_follows_main_angle() {
        assert!((0..3).all(|i| builtin(i).duration() > Time::ZERO));
        assert_eq!(cam_piece(0, 0, false), (false, 0.0, false)); // audio plays, picture gone
        assert_eq!(cam_piece(1, 0, true), (false, 1.0, true)); // other angle: picture, muted
        assert_eq!(cam_piece(1, 0, false), (true, 1.0, true));
    }
}
