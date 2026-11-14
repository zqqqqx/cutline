//! Rechtsklick → Eigenschaften: ein Fenster mit den Daten eines Mediums, Clips, einer Spur
//! oder eines Projekts (nur lesen, Pfade kopierbar).

use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::theme::{self, icon};

#[derive(Clone, PartialEq)]
pub enum Props {
    Media(MediaId),
    Clip(ClipId),
    Track(TrackId),
    Project(PathBuf),
}

pub fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.2} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).ceil())
    }
}

pub fn file_size(p: &std::path::Path) -> String {
    match std::fs::metadata(p).map(|m| m.len()) {
        Ok(b) => fmt_bytes(b),
        Err(_) => crate::tr("Datei fehlt"),
    }
}

fn secs(t: Time) -> String {
    let s = t.as_secs_f64();
    if s >= 60.0 {
        format!("{}:{:05.2}", (s / 60.0) as u64, s % 60.0)
    } else {
        format!("{s:.2} s")
    }
}

impl App {
    fn props_rows(&self, p: &Props) -> (String, Vec<(&'static str, String)>, Option<PathBuf>) {
        let kind_name = |k: MediaKind| match k {
            MediaKind::Video => "Video",
            MediaKind::Audio => "Audio",
            MediaKind::Image => "Bild",
        };
        match p {
            Props::Media(id) => {
                let Some(m) = self.project.media(*id) else { return (String::new(), vec![], None) };
                let uses = self
                    .project
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| matches!(&c.content, ClipContent::Media { media, .. } if media == id))
                    .count();
                let mut rows = vec![
                    ("Art", crate::tr(kind_name(m.kind))),
                    ("Dateigröße", file_size(&m.path)),
                ];
                if m.kind != MediaKind::Image {
                    rows.push(("Länge", secs(m.duration)));
                }
                if m.kind != MediaKind::Audio {
                    rows.push(("Auflösung", format!("{} × {}", m.width, m.height)));
                }
                if let Some(f) = m.fps {
                    rows.push(("Bildrate", format!("{:.3} fps", f.as_f64())));
                }
                if let Some(c) = &m.video_codec {
                    rows.push(("Video-Codec", c.clone()));
                }
                if let Some(c) = &m.audio_codec {
                    rows.push(("Audio-Codec", c.clone()));
                }
                rows.push(("Verwendet", format!("{uses} ×")));
                let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                (name, rows, Some(m.path.clone()))
            }
            Props::Clip(id) => {
                let Some((t, c)) = self.project.find_clip(*id) else { return (String::new(), vec![], None) };
                let mut rows = vec![
                    ("Spur", t.name.clone().unwrap_or_else(|| crate::tr(format!("{:?}", t.kind)))),
                    ("Start", secs(c.start)),
                    ("Ende", secs(c.end())),
                    ("Dauer", secs(c.duration)),
                ];
                let mut path = None;
                if let ClipContent::Media { media, .. } = &c.content {
                    rows.push(("Quellbeginn", secs(c.source_in)));
                    if let Some(m) = self.project.media(*media) {
                        path = Some(m.path.clone());
                    }
                }
                if (c.speed - 1.0).abs() > 1e-3 || c.fx.speed_curve.is_some() {
                    rows.push(("Tempo", format!("{:.2}×", c.speed)));
                }
                if c.fx.reverse {
                    rows.push(("Rückwärts", crate::tr("Ja")));
                }
                rows.push(("Lautstärke", format!("{:+.1} dB", c.volume_db)));
                if c.fade_in > Time::ZERO || c.fade_out > Time::ZERO {
                    rows.push(("Ein-/Ausblenden", format!("{} / {}", secs(c.fade_in), secs(c.fade_out))));
                }
                rows.push(("Deckkraft", format!("{:.0} %", c.opacity * 100.0)));
                if !c.fx.effects.is_empty() {
                    rows.push((
                        "Effekte",
                        c.fx.effects.iter().map(|e| crate::tr(cl_fx::fx_name(&e.id))).collect::<Vec<_>>().join(", "),
                    ));
                }
                if let Some(f) = &c.fx.filter {
                    rows.push(("Filter", crate::tr(cl_fx::fx_name(&f.id))));
                }
                if !c.keyframes.is_empty() {
                    rows.push(("Keyframes", c.keyframes.times().len().to_string()));
                }
                (c.label(&self.project), rows, path)
            }
            Props::Track(id) => {
                let Some(t) = self.project.timeline.tracks.iter().find(|t| t.id == *id) else {
                    return (String::new(), vec![], None);
                };
                let end = t.clips.iter().map(|c| c.end()).max().unwrap_or(Time::ZERO);
                let mut flags = Vec::new();
                for (on, n) in [(t.main, "Hauptspur"), (t.locked, "gesperrt"), (t.hidden, "ausgeblendet"), (t.muted, "stumm")] {
                    if on {
                        flags.push(crate::tr(n));
                    }
                }
                let rows = vec![
                    ("Art", crate::tr(format!("{:?}", t.kind))),
                    ("Clips", t.clips.len().to_string()),
                    ("Ende", secs(end)),
                    ("Status", if flags.is_empty() { "–".into() } else { flags.join(", ") }),
                ];
                (t.name.clone().unwrap_or_else(|| crate::tr("Spur")), rows, None)
            }
            Props::Project(path) => {
                let mut rows = vec![("Dateigröße", file_size(path))];
                if let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) {
                    rows.push(("Geändert", crate::home::ago(t)));
                }
                let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(p) = std::fs::read_to_string(path).ok().and_then(|s| Project::from_json(&s).ok()) {
                    rows.push(("Länge", secs(p.duration())));
                    rows.push(("Format", format!("{} × {}", p.settings.width, p.settings.height)));
                    rows.push(("Bildrate", format!("{:.2} fps", p.settings.fps.as_f64())));
                    rows.push(("Medien", p.media.len().to_string()));
                    let missing = p.media.iter().filter(|m| !m.path.is_file()).count();
                    if missing > 0 {
                        rows.push(("Fehlende Dateien", missing.to_string()));
                    }
                }
                (name, rows, Some(path.clone()))
            }
        }
    }

    pub fn props_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.props.clone() else { return };
        let (title, rows, path) = self.props_rows(&p);
        if rows.is_empty() {
            self.props = None;
            return;
        }
        let mut open = true;
        egui::Window::new(crate::tr(format!("{}  Eigenschaften", icon::INFO)))
            .id(egui::Id::new("props"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(&title).strong().size(13.0));
                ui.add_space(6.0);
                egui::Grid::new("props-grid").num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
                    for (k, v) in &rows {
                        ui.label(RichText::new(crate::tr(*k)).color(theme::TEXT_DIM()));
                        ui.label(v);
                        ui.end_row();
                    }
                });
                if let Some(path) = &path {
                    ui.add_space(8.0);
                    ui.label(RichText::new(path.display().to_string()).small().color(theme::TEXT_FAINT()));
                    ui.horizontal(|ui| {
                        if ui.add(theme::secondary(format!("{}  Pfad kopieren", icon::COPY))).clicked() {
                            ui.ctx().copy_text(path.display().to_string());
                            self.toast(format!("{} Kopiert", icon::CHECK));
                        }
                        if ui.add(theme::secondary(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                            crate::home::reveal(path);
                        }
                    });
                }
            });
        if !open {
            self.props = None;
        }
    }
}
