//! Einführung beim ersten Start (jederzeit überspringbar) und Rundgang durch den Editor.
//!
//! Einführung: Willkommen → „Wofür nutzt du Cutline?“ → passende Plugins, Modelle und Tipps.
//! Rundgang: hebt nacheinander die Bereiche des Editors hervor und erklärt sie.
//! Beides lässt sich über Menü → „Rundgang“ / „Einführung wiederholen“ erneut starten.

use std::collections::BTreeSet;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Id, Order, Rect, RichText, Sense, Stroke,
};

use crate::app::{App, Screen};
use crate::theme::{self, icon};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Uses,
    Suggest,
}

pub struct Onboard {
    pub step: Step,
    pub uses: BTreeSet<&'static str>,
    /// Tastenkürzel-Vorlage (Index in `keymap::PRESETS`)
    pub keys: usize,
    /// Angehakte Empfehlungen (Plugins, Modelle)
    pub picked: BTreeSet<&'static str>,
    picks_for: Option<BTreeSet<&'static str>>,
}

impl Onboard {
    pub fn new(previous: &[String]) -> Self {
        let uses = USES
            .iter()
            .map(|u| u.0)
            .filter(|id| previous.iter().any(|p| p == id))
            .collect();
        Self {
            step: Step::Welcome,
            uses,
            keys: 0,
            picked: BTreeSet::new(),
            picks_for: None,
        }
    }
}

/// (ID, Icon, Titel, Beschreibung)
pub const USES: &[(&str, &str, &str, &str)] = &[
    (
        "gaming",
        icon::GAME_CONTROLLER,
        "Gaming & Let's Plays",
        "Lange Aufnahmen, Highlights, Facecam",
    ),
    (
        "shorts",
        icon::DEVICE_MOBILE,
        "Shorts, TikTok & Reels",
        "Hochformat, Untertitel, schnelle Schnitte",
    ),
    (
        "talk",
        icon::MICROPHONE,
        "Podcasts & Interviews",
        "Mehrere Sprecher, Pausen und Füllwörter",
    ),
    (
        "vlog",
        icon::CAMERA,
        "Vlogs & Lifestyle",
        "Looks, Musik, Stock-Material",
    ),
    (
        "story",
        icon::FILM_SLATE,
        "Story-Serien & Hörspiele",
        "Figuren, Dialoge, mehrere Blickwinkel",
    ),
    (
        "music",
        icon::MUSIC_NOTES,
        "Musikvideos & Edits",
        "Beats, Übergänge, Effekte",
    ),
    (
        "work",
        icon::CHALKBOARD_TEACHER,
        "Schule, Arbeit & Tutorials",
        "Untertitel, Sprachaufnahme, Kapitel",
    ),
];

#[derive(Clone, Copy)]
enum Act {
    /// Plugin einschalten
    Plugin(&'static str),
    /// Modelle laden
    Models(&'static [&'static str]),
    /// Nur ein Hinweis, wo die Funktion zu finden ist
    Tip,
}

struct Sug {
    id: &'static str,
    icon: &'static str,
    title: &'static str,
    text: &'static str,
    note: &'static str,
    act: Act,
    uses: &'static [&'static str],
}

const SUGGESTIONS: &[Sug] = &[
    Sug {
        id: "asr",
        icon: icon::CLOSED_CAPTIONING,
        title: "Spracherkennung laden",
        text: "Automatische Untertitel und Füllwörter entfernen. Läuft lokal.",
        note: "ca. 690 MB",
        act: Act::Models(&["onnxruntime", "parakeet"]),
        uses: &["shorts", "talk", "work", "vlog", "gaming", "story"],
    },
    Sug {
        id: "rvm",
        icon: icon::USER_FOCUS,
        title: "Freistellen ohne Greenscreen",
        text: "Person vom Hintergrund trennen, z. B. Facecam über Gameplay.",
        note: "ca. 32 MB",
        act: Act::Models(&["onnxruntime", "rvm"]),
        uses: &["gaming", "shorts", "vlog"],
    },
    Sug {
        id: "epidemic",
        icon: icon::MUSIC_NOTE,
        title: "Epidemic Sound",
        text: "Musik und Soundeffekte direkt im Audio-Reiter.",
        note: "eigenes Konto",
        act: Act::Plugin("cutline.epidemic-sound"),
        uses: &["gaming", "shorts", "vlog", "music", "talk"],
    },
    Sug {
        id: "freesound",
        icon: icon::WAVEFORM,
        title: "Freesound",
        text: "Hunderttausende freie Soundeffekte im Audio-Reiter.",
        note: "kostenloser Schlüssel",
        act: Act::Plugin("cutline.freesound"),
        uses: &["gaming", "story", "vlog", "music"],
    },
    Sug {
        id: "pexels",
        icon: icon::IMAGES,
        title: "Pexels",
        text: "Kostenlose Stock-Videos und Fotos im Medien-Reiter.",
        note: "kostenloser Schlüssel",
        act: Act::Plugin("cutline.pexels"),
        uses: &["vlog", "work", "shorts"],
    },
    Sug {
        id: "story",
        icon: icon::USERS,
        title: "Story-Werkzeuge",
        text: "Figuren, Dialogboxen und Namensschilder mit einem Klick.",
        note: "",
        act: Act::Plugin("cutline.story-tools"),
        uses: &["story"],
    },
    Sug {
        id: "t_jump",
        icon: icon::SCISSORS,
        title: "Pausen entfernen",
        text: "Werkzeuge → Pausen entfernen schneidet Stille aus Sprechpassagen.",
        note: "",
        act: Act::Tip,
        uses: &["talk", "vlog", "work", "gaming"],
    },
    Sug {
        id: "t_high",
        icon: icon::STAR,
        title: "Highlights finden",
        text: "Werkzeuge → Highlights setzt Marker an lauten Momenten langer Aufnahmen.",
        note: "",
        act: Act::Tip,
        uses: &["gaming"],
    },
    Sug {
        id: "t_vert",
        icon: icon::DEVICE_MOBILE,
        title: "Hochkant-Layout",
        text: "Werkzeuge → Hochkant-Layout macht aus 16:9 ein 9:16-Video mit Facecam oben.",
        note: "",
        act: Act::Tip,
        uses: &["shorts", "gaming"],
    },
    Sug {
        id: "t_multi",
        icon: icon::VIDEO_CAMERA,
        title: "Multicam",
        text:
            "Mehrere Aufnahmen per Ton synchronisieren und mit 1–9 live zwischen ihnen schneiden.",
        note: "",
        act: Act::Tip,
        uses: &["talk", "story", "gaming"],
    },
    Sug {
        id: "t_duck",
        icon: icon::SPEAKER_LOW,
        title: "Auto-Ducking",
        text: "Werkzeuge → Musik unter Sprache absenken, ohne Keyframes von Hand.",
        note: "",
        act: Act::Tip,
        uses: &["talk", "vlog", "work"],
    },
    Sug {
        id: "t_beats",
        icon: icon::METRONOME,
        title: "Beat-Erkennung",
        text: "Beats eines Songs erkennen lassen, Clips rasten darauf ein.",
        note: "",
        act: Act::Tip,
        uses: &["music", "shorts"],
    },
    Sug {
        id: "t_chap",
        icon: icon::LIST_NUMBERS,
        title: "Kapitel aus Markern",
        text: "M setzt Marker, im Projekt-Reiter werden daraus YouTube-Kapitel.",
        note: "",
        act: Act::Tip,
        uses: &["talk", "work", "gaming"],
    },
    Sug {
        id: "t_recap",
        icon: icon::FILM_SLATE,
        title: "Rückblick aus Markern",
        text: "Werkzeuge → Rückblick schneidet markierte Momente zu einem „Was bisher geschah“.",
        note: "",
        act: Act::Tip,
        uses: &["story"],
    },
];

fn suggestions(uses: &BTreeSet<&'static str>) -> Vec<&'static Sug> {
    if uses.is_empty() {
        return SUGGESTIONS
            .iter()
            .filter(|s| matches!(s.id, "asr" | "t_jump" | "t_chap"))
            .collect();
    }
    SUGGESTIONS
        .iter()
        .filter(|s| s.uses.iter().any(|u| uses.contains(u)))
        .collect()
}

/// Rundgang: (Bereich, Titel, Text)
pub const TOUR: &[(&str, &str, &str)] = &[
    ("assets", "Bibliothek", "Medien importieren oder einfach hierher ziehen. Oben findest du Audio, Text, Sticker, Effekte, Übergänge, Filter und Untertitel. Mit dem Stern merkst du dir Favoriten, eigene Ordner legst du selbst an."),
    ("player", "Vorschau", "Zeigt das Bild am Playhead. Clips verschiebst, skalierst und drehst du direkt hier. Leertaste spielt ab, J/K/L spult."),
    ("inspector", "Inspektor", "Alles zum ausgewählten Clip: Bild, Ton, Geschwindigkeit, Animationen und Keyframes. Ohne Auswahl siehst du hier das Projekt und seine Marker."),
    ("timeline", "Zeitleiste", "Hier entsteht dein Video. Strg+B teilt, Q/W löscht links/rechts vom Playhead, M setzt einen Marker. Der Magnet hält die Hauptspur lückenlos."),
    ("tools", "Werkzeuge", "Pausen entfernen, Füllwörter, Auto-Ducking, Highlights, Multicam, Hochkant-Layout und Thumbnail mit einem Klick."),
    ("export", "Exportieren", "Vorlagen für YouTube, Shorts, TikTok, Instagram und Twitch. Ohne Wasserzeichen, bis 4K."),
    ("menu", "Menü", "Projekte, Vorlagen, Sicherungen, Einstellungen und Tastenkürzel. Den Rundgang findest du hier jederzeit wieder."),
];

/// Kachel zum Auswählen (Icon, Titel, kleine Zeile); `on` = ausgewählt.
fn choice_tile(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    glyph: &str,
    title: &str,
    hint: &str,
    on: bool,
) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let fill = if on {
        theme::BG_HOVER()
    } else if resp.hovered() {
        theme::BG_RAISED()
    } else {
        theme::BG_INPUT()
    };
    p.rect(
        r,
        theme::R_WIN,
        fill,
        // Fläche hebt sich schon ab: Rand nur noch für die Auswahl
        if on { Stroke::new(1.5_f32, theme::ACCENT()) } else { Stroke::NONE },
        egui::StrokeKind::Inside,
    );
    p.text(
        pos2(r.left() + 22.0, r.center().y),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(20.0), // Icon-Glyphe der Kachel
        if on { theme::ACCENT() } else { theme::TEXT_DIM() },
    );
    p.text(
        pos2(r.left() + 44.0, r.center().y - 8.0),
        Align2::LEFT_CENTER,
        crate::tr(title),
        FontId::proportional(theme::FS_TEXT),
        theme::TEXT(),
    );
    p.text(
        pos2(r.left() + 44.0, r.center().y + 9.0),
        Align2::LEFT_CENTER,
        crate::tr(hint),
        FontId::proportional(theme::FS_HINT),
        theme::TEXT_DIM(),
    );
    if on {
        p.text(
            pos2(r.right() - 16.0, r.center().y),
            Align2::CENTER_CENTER,
            icon::CHECK_CIRCLE,
            FontId::proportional(15.0), // Icon-Glyphe (Häkchen)
            theme::ACCENT(),
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl App {
    pub fn start_onboarding(&mut self) {
        self.tour = None;
        self.onboard = Some(Onboard::new(&self.settings.use_cases));
    }

    pub fn start_tour(&mut self) {
        self.onboard = None;
        if self.screen != Screen::Editor {
            self.new_project();
        }
        self.player_max = false;
        self.tour = Some(0);
    }

    fn finish_onboarding(&mut self, ctx: &egui::Context, ob: &Onboard, apply: bool) {
        self.settings.onboarding_done = true;
        if apply {
            self.settings.use_cases = ob.uses.iter().map(|s| s.to_string()).collect();
            if ob.keys > 0 {
                if let Some((_, diff)) = crate::keymap::PRESETS.get(ob.keys) {
                    self.settings.shortcuts = diff
                        .iter()
                        .map(|(a, k)| (a.to_string(), k.to_string()))
                        .collect();
                }
            }
        }
        self.settings.save();
        if !apply {
            return;
        }
        let mut need_key = false;
        let mut models: Vec<&'static str> = Vec::new();
        for s in SUGGESTIONS.iter().filter(|s| ob.picked.contains(s.id)) {
            match s.act {
                Act::Plugin(id) => {
                    self.set_plugin_enabled(id, true);
                    need_key |= id != "cutline.story-tools";
                }
                Act::Models(ids) => {
                    for id in ids {
                        if !models.contains(id) {
                            models.push(id);
                        }
                    }
                }
                Act::Tip => {}
            }
        }
        let m = self.models();
        for id in models {
            let skip = m.installed(id)
                || (id == "onnxruntime" && !cfg!(windows))
                || self.tasks.running(&format!("model:{id}")).is_some();
            if !skip {
                self.download_model(ctx, id);
            }
        }
        if need_key {
            self.toast("Schlüssel bzw. Anmeldung unter Einstellungen → Plugins und Konten eintragen");
        }
    }

    /// Einführung als Vollbild-Dialog (über allem, blockiert die Oberfläche dahinter).
    pub fn onboarding(&mut self, ctx: &egui::Context) {
        let Some(mut ob) = self.onboard.take() else {
            return;
        };
        let screen = ctx.screen_rect();
        // Titelleiste frei lassen: Fenster verschieben, minimieren, maximieren und schließen
        // bleiben während der Einführung möglich
        let below_title = Rect::from_min_max(screen.min + vec2(0.0, theme::TITLE_H), screen.max);
        egui::Area::new(Id::new("ob-dim"))
            .order(Order::Foreground)
            .fixed_pos(below_title.min)
            .show(ctx, |ui| {
                let (r, _) = ui.allocate_exact_size(below_title.size(), Sense::click_and_drag());
                ui.painter()
                    .rect_filled(r, 0.0, Color32::from_black_alpha(200));
            });
        let w = (screen.width() - 40.0).min(700.0);
        let h = (screen.height() - 40.0).min(540.0);
        // 0 = nichts, 1 = überspringen, 2 = fertig, 3 = fertig + Rundgang
        let mut end = 0u8;
        egui::Area::new(Id::new("ob-card")).order(Order::Tooltip).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
            egui::Frame::default()
                .fill(theme::BG_CARD())
                .stroke(Stroke::new(1.0_f32, theme::BORDER()))
                .corner_radius(theme::R_WIN)
                .inner_margin(28) // Hero-Abstand der Einführung
                .shadow(egui::Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(160) })
                .show(ui, |ui| {
                    ui.set_width(w - 56.0);
                    ui.set_height(h - 56.0);
                    let body_h = h - 56.0 - 44.0;
                    ui.allocate_ui_with_layout(vec2(w - 56.0, body_h), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_min_height(body_h);
                        match ob.step {
                            Step::Welcome => welcome(ui),
                            Step::Uses => uses_page(ui, &mut ob),
                            Step::Suggest => self.suggest_page(ui, &mut ob),
                        }
                    });
                    ui.add_space(12.0);
                    let idx = match ob.step {
                        Step::Welcome => 0,
                        Step::Uses => 1,
                        Step::Suggest => 2,
                    };
                    // Fortschrittspunkte mittig unter dem Inhalt
                    let row = ui.available_rect_before_wrap();
                    let c = pos2(row.center().x, row.top() + 15.0);
                    for i in 0..3 {
                        let p = c + vec2((i as f32 - 1.0) * 16.0, 0.0);
                        if i == idx {
                            ui.painter().rect_filled(Rect::from_center_size(p, vec2(16.0, 6.0)), 3.0, theme::ACCENT());
                        } else {
                            ui.painter().circle_filled(p, 3.0, theme::TEXT_FAINT());
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.set_min_height(30.0);
                        if idx > 0 && ui.add(theme::secondary(format!("{}  Zurück", icon::ARROW_LEFT))).clicked() {
                            ob.step = if idx == 2 { Step::Uses } else { Step::Welcome };
                        }
                        if ui.add(theme::ghost("Überspringen")).on_hover_text(crate::tr("Die Einführung findest du später unter Menü → Einführung wiederholen")).clicked() {
                            end = 1;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match ob.step {
                                Step::Welcome => {
                                    if ui.add(theme::primary(format!("Los geht's  {}", icon::ARROW_RIGHT)).min_size(vec2(130.0, 30.0))).clicked() {
                                        ob.step = Step::Uses;
                                    }
                                }
                                Step::Uses => {
                                    if ui.add(theme::primary(format!("Weiter  {}", icon::ARROW_RIGHT)).min_size(vec2(110.0, 30.0))).clicked() {
                                        ob.step = Step::Suggest;
                                    }
                                }
                                Step::Suggest => {
                                    if ui.add(theme::primary(format!("{}  Rundgang starten", icon::COMPASS)).min_size(vec2(150.0, 30.0))).on_hover_text(crate::tr("Übernimmt die Auswahl und zeigt dir die Bereiche des Editors")).clicked() {
                                        end = 3;
                                    }
                                    if ui.add(theme::secondary("Ohne Rundgang fertig").min_size(vec2(0.0, 30.0))).clicked() {
                                        end = 2;
                                    }
                                }
                            }
                        });
                    });
                });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            end = 1;
        }
        match end {
            0 => self.onboard = Some(ob),
            1 => self.finish_onboarding(ctx, &ob, false),
            2 => self.finish_onboarding(ctx, &ob, true),
            _ => {
                self.finish_onboarding(ctx, &ob, true);
                self.start_tour();
            }
        }
    }

    fn suggest_page(&mut self, ui: &mut egui::Ui, ob: &mut Onboard) {
        let list = suggestions(&ob.uses);
        // Vorauswahl neu setzen, wenn sich die Nutzung geändert hat
        if ob.picks_for.as_ref() != Some(&ob.uses) {
            let m = self.models();
            ob.picked = list
                .iter()
                .filter(|s| match s.act {
                    Act::Plugin(id) => !self
                        .plugins
                        .plugins
                        .iter()
                        .any(|p| p.manifest.plugin.id == id && p.enabled),
                    Act::Models(ids) => !ids.iter().all(|i| m.installed(i)),
                    Act::Tip => false,
                })
                // Downloads und Konten nur vorschlagen, nicht vorab ankreuzen – außer bei klarer Passung
                .filter(|s| {
                    !matches!(s.act, Act::Plugin(_))
                        || s.id == "story"
                        || s.id == "epidemic" && ob.uses.contains("music")
                })
                .map(|s| s.id)
                .collect();
            ob.picks_for = Some(ob.uses.clone());
        }
        ui.label(
            RichText::new(crate::tr("Passend für dich"))
                .size(theme::FS_HERO)
                .strong(),
        );
        ui.label(RichText::new(crate::tr("Alles ist optional und jederzeit unter Einstellungen und Plugins änderbar. Plugins sind ohne dein Okay immer aus.")).color(theme::TEXT_DIM()));
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let m = self.models();
                let actions: Vec<_> = list.iter().filter(|s| !matches!(s.act, Act::Tip)).collect();
                if !actions.is_empty() {
                    ui.label(
                        RichText::new(crate::tr("Einschalten und laden"))
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM()),
                    );
                    ui.add_space(4.0);
                    for s in actions {
                        let done = match s.act {
                            Act::Plugin(id) => self
                                .plugins
                                .plugins
                                .iter()
                                .any(|p| p.manifest.plugin.id == id && p.enabled),
                            Act::Models(ids) => ids.iter().all(|i| m.installed(i)),
                            Act::Tip => false,
                        };
                        let on = ob.picked.contains(s.id);
                        let (r, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 50.0),
                            if done { Sense::hover() } else { Sense::click() },
                        );
                        let p = ui.painter();
                        p.rect_filled(
                            r,
                            theme::R_WIN,
                            if resp.hovered() && !done {
                                theme::BG_RAISED()
                            } else {
                                theme::BG_INPUT()
                            },
                        );
                        p.text(
                            pos2(r.left() + 22.0, r.center().y),
                            Align2::CENTER_CENTER,
                            s.icon,
                            FontId::proportional(18.0), // Icon-Glyphe der Zeile
                            theme::TEXT_DIM(),
                        );
                        let title = if s.note.is_empty() {
                            crate::tr(s.title)
                        } else {
                            format!("{}  ·  {}", crate::tr(s.title), crate::tr(s.note))
                        };
                        p.text(
                            pos2(r.left() + 44.0, r.center().y - 8.0),
                            Align2::LEFT_CENTER,
                            title,
                            FontId::proportional(theme::FS_TEXT),
                            theme::TEXT(),
                        );
                        p.text(
                            pos2(r.left() + 44.0, r.center().y + 9.0),
                            Align2::LEFT_CENTER,
                            crate::tr(s.text),
                            FontId::proportional(theme::FS_HINT),
                            theme::TEXT_DIM(),
                        );
                        let c = pos2(r.right() - 22.0, r.center().y);
                        if done {
                            p.text(
                                c,
                                Align2::RIGHT_CENTER,
                                crate::tr("bereits aktiv"),
                                FontId::proportional(theme::FS_HINT),
                                theme::TEXT_FAINT(),
                            );
                        } else {
                            let b = Rect::from_center_size(c, vec2(16.0, 16.0));
                            p.rect(
                                b,
                                theme::R_CTRL,
                                if on {
                                    theme::ACCENT()
                                } else {
                                    Color32::TRANSPARENT
                                },
                                Stroke::new(
                                    1.5_f32,
                                    if on { theme::ACCENT() } else { theme::TEXT_FAINT() },
                                ),
                                egui::StrokeKind::Inside,
                            );
                            if on {
                                p.text(
                                    c,
                                    Align2::CENTER_CENTER,
                                    icon::CHECK,
                                    FontId::proportional(12.0), // Icon-Glyphe im Ankreuzfeld
                                    theme::ACCENT_INK(),
                                );
                            }
                        }
                        if resp.clicked() {
                            if on {
                                ob.picked.remove(s.id);
                            } else {
                                ob.picked.insert(s.id);
                            }
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_space(10.0);
                }
                let tips: Vec<_> = list.iter().filter(|s| matches!(s.act, Act::Tip)).collect();
                if !tips.is_empty() {
                    ui.label(
                        RichText::new(crate::tr("Schon eingebaut"))
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM()),
                    );
                    ui.add_space(4.0);
                    for s in tips {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [28.0, 20.0],
                                egui::Label::new(
                                    RichText::new(s.icon).size(15.0) /* Icon-Glyphe */.color(theme::TEXT_DIM()),
                                ),
                            );
                            ui.label(RichText::new(crate::tr(s.title)).strong());
                            ui.label(
                                RichText::new(crate::tr(s.text))
                                    .small()
                                    .color(theme::TEXT_DIM()),
                            );
                        });
                        ui.add_space(2.0);
                    }
                }
            });
    }

    /// Rundgang: Bereich hervorheben, Erklärung daneben.
    pub fn tour_overlay(&mut self, ctx: &egui::Context) {
        let Some(i) = self.tour else {
            return;
        };
        if self.screen != Screen::Editor || i >= TOUR.len() {
            self.tour = None;
            return;
        }
        let (key, title, text) = TOUR[i];
        let screen = ctx.screen_rect();
        let hole = self
            .tour_rects
            .get(key)
            .map(|r| r.expand(4.0).intersect(screen));
        // Titelleiste frei lassen: Fenster verschieben, minimieren, maximieren und schließen
        // bleiben während der Einführung möglich
        let below_title = Rect::from_min_max(screen.min + vec2(0.0, theme::TITLE_H), screen.max);
        egui::Area::new(Id::new("tour-dim"))
            .order(Order::Foreground)
            .fixed_pos(below_title.min)
            .show(ctx, |ui| {
                let (r, _) = ui.allocate_exact_size(below_title.size(), Sense::click_and_drag());
                let p = ui.painter();
                let dim = Color32::from_black_alpha(185);
                match hole {
                    Some(h) => {
                        p.rect_filled(
                            Rect::from_min_max(r.min, pos2(r.right(), h.top())),
                            0.0,
                            dim,
                        );
                        p.rect_filled(
                            Rect::from_min_max(pos2(r.left(), h.bottom()), r.max),
                            0.0,
                            dim,
                        );
                        p.rect_filled(
                            Rect::from_min_max(pos2(r.left(), h.top()), pos2(h.left(), h.bottom())),
                            0.0,
                            dim,
                        );
                        p.rect_filled(
                            Rect::from_min_max(
                                pos2(h.right(), h.top()),
                                pos2(r.right(), h.bottom()),
                            ),
                            0.0,
                            dim,
                        );
                        p.rect_stroke(
                            h,
                            theme::R_CARD,
                            Stroke::new(2.0_f32, theme::ACCENT()),
                            egui::StrokeKind::Outside,
                        );
                    }
                    None => {
                        p.rect_filled(r, 0.0, dim);
                    }
                }
            });
        // Erklärung: rechts, links, unten oder oben vom Bereich – wo Platz ist
        let size = vec2(330.0, 170.0);
        let pos = hole
            .and_then(|h| {
                let gap = 14.0;
                let y = (h.center().y - size.y / 2.0)
                    .clamp(screen.top() + 8.0, screen.bottom() - size.y - 8.0);
                let x = (h.center().x - size.x / 2.0)
                    .clamp(screen.left() + 8.0, screen.right() - size.x - 8.0);
                [
                    pos2(h.right() + gap, y),
                    pos2(h.left() - gap - size.x, y),
                    pos2(x, h.bottom() + gap),
                    pos2(x, h.top() - gap - size.y),
                ]
                .into_iter()
                .find(|p| {
                    screen
                        .shrink(4.0)
                        .contains_rect(Rect::from_min_size(*p, size))
                })
            })
            .unwrap_or_else(|| screen.center() - size / 2.0);
        let mut go: i32 = 0;
        let mut quit = false;
        egui::Area::new(Id::new("tour-card"))
            .order(Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::BG_CARD())
                    .stroke(Stroke::new(1.0_f32, theme::BORDER()))
                    .corner_radius(theme::R_WIN)
                    .inner_margin(theme::PAD_DIALOG)
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .show(ui, |ui| {
                        ui.set_width(size.x - 32.0);
                        ui.label(
                            RichText::new(crate::tr(format!(
                                "Rundgang · {} von {}",
                                i + 1,
                                TOUR.len()
                            )))
                            .small()
                            .color(theme::TEXT_FAINT()),
                        );
                        ui.add_space(2.0);
                        ui.label(RichText::new(crate::tr(title)).size(theme::FS_PAGE).strong());
                        ui.add_space(4.0);
                        ui.label(RichText::new(crate::tr(text)).color(theme::TEXT_DIM()));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add(theme::ghost("Beenden")).clicked() {
                                quit = true;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let last = i + 1 == TOUR.len();
                                    if ui
                                        .add(theme::primary(if last {
                                            "Fertig".to_string()
                                        } else {
                                            format!("Weiter  {}", icon::ARROW_RIGHT)
                                        }))
                                        .clicked()
                                    {
                                        go = 1;
                                    }
                                    if i > 0 && ui.add(theme::secondary(icon::ARROW_LEFT)).clicked()
                                    {
                                        go = -1;
                                    }
                                },
                            );
                        });
                    });
            });
        ctx.input(|inp| {
            if inp.key_pressed(egui::Key::Escape) {
                quit = true;
            }
            if inp.key_pressed(egui::Key::ArrowRight) || inp.key_pressed(egui::Key::Enter) {
                go = 1;
            }
            if inp.key_pressed(egui::Key::ArrowLeft) {
                go = -1;
            }
        });
        if quit || (go == 1 && i + 1 == TOUR.len()) {
            self.tour = None;
            self.toast("Rundgang und Einführung findest du jederzeit im Menü");
        } else if go != 0 {
            self.tour = Some((i as i32 + go).max(0) as usize);
        }
    }
}

fn welcome(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(18.0);
        let (r, _) = ui.allocate_exact_size(vec2(76.0, 76.0), Sense::hover());
        theme::logo(ui.painter(), r);
        ui.add_space(16.0);
        ui.label(
            RichText::new(crate::tr("Willkommen bei Cutline"))
                .size(theme::FS_HERO)
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::tr(
                "Einfach zu bedienen, schnell, komplett auf deinem Rechner.",
            ))
            .size(theme::FS_TEXT)
            .color(theme::TEXT_DIM()),
        );
    });
    ui.add_space(30.0);
    let w = ui.available_width();
    let cw = (w - 24.0) / 3.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for (g, t, d) in [
            (icon::CPU, "Einfach schneiden", "Vertraute Bedienung: reinziehen, schneiden, exportieren. Jede Funktion ist ab dem ersten Start freigeschaltet."),
            (icon::SHIELD_CHECK, "Schnell arbeiten", "In Rust geschrieben. Flüssige Vorschau mit Proxys, schnelle Exporte mit Hardware-Encodern."),
            (icon::PUZZLE_PIECE, "Offen", "Open Source, läuft lokal, ohne Konto, ohne Abo, ohne Wasserzeichen. Projekte sind offenes JSON."),
        ] {
            egui::Frame::default().fill(theme::BG_INPUT()).corner_radius(theme::R_WIN).inner_margin(theme::PAD_CARD_L).show(ui, |ui| {
                ui.set_width(cw - 2.0 * theme::PAD_CARD_L as f32);
                ui.set_min_height(104.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(g).size(22.0) /* Icon-Glyphe der Kachel */.color(theme::TEXT()));
                    ui.add_space(4.0);
                    ui.label(RichText::new(crate::tr(t)).strong());
                    ui.label(RichText::new(crate::tr(d)).small().color(theme::TEXT_DIM()));
                });
            });
        }
    });
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(crate::tr(
                "In drei kurzen Schritten richtest du Cutline für deine Videos ein.",
            ))
            .small()
            .color(theme::TEXT_FAINT()),
        );
    });
}

fn uses_page(ui: &mut egui::Ui, ob: &mut Onboard) {
    ui.label(
        RichText::new(crate::tr("Wofür nutzt du Cutline?"))
            .size(theme::FS_HERO)
            .strong(),
    );
    ui.label(
        RichText::new(crate::tr(
            "Mehrfachauswahl. Danach schlagen wir passende Werkzeuge und Plugins vor.",
        ))
        .color(theme::TEXT_DIM()),
    );
    ui.add_space(12.0);
    let w = ui.available_width();
    let tw = (w - 10.0) / 2.0;
    for row in USES.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (id, g, t, d) in row {
                let on = ob.uses.contains(id);
                if choice_tile(ui, vec2(tw, 52.0), g, t, d, on).clicked() {
                    if on {
                        ob.uses.remove(id);
                    } else {
                        ob.uses.insert(id);
                    }
                }
            }
        });
        ui.add_space(6.0);
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{}  {}",
                icon::KEYBOARD,
                crate::tr("Tastenkürzel wie in")
            ))
            .color(theme::TEXT_DIM()),
        );
        ui.add_space(6.0);
        let mut sel = ob.keys;
        ui.allocate_ui(vec2(360.0, 24.0), |ui| {
            if crate::widgets::segmented(
                ui,
                &["Cutline", "Premiere Pro", "DaVinci Resolve"],
                &mut sel,
            ) {
                ob.keys = sel;
            }
        });
    });
}
