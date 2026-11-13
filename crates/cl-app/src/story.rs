//! Plugin „Story-Werkzeuge“ (eingebaut, standardmäßig aus): Figuren mit Name, Farbe und
//! Porträt; Dialogbox und Namensschild für eine Figur am Playhead einfügen.
//! Einstellungen unter Einstellungen → Integrationen, Werkzeuge im Menü „Werkzeuge“.

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
    let _ = std::fs::write(p, serde_json::to_string_pretty(list).unwrap_or_default());
}

pub fn enabled(app: &App) -> bool {
    app.plugins.integration_enabled(BUILTIN)
}

impl App {
    /// Einträge im Menü „Werkzeuge“
    pub fn story_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(crate::tr("Story"))
                .small()
                .color(theme::TEXT_FAINT),
        );
        let chars = self.characters.clone();
        if chars.is_empty() {
            if ui
                .add(
                    egui::Button::new(crate::tr(format!(
                        "{}   Figuren anlegen …",
                        icon::USER_PLUS
                    )))
                    .frame(false),
                )
                .clicked()
            {
                ui.close_menu();
                self.settings_section = crate::settings::Section::Integrations;
                self.settings_open = true;
            }
            return;
        }
        let mut act: Option<(bool, usize)> = None;
        // direkte Einträge statt Untermenüs (bleiben zuverlässig offen)
        for (i, c) in chars.iter().enumerate() {
            if ui
                .add(
                    egui::Button::new(crate::tr(format!(
                        "{}   Dialogbox: {}",
                        icon::CHAT_TEXT,
                        c.name
                    )))
                    .frame(false)
                    .min_size(egui::vec2(250.0, 22.0)),
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
                    egui::Button::new(crate::tr(format!(
                        "{}   Namensschild: {}",
                        icon::IDENTIFICATION_BADGE,
                        c.name
                    )))
                    .frame(false)
                    .min_size(egui::vec2(250.0, 22.0)),
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
        c.duration = Time::from_secs_f64(secs);
        self.apply(EditOp::ReplaceClip { clip: c.clone() });
        self.apply(EditOp::SetClipTiming {
            clip: id,
            start: c.start,
            duration: Time::from_secs_f64(secs),
            source_in: Time::ZERO,
        });
        Some(id)
    }

    /// Dialogbox unten: Fläche in Figurfarbe, „Name: …“, Porträt links daneben.
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
                        duration: Time::from_secs_f64(secs),
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

    /// Namensschild (Bauchbinde) unten links in Figurfarbe.
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

    /// Abschnitt in Einstellungen → Integrationen
    pub fn story_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Story-Werkzeuge");
        let mut on = enabled(self);
        if widgets::toggle_row(ui, "Plugin aktiv", &mut on) {
            let id = self
                .plugins
                .plugins
                .iter()
                .find(|p| p.manifest.plugin.entry.as_deref() == Some("builtin:story"))
                .map(|p| p.manifest.plugin.id.clone());
            if let Some(id) = id {
                self.set_plugin_enabled(&id, on);
            }
        }
        if !on {
            ui.label(RichText::new(crate::tr("Figuren mit Name, Farbe und Porträt; Dialogboxen und Namensschilder mit einem Klick (Menü „Werkzeuge“).")).small().color(theme::TEXT_DIM));
            return;
        }
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
