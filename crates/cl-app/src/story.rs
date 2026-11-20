//! Plugin „Story-Werkzeuge“ (built in, off by default): characters with name, color and
//! portrait; insert a dialog box and name tag for a character at the playhead.
//! Settings under Settings → Plugins and accounts, tools in the „Schnellaktionen“ menu.

use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

pub const BUILTIN: &str = "story";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub color: [u8; 3],
    #[serde(default)]
    pub portrait: Option<PathBuf>,
}

fn store() -> PathBuf {
    crate::paths::data_dir()
        .join("Integrations")
        .join("story.json")
}

pub fn load() -> Vec<Character> {
    std::fs::read_to_string(store())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(list: &[Character]) {
    let p = store();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = crate::paths::write_atomic(&p, serde_json::to_string_pretty(list).unwrap_or_default());
}

pub fn enabled(app: &App) -> bool {
    app.plugins.integration_enabled(BUILTIN)
}

impl App {
    /// Entries in the „Werkzeuge“ menu
    pub fn story_menu(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(crate::tr("Story")).small().color(theme::TEXT_DIM()));
        });
        let chars = self.characters.clone();
        if chars.is_empty() {
            if ui
                .add(crate::theme::menu_row(crate::tr(format!(
                    "{}   Figuren anlegen …",
                    icon::USER_PLUS
                ))))
                .clicked()
            {
                ui.close_menu();
                self.settings_section = crate::settings::Section::Integrations;
                self.settings_open = true;
            }
            return;
        }
        let mut act: Option<(bool, usize)> = None;
        // direct entries instead of submenus (stay open reliably)
        for (i, c) in chars.iter().enumerate() {
            if ui
                .add(
                    crate::theme::menu_row(crate::tr(format!(
                        "{}   Dialogbox: {}",
                        icon::CHAT_TEXT,
                        c.name
                    )))
                    .min_width(250.0),
                )
                .clicked()
            {
                act = Some((true, i));
                ui.close_menu();
            }
        }
        for (i, c) in chars.iter().enumerate() {
            if ui
                .add(
                    crate::theme::menu_row(crate::tr(format!(
                        "{}   Namensschild: {}",
                        icon::IDENTIFICATION_BADGE,
                        c.name
                    )))
                    .min_width(250.0),
                )
                .clicked()
            {
                act = Some((false, i));
                ui.close_menu();
            }
        }
        if let Some((dialog, i)) = act {
            let c = chars[i].clone();
            if dialog {
                self.insert_dialog(&c);
            } else {
                self.insert_name_tag(&c);
            }
        }
    }

    fn text_clip(&mut self, style: TextStyle, x: f32, y: f32, secs: f64) -> Option<ClipId> {
        let id = self.add_text_at(self.playhead, None, None)?;
        let mut c = self.project.find_clip(id)?.1.clone();
        c.content = ClipContent::Text(style);
        c.transform = Transform {
            x,
            y,
            scale: 1.0,
            rotation_deg: 0.0,
        };
        // ReplaceClip keeps position and length; the duration is only set by SetClipTiming
        self.apply(EditOp::ReplaceClip { clip: c.clone() });
        self.apply(EditOp::SetClipTiming {
            clip: id,
            start: c.start,
            duration: self.room_until_next(id, Time::from_secs_f64(secs)),
            source_in: Time::ZERO,
        });
        Some(id)
    }

    /// Desired duration, at most up to the next clip on the same track (at least one frame,
    /// as far as there is room): the core would reject an overlap.
    fn room_until_next(&self, id: ClipId, want: Time) -> Time {
        let Some((t, c)) = self.project.find_clip(id) else { return want };
        let frame = Time::from_secs_f64(1.0 / self.project.settings.fps.as_f64());
        match t.clips.iter().filter(|o| o.id != id && o.start >= c.start).map(|o| o.start).min() {
            Some(n) => want.min(n - c.start).max(frame.min(n - c.start)),
            None => want,
        }
    }

    /// Dialog box at the bottom: area in the character color, „Name: …“, portrait next to it on the left.
    pub fn insert_dialog(&mut self, ch: &Character) {
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let [r, g, b] = ch.color;
        let style = TextStyle {
            text: format!("{}: …", ch.name),
            size: (h * 0.045).max(24.0),
            color: [255, 255, 255, 255],
            bold: false,
            background: Some([
                (r as f32 * 0.35) as u8,
                (g as f32 * 0.35) as u8,
                (b as f32 * 0.35) as u8,
                230,
            ]),
            outline: Some((2.0, [r, g, b, 255])),
            shadow: false,
            align: -1,
            ..Default::default()
        };
        let secs = 4.0;
        self.text_clip(style, w * 0.06, h * 0.33, secs);
        if let Some(p) = ch.portrait.clone().filter(|p| p.is_file()) {
            if let Some(id) = self.add_own_sticker_at(&p, self.playhead, None) {
                if let Some(mut c) = self.project.find_clip(id).map(|(_, c)| c.clone()) {
                    c.transform = Transform {
                        x: -w * 0.36,
                        y: h * 0.30,
                        scale: 0.28,
                        rotation_deg: 0.0,
                    };
                    let start = c.start;
                    self.apply(EditOp::ReplaceClip { clip: c });
                    self.apply(EditOp::SetClipTiming {
                        clip: id,
                        start,
                        duration: self.room_until_next(id, Time::from_secs_f64(secs)),
                        source_in: Time::ZERO,
                    });
                }
            }
        }
        self.clip_tab = crate::app::ClipTab::Text;
        self.focus_text = true;
        self.toast(format!(
            "Dialogbox für {} eingefügt – Text im Inspektor eingeben",
            ch.name
        ));
    }

    /// Name tag (lower third) bottom left in the character color.
    pub fn insert_name_tag(&mut self, ch: &Character) {
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let [r, g, b] = ch.color;
        let style = TextStyle {
            text: ch.name.clone(),
            size: (h * 0.05).max(24.0),
            color: [255, 255, 255, 255],
            bold: true,
            background: Some([r, g, b, 235]),
            shadow: true,
            align: -1,
            ..Default::default()
        };
        if let Some(id) = self.text_clip(style, -w * 0.30, h * 0.30, 3.0) {
            if let Some(mut c) = self.project.find_clip(id).map(|(_, c)| c.clone()) {
                c.anim.intro = Some(AnimRef {
                    id: "slide-left".into(),
                    duration: Time::from_secs_f64(0.4),
                });
                self.apply(EditOp::ReplaceClip { clip: c });
            }
        }
        self.toast(format!("Namensschild für {} eingefügt", ch.name));
    }

    /// Section in Settings → Plugins and accounts
    pub fn story_settings(&mut self, ui: &mut egui::Ui) {
        if !enabled(self) {
            return;
        }
        theme::section(ui, "Story-Werkzeuge");
        let mut changed = false;
        let mut remove = None;
        for (i, c) in self.characters.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let mut rgba = [c.color[0], c.color[1], c.color[2], 255];
                if widgets::color_button(ui, &mut rgba) {
                    c.color = [rgba[0], rgba[1], rgba[2]];
                    changed = true;
                }
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut c.name).desired_width(140.0))
                    .changed();
                let label = c
                    .portrait
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| crate::tr("Porträt …"));
                if ui
                    .add(theme::ghost(format!("{}  {label}", icon::IMAGE)))
                    .clicked()
                {
                    if let Some(f) = crate::dialog()
                        .add_filter(crate::tr("Bilder"), crate::library::STICKER_EXT)
                        .pick_file()
                    {
                        c.portrait = Some(f);
                        changed = true;
                    }
                }
                if theme::icon_button(ui, icon::TRASH, "Figur entfernen", true, false).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.characters.remove(i);
            changed = true;
        }
        if ui
            .add(theme::secondary(format!(
                "{}  Figur hinzufügen",
                icon::USER_PLUS
            )))
            .clicked()
        {
            let palette = [
                [240, 80, 80],
                [80, 160, 255],
                [90, 200, 120],
                [240, 190, 60],
                [170, 110, 240],
            ];
            let n = self.characters.len();
            self.characters.push(Character {
                name: format!("{} {}", crate::tr("Figur"), n + 1),
                color: palette[n % palette.len()],
                portrait: None,
            });
            changed = true;
        }
        if changed {
            save(&self.characters);
        }
    }
}

#[cfg(test)]
mod room_tests {
    use super::*;

    /// Bug: if a clip was on the text track behind it, the core rejected the 4 s (error message,
    /// the dialog box stayed at the default duration). Now up to the neighbor.
    #[test]
    fn dialog_text_stops_at_the_next_clip() {
        let (mut app, _home, _g) = crate::saving::tests::test_app();
        let t = Time::from_secs_f64;
        let mut p = Project::new("S");
        let mut tr = Track::new(TrackKind::Text);
        tr.clips.push(Clip::new(ClipContent::Text(TextStyle::default()), t(1.0), t(2.0)));
        p.timeline.tracks.push(tr);
        app.set_doc(p);
        app.settings.text_duration = 0.5;
        let id = app.text_clip(TextStyle::default(), 0.0, 0.0, 4.0).expect("inserted");
        app.sync_project();
        let (tr, c) = app.project.find_clip(id).unwrap();
        assert_eq!(tr.clips.len(), 2, "same track");
        assert_eq!((c.start, c.duration), (t(0.0), t(1.0)));
        // without a neighbor: full duration
        app.playhead = t(10.0);
        let id = app.text_clip(TextStyle::default(), 0.0, 0.0, 4.0).unwrap();
        app.sync_project();
        assert_eq!(app.project.find_clip(id).unwrap().1.duration, t(4.0));
    }
}
