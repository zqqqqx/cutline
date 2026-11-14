//! Projektvorlagen: das offene Projekt als Vorlage speichern (Intro, Bauchbinden, Outro,
//! Endcard, Format), auf der Startseite neue Projekte daraus anlegen, Vorlagen umbenennen
//! und entfernen. Ablage: `<Daten>/Templates/<Name>.cutline`.

use std::path::PathBuf;

use cl_core::{AnimRef, Clip, ClipContent, ClipId, Document, EditOp, OpSource, Project, TextStyle, Time};
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

fn safe(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect()
}

#[derive(Clone)]
pub enum TplDialog {
    Save { name: String },
    Rename { path: PathBuf, name: String },
    /// „Aus Vorlage einfügen“: Liste aller Vorlagen mit Einfügeposition
    Insert,
}

/// Mitgelieferte Vorlagen (nicht löschbar), im Code erzeugt.
const BUILTIN: [&str; 3] = ["Intro-Titel", "Endkarte", "Kapitel-Titel"];

fn builtin(i: usize) -> Project {
    // (Text, Größe, fett, Hintergrund, y, Einblenden in s)
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

/// Alle Vorlagen für den Einfügen-Dialog: erst mitgelieferte, dann eigene.
fn load_all() -> Vec<(String, Result<Project, String>)> {
    let own = list().into_iter().map(|(n, p)| {
        let r = std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()));
        (n, r)
    });
    (0..BUILTIN.len()).map(|i| (BUILTIN[i].to_string(), Ok(builtin(i)))).chain(own).collect()
}

/// Umrechnung Vorlage + Zielprojekt + Zeit -> Ops (für einen Batch): Medien aufnehmen,
/// Inhalt ab `at` um die Vorlagenlänge nach hinten schieben, Vorlagen-Clips setzen.
/// Fehlende Spuren legt `AddClip` selbst an.
/// Vorlagen ohne Hauptspur-Clips (Titel, Endkarte) sind Überlagerungen: sie verschieben nichts.
/// shortcut: Clips, die `at` überspannen, bleiben liegen; die Hauptspur mit Magnet schiebt sich
/// nur durch eingefügte Hauptspur-Clips.
pub fn insert_ops(tpl: &Project, target: &Project, at: Time) -> Vec<EditOp> {
    let len = tpl.duration();
    let pushes = tpl.timeline.tracks.iter().any(|t| t.main && !t.clips.is_empty());
    let mut ops: Vec<EditOp> = tpl.media.iter().map(|m| EditOp::ImportMedia { media: m.clone() }).collect();
    for t in target.timeline.tracks.iter().filter(|t| pushes && !t.locked && !(t.main && target.settings.main_track_magnet)) {
        let mut v: Vec<&Clip> = t.clips.iter().filter(|c| c.start >= at).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.start)); // hinterste zuerst, damit nichts kollidiert
        ops.extend(v.into_iter().map(|c| EditOp::MoveClip { clip: c.id, track: Some(t.id), start: c.start + len }));
    }
    for c in tpl.timeline.tracks.iter().flat_map(|t| &t.clips) {
        let mut n = c.clone();
        n.id = ClipId::new();
        n.start = c.start + at;
        ops.push(EditOp::AddClip { track: None, clip: n });
    }
    ops
}

/// Multicam-Abschnitt: (deaktiviert, Deckkraft-Faktor, stumm). Die Tonquelle läuft immer
/// (nicht aktiv: nur das Bild verschwindet), die anderen Winkel sind stumm.
pub fn cam_piece(angle: usize, audio: usize, active: bool) -> (bool, f32, bool) {
    if angle == audio {
        (false, if active { 1.0 } else { 0.0 }, false)
    } else {
        (!active, 1.0, true)
    }
}

impl App {
    /// Startseite: Liste der Vorlagen unter „Neues Projekt“.
    pub fn home_templates(&mut self, ui: &mut egui::Ui) {
        let list = list();
        ui.add_space(8.0);
        ui.label(
            RichText::new(crate::tr("Aus Vorlage"))
                .small()
                .color(theme::TEXT_FAINT()),
        );
        let mut open: Option<PathBuf> = None;
        let mut rename: Option<(PathBuf, String)> = None;
        let mut remove: Option<PathBuf> = None;
        // linksbündig wie die übrigen Einträge der Spalte (add_sized zentriert)
        let row = |ui: &mut egui::Ui, name: &str| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                ui.add(theme::ghost(format!("{}  {name}", icon::COPY_SIMPLE)).min_size(egui::vec2(0.0, 26.0)))
            })
            .inner
        };
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
            // eindeutiger Name: eine früher entfernte gleichnamige Vorlage bleibt erhalten
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
        *self.doc.lock().unwrap() = Document::new(proj);
        self.revision = u64::MAX;
        self.sync_project();
        self.screen = Screen::Editor;
        self.toast("Neues Projekt aus Vorlage");
    }

    /// Vorlage ins offene Projekt einfügen (`mode`: 0 Anfang, 1 Playhead, 2 Ende), ein Undo-Schritt.
    fn insert_template(&mut self, tpl: &Project, mode: u8) {
        let at = match mode {
            0 => Time::ZERO,
            1 => self.playhead,
            _ => self.project.duration(),
        };
        let ops = insert_ops(tpl, &self.project, at);
        if !ops.is_empty() {
            self.apply(EditOp::Batch { label: "Vorlage einfügen".into(), ops });
            self.toast("Vorlage eingefügt");
        }
    }

    fn tpl_insert_dialog(&mut self, ctx: &egui::Context) {
        let mut pick: Option<(Project, u8)> = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new(crate::tr("Aus Vorlage einfügen")).id(egui::Id::new("tpl-insert")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0)).show(ctx, |ui| {
            for (name, proj) in load_all() {
                let Ok(proj) = proj else { continue };
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
            if ui.add(theme::secondary("Abbrechen")).clicked() {
                close = true;
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

    /// Multicam-Hinweisfenster: Erklärung und Tonquelle, solange der Modus aktiv ist.
    fn multicam_hint(&mut self, ctx: &egui::Context) {
        let Some(n) = self.multicam.as_ref().map(|m| m.0.len()) else {
            return;
        };
        let mut sel = self.multicam_audio.min(n - 1);
        let mut cancel = false;
        egui::Window::new(crate::tr("Multicam")).id(egui::Id::new("multicam-hint")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 60.0)).show(ctx, |ui| {
            ui.set_max_width(300.0);
            ui.label(RichText::new(crate::tr("Multicam: Wiedergabe starten und mit 1–9 den Blickwinkel wählen, Stopp übernimmt den Schnitt")).size(theme::FS_UI));
            ui.add_space(6.0);
            ui.label(RichText::new(crate::tr("Ton von (läuft durchgehend)")).small().color(theme::TEXT_FAINT()));
            let names: Vec<String> = (1..=n).map(|i| format!("{} {i}", crate::tr("Kamera"))).collect();
            let items: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            crate::widgets::segmented(ui, &items, &mut sel);
            ui.add_space(6.0);
            cancel = ui.add(theme::secondary("Abbrechen")).clicked();
        });
        self.multicam_audio = sel;
        if cancel {
            self.multicam = None;
        }
    }

    /// Dialog „Als Vorlage speichern“ / „Vorlage umbenennen“.
    pub fn template_dialog(&mut self, ctx: &egui::Context) {
        self.multicam_hint(ctx);
        if matches!(self.tpl_dialog, Some(TplDialog::Insert)) {
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
            TplDialog::Insert => return,
        };
        egui::Window::new(crate::tr(title)).id(egui::Id::new("tpl-dialog")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0)).show(ctx, |ui| {
            let name = match d {
                TplDialog::Save { name } | TplDialog::Rename { name, .. } => name,
                TplDialog::Insert => return,
            };
            let r = ui.add(egui::TextEdit::singleline(name).hint_text(crate::tr("z. B. Folge mit Intro und Endcard")).desired_width(280.0));
            r.request_focus();
            edited |= r.changed();
            theme::field_error(ui, err.as_deref());
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            ui.horizontal(|ui| {
                if ui.add(theme::primary("OK")).clicked() {
                    ok = true;
                }
                if ui.add(theme::secondary("Abbrechen")).clicked() {
                    close = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
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
                    // vorhandene Vorlage nicht ersetzen (nur Groß-/Kleinschreibung ändern geht)
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
    use cl_core::{Track, TrackKind};

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

    #[test]
    fn builtins_have_content_and_cam_audio_follows_main_angle() {
        assert!((0..3).all(|i| builtin(i).duration() > Time::ZERO));
        assert_eq!(cam_piece(0, 0, false), (false, 0.0, false)); // Ton läuft, Bild weg
        assert_eq!(cam_piece(1, 0, true), (false, 1.0, true)); // anderer Winkel: Bild, stumm
        assert_eq!(cam_piece(1, 0, false), (true, 1.0, true));
    }
}
