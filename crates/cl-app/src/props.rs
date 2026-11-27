//! Right-click → Properties: a window with the data of a medium, clip, track
//! or project (read-only, paths copyable).

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
        format!("{} GB", theme::dec(b as f64 / (1u64 << 30) as f64, 2))
    } else if b >= 1 << 20 {
        format!("{} MB", theme::dec(b as f64 / (1u64 << 20) as f64, 1))
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).ceil())
    }
}

pub fn file_size(p: &std::path::Path) -> String {
    match std::fs::metadata(p).map(|m| m.len()) {
        Ok(b) => fmt_bytes(b),
        Err(_) => crate::tr("File missing"),
    }
}

fn secs(t: Time) -> String {
    let s = t.as_secs_f64();
    if s >= 60.0 {
        format!("{}:{}", (s / 60.0) as u64, theme::local_num(format!("{:05.2}", s % 60.0)))
    } else {
        crate::theme::secs(s, 2)
    }
}

/// Title, rows and path of the properties window
pub type PropsRows = (String, Vec<(&'static str, String)>, Option<PathBuf>);

impl App {
    fn props_rows(&self, p: &Props) -> PropsRows {
        let kind_name = |k: MediaKind| match k {
            MediaKind::Video => "Video",
            MediaKind::Audio => "Audio",
            MediaKind::Image => "Image",
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
                    ("Type", crate::tr(kind_name(m.kind))),
                    ("File size", file_size(&m.path)),
                ];
                if m.kind != MediaKind::Image {
                    rows.push(("Length", secs(m.duration)));
                }
                if m.kind != MediaKind::Audio {
                    rows.push(("Resolution", format!("{} × {}", m.width, m.height)));
                }
                if let Some(f) = m.fps {
                    rows.push(("Frame rate", format!("{} fps", theme::dec(f.as_f64(), 3))));
                }
                if let Some(c) = &m.video_codec {
                    rows.push(("Video codec", c.clone()));
                }
                if let Some(c) = &m.audio_codec {
                    rows.push(("Audio codec", c.clone()));
                }
                rows.push(("Used", format!("{uses} ×")));
                let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                (name, rows, Some(m.path.clone()))
            }
            Props::Clip(id) => {
                let Some((t, c)) = self.project.find_clip(*id) else { return (String::new(), vec![], None) };
                let mut rows = vec![
                    ("Track", t.name.as_deref().map(crate::captions::shown_track_name).unwrap_or_else(|| crate::tr(format!("{:?}", t.kind)))),
                    ("Start", secs(c.start)),
                    ("End", secs(c.end())),
                    ("Duration", secs(c.duration)),
                ];
                let mut path = None;
                if let ClipContent::Media { media, .. } = &c.content {
                    rows.push(("Source in", secs(c.source_in)));
                    if let Some(m) = self.project.media(*media) {
                        path = Some(m.path.clone());
                    }
                }
                if (c.speed - 1.0).abs() > 1e-3 || c.fx.speed_curve.is_some() {
                    rows.push(("Speed", format!("{}×", theme::dec(c.speed as f64, 2))));
                }
                if c.fx.reverse {
                    rows.push(("Reverse", crate::tr("Yes")));
                }
                rows.push(("Volume", format!("{} dB", theme::local_num(format!("{:+.1}", c.volume_db)))));
                if c.fade_in > Time::ZERO || c.fade_out > Time::ZERO {
                    rows.push(("Fade in/out", format!("{} / {}", secs(c.fade_in), secs(c.fade_out))));
                }
                rows.push(("Opacity", format!("{:.0} %", c.opacity * 100.0)));
                if !c.fx.effects.is_empty() {
                    rows.push((
                        "Effects",
                        c.fx.effects.iter().map(|e| crate::tr(cl_fx::fx_name(&e.id))).collect::<Vec<_>>().join(", "),
                    ));
                }
                if let Some(f) = &c.fx.filter {
                    rows.push(("Filters", crate::tr(cl_fx::fx_name(&f.id))));
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
                for (on, n) in [(t.main, "main track"), (t.locked, "locked"), (t.hidden, "hidden"), (t.muted, "muted")] {
                    if on {
                        flags.push(crate::tr(n));
                    }
                }
                let rows = vec![
                    ("Type", crate::tr(format!("{:?}", t.kind))),
                    ("Clips", t.clips.len().to_string()),
                    ("End", secs(end)),
                    ("Status", if flags.is_empty() { "–".into() } else { flags.join(", ") }),
                ];
                (t.name.as_deref().map(crate::captions::shown_track_name).unwrap_or_else(|| crate::tr("Track")), rows, None)
            }
            Props::Project(path) => {
                let mut rows = vec![("File size", file_size(path))];
                if let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) {
                    rows.push(("Modified", crate::home::ago(t)));
                }
                let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(p) = std::fs::read_to_string(path).ok().and_then(|s| Project::from_json(&s).ok()) {
                    rows.push(("Length", secs(p.duration())));
                    rows.push(("Format", format!("{} × {}", p.settings.width, p.settings.height)));
                    rows.push(("Frame rate", format!("{} fps", theme::dec(p.settings.fps.as_f64(), 2))));
                    rows.push(("Media", p.media.len().to_string()));
                    let missing = p.media.iter().filter(|m| !m.path.is_file()).count();
                    if missing > 0 {
                        rows.push(("Missing files", missing.to_string()));
                    }
                }
                (name, rows, Some(path.clone()))
            }
        }
    }

    pub fn props_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.props.clone() else {
            self.props_cache = None;
            return;
        };
        // Rows read files (size, project file): only on open and after changes
        let (title, rows, path) = match &self.props_cache {
            Some((k, rev, r)) if *k == p && *rev == self.revision => r.clone(),
            _ => {
                let r = self.props_rows(&p);
                self.props_cache = Some((p.clone(), self.revision, r.clone()));
                r
            }
        };
        if rows.is_empty() {
            self.props = None;
            return;
        }
        let mut open = true;
        let head = format!("{}  Properties", icon::INFO);
        crate::dialog::window(ctx, crate::tr(head.as_str()), "props")
            .default_width(340.0)
            .show(ctx, |ui| {
                if crate::dialog::title(ui, &head) {
                    open = false;
                }
                ui.label(RichText::new(&title).strong().size(theme::FS_TEXT));
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
                    ui.label(RichText::new(crate::paths::shown(path)).small().color(theme::TEXT_DIM()));
                    ui.horizontal(|ui| {
                        if ui.add(theme::secondary(format!("{}  Copy path", icon::COPY))).clicked() {
                            ui.ctx().copy_text(path.display().to_string());
                            self.toast(format!("{} Copied", icon::CHECK));
                        }
                        if ui.add(theme::secondary(format!("{}  Show in Explorer", icon::FOLDER_OPEN))).clicked() {
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
