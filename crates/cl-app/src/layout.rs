//! Layout-Profile: Anordnung der großen Bereiche (Medien, Inspektor, Zeitleiste) und ihre
//! Größen. „Layout bearbeiten“ zeigt über jedem Bereich Knöpfe zum Umsetzen; Profile lassen
//! sich speichern und wechseln.

use eframe::egui::{self, vec2, Color32, Rect, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutProfile {
    pub name: String,
    /// Medien rechts, Inspektor links
    pub assets_right: bool,
    /// Zeitleiste oben statt unten
    pub timeline_top: bool,
    /// Zeitleiste über die ganze Breite (sonst nur unter dem Player)
    pub timeline_full: bool,
    /// Anteile: Medienbreite, Inspektorbreite, Zeitleistenhöhe
    pub frac: (f32, f32, f32),
}

impl Default for LayoutProfile {
    fn default() -> Self {
        Self {
            name: "Standard".into(),
            assets_right: false,
            timeline_top: false,
            timeline_full: true,
            frac: (0.27, 0.22, 0.38),
        }
    }
}

/// Mitgelieferte Profile
pub fn builtin() -> Vec<LayoutProfile> {
    let d = LayoutProfile::default();
    vec![
        d.clone(),
        LayoutProfile {
            name: "Großer Player".into(),
            frac: (0.2, 0.18, 0.3),
            ..d.clone()
        },
        LayoutProfile {
            name: "Medien rechts".into(),
            assets_right: true,
            ..d.clone()
        },
        LayoutProfile {
            name: "Lange Zeitleiste".into(),
            timeline_full: false,
            frac: (0.24, 0.2, 0.45),
            ..d
        },
    ]
}

impl App {
    /// Profil übernehmen (Größen werden im nächsten Bild gesetzt)
    pub fn apply_layout(&mut self, p: LayoutProfile) {
        self.layout_frac = p.frac;
        self.settings.layout = p;
        // erzwingt das Setzen der Panelgrößen aus den Anteilen
        self.layout_screen = Some(Rect::NOTHING);
        self.settings.save();
    }

    /// Bearbeitungsmodus: Knöpfe über den Bereichen und eine Leiste zum Speichern
    pub fn layout_editor(&mut self, ctx: &egui::Context) {
        if !self.layout_edit {
            return;
        }
        let rects: Vec<(&str, Rect)> = ["assets", "inspector", "timeline", "player"]
            .iter()
            .filter_map(|k| self.tour_rects.get(k).map(|r| (*k, *r)))
            .collect();
        let mut l = self.settings.layout.clone();
        for (k, r) in rects {
            let title = match k {
                "assets" => "Medien",
                "inspector" => "Inspektor",
                "timeline" => "Zeitleiste",
                _ => "Player",
            };
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new(("lay-dim", k))));
            painter.rect_filled(r.shrink(3.0), theme::R_CARD, Color32::from_black_alpha(120));
            painter.rect_stroke(r.shrink(3.0), theme::R_CARD, egui::Stroke::new(1.5_f32, theme::ACCENT()), egui::StrokeKind::Inside);
            egui::Area::new(egui::Id::new(("lay-card", k)))
                .order(egui::Order::Foreground)
                .fixed_pos(r.center() - vec2(90.0, 30.0))
                .show(ctx, |ui| {
                    theme::card(theme::PAD_CARD_L).show(ui, |ui| {
                        ui.set_width(160.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(crate::tr(title)).strong());
                            ui.horizontal(|ui| match k {
                                "assets" | "inspector" => {
                                    let left = (k == "assets") != l.assets_right;
                                    let (g, tip) = if left {
                                        (icon::ARROW_RIGHT, "Auf die andere Seite")
                                    } else {
                                        (icon::ARROW_LEFT, "Auf die andere Seite")
                                    };
                                    if theme::icon_button(ui, g, tip, true, false).clicked() {
                                        l.assets_right = !l.assets_right;
                                    }
                                }
                                "timeline" => {
                                    let g = if l.timeline_top { icon::ARROW_DOWN } else { icon::ARROW_UP };
                                    if theme::icon_button(ui, g, if l.timeline_top { "Nach unten" } else { "Nach oben" }, true, false).clicked() {
                                        l.timeline_top = !l.timeline_top;
                                    }
                                    if theme::icon_button(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, "Volle Breite", true, l.timeline_full).clicked() {
                                        l.timeline_full = !l.timeline_full;
                                    }
                                }
                                _ => {
                                    ui.label(RichText::new(crate::tr("Ränder ziehen zum Ändern der Größe")).small().color(theme::TEXT_DIM()));
                                }
                            });
                        });
                    });
                });
        }
        if l != self.settings.layout {
            l.frac = self.layout_frac;
            self.settings.layout = l;
            self.layout_screen = Some(Rect::NOTHING);
        }
        // Leiste oben: Profile wählen, speichern, fertig
        egui::Area::new(egui::Id::new("lay-bar"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -16.0))
            .show(ctx, |ui| {
                theme::card(theme::PAD_CARD).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}  {}", icon::LAYOUT, crate::tr("Layout"))).strong());
                        let mut all = builtin();
                        all.extend(self.settings.layouts.clone());
                        egui::ComboBox::from_id_salt("lay-pick")
                            .selected_text(crate::tr(&self.settings.layout.name))
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for p in all {
                                    if ui.selectable_label(p.name == self.settings.layout.name, crate::tr(&p.name)).clicked() {
                                        self.apply_layout(p);
                                    }
                                }
                            });
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.layout_name)
                                .hint_text(crate::tr("Name des Profils"))
                                .desired_width(130.0),
                        );
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.add(theme::secondary(format!("{}  Speichern", icon::FLOPPY_DISK))).clicked() || enter {
                            let name = self.layout_name.trim().to_string();
                            if name.is_empty() {
                                self.dialog_error = Some("Bitte einen Namen eingeben".into());
                            } else if builtin().iter().any(|b| b.name == name) {
                                self.dialog_error = Some("Dieser Name gehört zu einem mitgelieferten Profil".into());
                            } else {
                                let mut p = self.settings.layout.clone();
                                p.name = name.clone();
                                p.frac = self.layout_frac;
                                self.settings.layouts.retain(|x| x.name != name);
                                self.settings.layouts.push(p.clone());
                                self.settings.layout = p;
                                self.settings.save();
                                self.dialog_error = None;
                                self.toast(format!("{} {}", icon::CHECK, crate::tr("Layout gespeichert")));
                            }
                        }
                        let own = self.settings.layouts.iter().any(|p| p.name == self.settings.layout.name);
                        if own && theme::icon_button(ui, icon::TRASH, "Profil löschen", true, false).clicked() {
                            let n = self.settings.layout.name.clone();
                            self.settings.layouts.retain(|p| p.name != n);
                            self.apply_layout(LayoutProfile::default());
                        }
                        if ui.add(theme::primary(format!("{}  Fertig", icon::CHECK))).clicked() {
                            self.layout_edit = false;
                            self.dialog_error = None;
                            self.settings.layout.frac = self.layout_frac;
                            self.settings.save();
                        }
                    });
                    theme::field_error(ui, self.dialog_error.as_deref());
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_are_distinct_and_roundtrip() {
        let b = builtin();
        let mut names: Vec<&str> = b.iter().map(|p| p.name.as_str()).collect();
        names.dedup();
        assert_eq!(names.len(), b.len());
        for p in &b {
            let s = serde_json::to_string(p).unwrap();
            assert_eq!(&serde_json::from_str::<LayoutProfile>(&s).unwrap(), p);
            assert!(p.frac.0 > 0.1 && p.frac.1 > 0.1 && p.frac.2 > 0.2);
        }
        // ältere Einstellungen ohne Feld → Standard
        assert_eq!(serde_json::from_str::<LayoutProfile>("{}").unwrap(), LayoutProfile::default());
    }
}
