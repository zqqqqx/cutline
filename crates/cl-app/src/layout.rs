//! Layout profiles: arrangement of the large panels (media, inspector, timeline) and their
//! sizes. „Layout bearbeiten“ shows buttons above each panel to move it; profiles can
//! be saved and switched.

use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutProfile {
    pub name: String,
    /// Media on the right, inspector on the left
    pub assets_right: bool,
    /// Timeline at the top instead of the bottom
    pub timeline_top: bool,
    /// Timeline across the full width (otherwise only below the player)
    pub timeline_full: bool,
    /// Toolbar of the media menu at the top (horizontal) instead of at the side on the outer edge
    pub rail_top: bool,
    /// Shares: media width, inspector width, timeline height
    pub frac: (f32, f32, f32),
}

impl Default for LayoutProfile {
    fn default() -> Self {
        Self {
            name: "Standard".into(),
            assets_right: false,
            timeline_top: false,
            timeline_full: true,
            rail_top: false,
            frac: (0.27, 0.22, 0.38),
        }
    }
}

/// Bundled profiles
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

/// Arrangement tiles in the settings: (label, explanation, media on the right, timeline at the top)
pub const ARRANGEMENTS: [(&str, &str, bool, bool); 4] = [
    ("Standard", "Medien links, Inspektor rechts, Zeitleiste unten", false, false),
    ("Getauscht", "Medien rechts, Inspektor links, Zeitleiste unten", true, false),
    ("Oben", "Medien links, Inspektor rechts, Zeitleiste oben", false, true),
    ("Beides", "Medien rechts, Inspektor links, Zeitleiste oben", true, true),
];

/// Mini sketch of an arrangement in `r`: rects for media, player, inspector, timeline
/// and the toolbar strip (lies within the media rect).
pub fn sketch(r: Rect, l: &LayoutProfile) -> ([Rect; 4], Rect) {
    let (g, inner) = (2.0, r.shrink(3.0));
    let (aw, iw) = (inner.width() * 0.27, inner.width() * 0.22);
    let th = inner.height() * 0.4;
    // Row or column that the timeline shares with the rest
    let split = |b: Rect| {
        if l.timeline_top {
            (Rect::from_min_max(b.min, pos2(b.right(), b.top() + th)), Rect::from_min_max(pos2(b.left(), b.top() + th + g), b.max))
        } else {
            (Rect::from_min_max(pos2(b.left(), b.bottom() - th), b.max), Rect::from_min_max(b.min, pos2(b.right(), b.bottom() - th - g)))
        }
    };
    let (tl, rest) = if l.timeline_full { split(inner) } else { (Rect::NOTHING, inner) };
    let (lw, rw) = if l.assets_right { (iw, aw) } else { (aw, iw) };
    let left = Rect::from_min_max(rest.min, pos2(rest.left() + lw, rest.bottom()));
    let right = Rect::from_min_max(pos2(rest.right() - rw, rest.top()), rest.max);
    let mid = Rect::from_min_max(pos2(left.right() + g, rest.top()), pos2(right.left() - g, rest.bottom()));
    let (tl, player) = if l.timeline_full { (tl, mid) } else { split(mid) };
    let (assets, insp) = if l.assets_right { (right, left) } else { (left, right) };
    let rail = if l.rail_top {
        Rect::from_min_max(assets.min, pos2(assets.right(), assets.top() + 3.0))
    } else if l.assets_right {
        Rect::from_min_max(pos2(assets.right() - 3.0, assets.top()), assets.max)
    } else {
        Rect::from_min_max(assets.min, pos2(assets.left() + 3.0, assets.bottom()))
    };
    ([assets, player, insp, tl], rail)
}

impl App {
    /// Settings → General: arrangement of the large panels as tiles with a mini sketch,
    /// plus full timeline width and toolbar position. Sizes and profile name stay.
    pub fn arrangement_settings(&mut self, ui: &mut egui::Ui) {
        let cur = self.settings.layout.clone();
        let mut pick: Option<LayoutProfile> = None;
        theme::row(ui, "Anordnung", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (name, tip, ar, tt) in ARRANGEMENTS {
                    let on = cur.assets_right == ar && cur.timeline_top == tt;
                    // same structure and same sizes as the design tiles
                    let (r, resp) = ui.allocate_exact_size(vec2(66.0, 50.0), egui::Sense::click());
                    let sw = Rect::from_min_size(r.min, vec2(66.0, 34.0));
                    let l = LayoutProfile { assets_right: ar, timeline_top: tt, ..cur.clone() };
                    let pnt = ui.painter();
                    pnt.rect_filled(sw, theme::R_CARD, theme::BG_INPUT());
                    let (areas, rail) = sketch(sw, &l);
                    for (i, a) in areas.into_iter().enumerate() {
                        // Player slightly lighter than the other panels
                        pnt.rect_filled(a, 1.5, if i == 1 { theme::BG_HOVER() } else { theme::BG_RAISED() });
                    }
                    pnt.rect_filled(rail, 1.0, theme::TEXT_FAINT());
                    let stroke = if on {
                        egui::Stroke::new(2.0_f32, theme::ACCENT())
                    } else if resp.hovered() {
                        egui::Stroke::new(1.0_f32, theme::TEXT_DIM())
                    } else {
                        egui::Stroke::new(1.0_f32, theme::BORDER())
                    };
                    pnt.rect_stroke(sw, theme::R_CARD, stroke, egui::StrokeKind::Outside);
                    pnt.text(
                        pos2(sw.center().x, r.bottom() - 6.0),
                        egui::Align2::CENTER_CENTER,
                        crate::tr(name),
                        egui::FontId::proportional(theme::FS_HINT),
                        if on { theme::TEXT() } else { theme::TEXT_DIM() },
                    );
                    if resp.on_hover_text(crate::tr(tip)).clicked() && !on {
                        pick = Some(l);
                    }
                }
            });
        });
        let mut l = pick.unwrap_or_else(|| cur.clone());
        crate::widgets::toggle_row(ui, "Zeitleiste über die ganze Breite", &mut l.timeline_full);
        theme::row(ui, "Symbolleiste", |ui| {
            let mut sel = l.rail_top as usize;
            ui.allocate_ui(vec2(200.0, 24.0), |ui| {
                if crate::widgets::segmented(ui, &["Seitlich", "Oben"], &mut sel) {
                    l.rail_top = sel == 1;
                }
            });
        });
        if l != cur {
            l.frac = self.layout_frac;
            self.apply_layout(l);
        }
        if crate::widgets::toggle_row(ui, "Layout sperren", &mut self.settings.layout_locked) {
            self.settings.save();
        }
    }

    /// Apply a profile (sizes are set in the next frame)
    pub fn apply_layout(&mut self, p: LayoutProfile) {
        self.layout_frac = p.frac;
        self.settings.layout = p;
        // forces setting the panel sizes from the shares
        self.layout_screen = Some(Rect::NOTHING);
        self.settings.save();
    }

    /// Edit mode: buttons above the panels and a bar for saving
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
                                _ => {}
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
        // Bar at the top: choose profiles, save, done
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
        // older settings without the field → default
        assert_eq!(serde_json::from_str::<LayoutProfile>("{}").unwrap(), LayoutProfile::default());
        // Profile from before `rail_top`: loads, toolbar at the side
        let old = r#"{"name":"Mein","assets_right":true,"timeline_top":true,"timeline_full":false,"frac":[0.3,0.2,0.4]}"#;
        let p = serde_json::from_str::<LayoutProfile>(old).unwrap();
        assert!(p.assets_right && p.timeline_top && !p.timeline_full && !p.rail_top && p.name == "Mein");
        assert!(builtin().iter().all(|p| !p.rail_top));
    }

    #[test]
    fn arrangement_tiles_and_sketch() {
        // four different tiles, the first is the default
        let mut flags: Vec<(bool, bool)> = ARRANGEMENTS.iter().map(|a| (a.2, a.3)).collect();
        let d = LayoutProfile::default();
        assert_eq!(flags[0], (d.assets_right, d.timeline_top));
        assert!(d.timeline_full && !d.rail_top);
        flags.sort();
        flags.dedup();
        assert_eq!(flags.len(), 4);
        // Sketches: panels lie within the tile, do not overlap, toolbar in the media panel
        let tile = Rect::from_min_size(pos2(10.0, 20.0), vec2(66.0, 34.0));
        for (_, _, ar, tt) in ARRANGEMENTS {
            for full in [true, false] {
                for rail_top in [true, false] {
                    let l = LayoutProfile { assets_right: ar, timeline_top: tt, timeline_full: full, rail_top, ..d.clone() };
                    let (areas, rail) = sketch(tile, &l);
                    for (i, a) in areas.iter().enumerate() {
                        assert!(a.width() > 1.0 && a.height() > 1.0 && tile.contains_rect(*a), "{l:?} {i}");
                        for b in &areas[i + 1..] {
                            assert!(!a.intersects(*b), "{l:?}");
                        }
                    }
                    assert!(areas[0].contains_rect(rail));
                    // Toolbar at the side: on the outer edge of the media panel
                    if !rail_top {
                        let outer = if ar { areas[0].right() == rail.right() } else { areas[0].left() == rail.left() };
                        assert!(outer);
                    }
                    // Media on the right side, timeline at top/bottom
                    assert_eq!(areas[0].center().x > areas[2].center().x, ar);
                    assert_eq!(areas[3].center().y < areas[1].center().y, tt);
                }
            }
        }
    }

}
