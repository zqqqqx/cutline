//! Presets: den Look eines Clips speichern (Anpassen, Filter, Effekte, LUT, Animationen,
//! Deckkraft, Ton-Bearbeitung, bei Text der Textstil ohne Inhalt) und mit einem Klick auf
//! einen oder mehrere Clips anwenden. Ablage: `<Daten>/Library/presets.json`.

use cl_core::{Animations, AudioFx, Clip, ClipContent, ClipFx, TextStyle};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub fx: ClipFx,
    #[serde(default)]
    pub anim: Animations,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub audio: AudioFx,
    /// Textstil (ohne Inhalt), nur bei Text-Presets
    #[serde(default)]
    pub text: Option<TextStyle>,
}

fn one() -> f32 {
    1.0
}

impl Preset {
    /// Look eines Clips übernehmen. Clip-gebundene Dinge (Maske aus „Hintergrund entfernen“,
    /// Geschwindigkeitskurve, Rückwärts) bleiben außen vor.
    pub fn from_clip(name: &str, c: &Clip) -> Self {
        let mut fx = c.fx.clone();
        fx.cutout = None;
        fx.person = None;
        fx.speed_curve = None;
        fx.reverse = false;
        let text = match &c.content {
            ClipContent::Text(t) => Some(TextStyle {
                text: String::new(),
                ..t.clone()
            }),
            _ => None,
        };
        Self {
            name: name.trim().to_string(),
            fx,
            anim: c.anim.clone(),
            opacity: c.opacity,
            audio: c.audio.clone(),
            text,
        }
    }

    /// Auf einen Clip anwenden (Inhalt, Position, Zeit und Maske bleiben).
    pub fn apply(&self, c: &mut Clip) {
        let keep = (
            c.fx.cutout.clone(),
            c.fx.person.clone(),
            c.fx.speed_curve.clone(),
            c.fx.reverse,
        );
        c.fx = self.fx.clone();
        (c.fx.cutout, c.fx.person, c.fx.speed_curve, c.fx.reverse) = keep;
        c.anim = self.anim.clone();
        c.opacity = self.opacity;
        c.audio = self.audio.clone();
        if let (ClipContent::Text(t), Some(s)) = (&mut c.content, &self.text) {
            let text = std::mem::take(&mut t.text);
            *t = TextStyle { text, ..s.clone() };
        }
    }

    pub fn is_text(&self) -> bool {
        self.text.is_some()
    }
}

fn path() -> std::path::PathBuf {
    crate::paths::data_dir()
        .join("Library")
        .join("presets.json")
}

pub fn load() -> Vec<Preset> {
    crate::paths::read_json_or_quarantine(&path())
}

pub fn save(list: &[Preset]) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = crate::paths::write_atomic(&p, serde_json::to_string_pretty(list).unwrap_or_default());
}

/// Offener Preset-Dialog
#[derive(Clone)]
pub enum PresetDialog {
    Save { name: String },
    Rename { index: usize, name: String },
}

impl App {
    /// Preset-Leiste über den Inspektor-Tabs: speichern und anwenden.
    pub fn preset_bar(&mut self, ui: &mut egui::Ui, c: &Clip) {
        let is_text = matches!(c.content, ClipContent::Text(_));
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::tr("Look"))
                    .small()
                    .color(theme::TEXT_DIM()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(
                    ui,
                    icon::FLOPPY_DISK,
                    "Look als Preset speichern",
                    true,
                    false,
                )
                .clicked()
                {
                    self.preset_dialog = Some(PresetDialog::Save {
                        name: String::new(),
                    });
                }
                let matching: Vec<(usize, String)> = self
                    .presets
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.is_text() == is_text)
                    .map(|(i, p)| (i, p.name.clone()))
                    .collect();
                let label = if matching.is_empty() {
                    crate::tr("Keine Presets")
                } else {
                    crate::tr(format!("Looks ({})", matching.len()))
                };
                // Aufklappbare Liste statt Popup (ein offenes Popup blieb beim Clipwechsel stehen)
                let caret = if self.preset_list_open {
                    icon::CARET_UP
                } else {
                    icon::CARET_DOWN
                };
                if ui
                    .add_enabled(
                        !matching.is_empty(),
                        theme::ghost(format!("{}  {label} {caret}", icon::SWATCHES)),
                    )
                    .clicked()
                {
                    self.preset_list_open = !self.preset_list_open;
                }
                if matching.is_empty() {
                    self.preset_list_open = false;
                }
            });
        });
        let mut apply = None;
        let mut rename = None;
        let mut remove = None;
        if self.preset_list_open {
            let matching: Vec<(usize, String)> = self
                .presets
                .iter()
                .enumerate()
                .filter(|(_, p)| p.is_text() == is_text)
                .map(|(i, p)| (i, p.name.clone()))
                .collect();
            egui::Frame::default()
                .fill(theme::BG_RAISED())
                .corner_radius(theme::R_CARD)
                .inner_margin(theme::PAD_CARD)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (i, n) in &matching {
                        ui.horizontal(|ui| {
                            if ui
                                .add(crate::theme::menu_row(n.as_str()).min_width(130.0))
                                .on_hover_text(crate::tr("Auf die Auswahl anwenden"))
                                .clicked()
                            {
                                apply = Some(*i);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if theme::icon_button(
                                        ui,
                                        icon::TRASH,
                                        "Preset entfernen",
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        remove = Some(*i);
                                    }
                                    if theme::icon_button(
                                        ui,
                                        icon::PENCIL_SIMPLE,
                                        "Umbenennen",
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        rename = Some(*i);
                                    }
                                },
                            );
                        });
                    }
                });
        }
        {
            {
                if let Some(i) = apply {
                    self.apply_preset(i);
                    self.preset_list_open = false;
                }
                if let Some(i) = rename {
                    self.preset_dialog = Some(PresetDialog::Rename {
                        index: i,
                        name: self.presets[i].name.clone(),
                    });
                }
                if let Some(i) = remove {
                    let p = self.presets.remove(i);
                    save(&self.presets);
                    self.toast(format!("Preset „{}“ entfernt", p.name));
                }
            }
        }
    }

    /// Preset `i` auf die Auswahl anwenden (Inspektor-Leiste und Clip-Menü „Look anwenden“).
    pub fn apply_preset(&mut self, i: usize) {
        let Some(p) = self.presets.get(i).cloned() else {
            return;
        };
        self.modify_selection(|x| {
            // Text-Presets nur auf Text, Look-Presets nur auf Medien
            if matches!(x.content, ClipContent::Text(_)) == p.is_text() {
                p.apply(x);
            }
        });
        self.toast(format!("Preset „{}“ angewendet", p.name));
    }

    /// Dialog „Preset speichern/umbenennen“ (jeden Frame aufrufen).
    pub fn preset_dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.preset_dialog else {
            self.preset_dialog_since = 0;
            return;
        };
        let pass = ctx.cumulative_pass_nr();
        if self.preset_dialog_since == 0 {
            self.preset_dialog_since = pass;
        }
        let fresh = pass <= self.preset_dialog_since + 1;
        let (mut ok, mut close) = (false, false);
        let err = self.dialog_error.clone();
        let mut edited = false;
        let title = match d {
            PresetDialog::Save { .. } => "Look als Preset speichern",
            PresetDialog::Rename { .. } => "Preset umbenennen",
        };
        let modal = egui::Modal::new(egui::Id::new("preset-dialog")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(RichText::new(crate::tr(title)).strong().size(theme::FS_TITLE));
            ui.add_space(8.0);
            let name = match d {
                PresetDialog::Save { name } | PresetDialog::Rename { name, .. } => name,
            };
            let r = ui.add(egui::TextEdit::singleline(name).hint_text(crate::tr("Name, z. B. Gaming-Look")).desired_width(f32::INFINITY));
            if fresh {
                r.request_focus();
            }
            edited |= r.changed();
            theme::field_error(ui, err.as_deref());
            if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::primary("OK")).clicked() {
                    ok = true;
                }
                if ui.add(theme::secondary("Abbrechen")).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() && !fresh {
            close = true;
        }
        if edited {
            self.dialog_error = None;
        }
        let empty = matches!(&self.preset_dialog, Some(PresetDialog::Save { name } | PresetDialog::Rename { name, .. }) if name.trim().is_empty());
        if ok && empty {
            self.dialog_error = Some("Bitte einen Namen eingeben".into());
        } else if ok {
            self.dialog_error = None;
            match self.preset_dialog.take() {
                Some(PresetDialog::Save { name }) if !name.trim().is_empty() => {
                    if let Some(c) = self.selected.and_then(|s| self.clip_view(s)) {
                        let p = Preset::from_clip(&name, &c);
                        // gleicher Name ersetzt das alte Preset
                        self.presets.retain(|x| x.name != p.name);
                        self.presets.push(p);
                        save(&self.presets);
                        self.toast(format!("Preset „{}“ gespeichert", name.trim()));
                    }
                }
                Some(PresetDialog::Rename { index, name }) if !name.trim().is_empty() => {
                    if let Some(p) = self.presets.get_mut(index) {
                        p.name = name.trim().to_string();
                        save(&self.presets);
                    }
                }
                _ => {}
            }
        } else if close {
            self.preset_dialog = None;
            self.dialog_error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{FxRef, MediaId, MediaKind, Time};

    #[test]
    fn preset_roundtrip_keeps_clip_specific_parts() {
        let mut a = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Video,
            },
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        a.fx.filter = Some(FxRef {
            id: "vivid".into(),
            intensity: 0.7,
            params: Default::default(),
        });
        a.fx.adjust.brightness = 0.2;
        a.fx.cutout = Some("matte.mkv".into());
        a.opacity = 0.8;
        let p = Preset::from_clip("Look", &a);
        assert!(p.fx.cutout.is_none());
        let mut b = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Video,
            },
            Time::ZERO,
            Time::from_secs_f64(1.0),
        );
        b.fx.cutout = Some("eigene.mkv".into());
        p.apply(&mut b);
        assert_eq!(b.fx.filter, a.fx.filter);
        assert_eq!(b.fx.adjust.brightness, 0.2);
        assert_eq!(b.opacity, 0.8);
        assert_eq!(
            b.fx.cutout.as_deref(),
            Some("eigene.mkv"),
            "Maske des Ziel-Clips bleibt"
        );
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Preset>(&json).unwrap(), p);
    }
}
