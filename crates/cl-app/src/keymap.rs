//! Tastenkürzel: Aktionen mit Standardbelegung, eigene Belegungen aus den Einstellungen,
//! Vorlagen (Cutline, Premiere-ähnlich, Resolve-ähnlich) und die Einstellungsseite zum Ändern.

use std::collections::BTreeMap;

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::app::App;
use crate::theme;

/// Eine Tastenkombination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub mods: Modifiers,
    pub key: Key,
}

impl Chord {
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = Modifiers::NONE;
        let mut key = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "strg" | "cmd" => mods |= Modifiers::COMMAND,
                "shift" | "umschalt" => mods |= Modifiers::SHIFT,
                "alt" => mods |= Modifiers::ALT,
                _ => key = Key::from_name(part),
            }
        }
        // „Ctrl++“ = Plus
        if key.is_none() && s.ends_with("++") {
            key = Some(Key::Plus);
        }
        Some(Self { mods, key: key? })
    }

    /// Anzeige, z. B. „Strg+Umschalt+Z“ (wird von `tr()` übersetzt)
    pub fn label(&self) -> String {
        let mut v = Vec::new();
        if self.mods.command {
            v.push("Strg".to_string());
        }
        if self.mods.alt {
            v.push("Alt".into());
        }
        if self.mods.shift {
            v.push("Umschalt".into());
        }
        v.push(
            match self.key {
                Key::Space => "Leertaste",
                Key::Delete => "Entf",
                Key::Home => "Pos1",
                Key::End => "Ende",
                Key::ArrowLeft => "←",
                Key::ArrowRight => "→",
                Key::ArrowUp => "↑",
                Key::ArrowDown => "↓",
                Key::Comma => ",",
                k => k.name(),
            }
            .to_string(),
        );
        v.join("+")
    }

    /// Speicherform, z. B. „Ctrl+Shift+Z“
    pub fn store(&self) -> String {
        let mut v = Vec::new();
        if self.mods.command {
            v.push("Ctrl");
        }
        if self.mods.alt {
            v.push("Alt");
        }
        if self.mods.shift {
            v.push("Shift");
        }
        v.push(self.key.name());
        v.join("+")
    }
}

/// (Aktion, Beschreibung, Standardbelegungen)
pub const ACTIONS: &[(&str, &str, &[&str])] = &[
    ("play", "Abspielen / Pause", &["Space"]),
    (
        "shuttle_back",
        "Rückwärts abspielen (mehrfach: schneller)",
        &["J"],
    ),
    ("shuttle_stop", "Anhalten", &["K"]),
    (
        "shuttle_fwd",
        "Vorwärts abspielen (mehrfach: schneller)",
        &["L"],
    ),
    ("mark_in", "In-Punkt setzen", &["I"]),
    ("mark_out", "Out-Punkt setzen", &["O"]),
    ("clear_range", "In/Out löschen", &["Alt+X"]),
    ("loop", "Bereich in Schleife abspielen", &["Ctrl+L"]),
    ("marker", "Marker setzen", &["M"]),
    ("marker_next", "Nächster Marker", &["Shift+M"]),
    ("marker_prev", "Vorheriger Marker", &["Ctrl+Shift+M"]),
    ("split", "Am Playhead teilen", &["Ctrl+B"]),
    ("del_left", "Links vom Playhead löschen", &["Q"]),
    ("del_right", "Rechts vom Playhead löschen", &["W"]),
    (
        "ripple_delete",
        "Löschen und Lücke schließen",
        &["Shift+Delete", "Shift+Backspace"],
    ),
    ("delete", "Auswahl löschen", &["Delete", "Backspace"]),
    ("nudge_left", "Clip 1 Bild nach links", &["Alt+ArrowLeft"]),
    (
        "nudge_right",
        "Clip 1 Bild nach rechts",
        &["Alt+ArrowRight"],
    ),
    (
        "nudge_left10",
        "Clip 10 Bilder nach links",
        &["Alt+Shift+ArrowLeft"],
    ),
    (
        "nudge_right10",
        "Clip 10 Bilder nach rechts",
        &["Alt+Shift+ArrowRight"],
    ),
    // kommen meist als Kopier-/Einfügeereignis, nicht als Taste (siehe `App::shortcuts`)
    ("copy", "Kopieren", &["Ctrl+C"]),
    ("cut", "Ausschneiden", &["Ctrl+X"]),
    ("paste", "Einfügen", &["Ctrl+V"]),
    ("paste_attrs", "Attribute einfügen", &["Ctrl+Alt+V"]),
    ("group", "Gruppieren", &["Ctrl+G"]),
    ("ungroup", "Gruppe auflösen", &["Ctrl+Shift+G"]),
    ("duplicate", "Duplizieren", &["Ctrl+D"]),
    ("select_all", "Alle Clips auswählen", &["Ctrl+A"]),
    ("disable", "Clip deaktivieren", &["V"]),
    ("tool_select", "Werkzeug: Auswählen", &["A"]),
    ("tool_blade", "Werkzeug: Klinge", &["B"]),
    ("undo", "Rückgängig", &["Ctrl+Z"]),
    ("redo", "Wiederholen", &["Ctrl+Shift+Z", "Ctrl+Y"]),
    ("prev", "Ein Bild zurück", &["ArrowLeft"]),
    ("next", "Ein Bild vor", &["ArrowRight"]),
    ("prev_edit", "Zum vorherigen Schnitt", &["ArrowUp"]),
    ("next_edit", "Zum nächsten Schnitt", &["ArrowDown"]),
    ("home", "Zum Anfang", &["Home"]),
    ("end", "Zum Ende", &["End"]),
    ("magnet", "Hauptspur-Magnet", &["P"]),
    ("snap", "Einrasten", &["N"]),
    ("voiceover", "Einsprechen", &["R"]),
    ("max", "Großer Player", &["F"]),
    ("import", "Importieren", &["Ctrl+I"]),
    ("save", "Speichern", &["Ctrl+S"]),
    ("save_as", "Speichern unter", &["Ctrl+Shift+S"]),
    ("open", "Öffnen", &["Ctrl+O"]),
    ("export", "Exportieren", &["Ctrl+E"]),
    ("settings", "Einstellungen", &["Ctrl+Comma"]),
    ("command_search", "Befehlssuche", &["Ctrl+K"]),
    (
        "zoom_in",
        "Oberfläche größer",
        &["Ctrl+Plus", "Ctrl+Equals"],
    ),
    ("zoom_out", "Oberfläche kleiner", &["Ctrl+Minus"]),
    ("zoom_reset", "Oberfläche 100 %", &["Ctrl+0"]),
    ("escape", "Auswahl aufheben", &["Escape"]),
    // Menü oben links (Namen wie im Menü)
    ("new_project", "Neues Projekt", &[]),
    ("import_project", "Projekt aus anderem Programm importieren …", &[]),
    ("import_fonts", "Schrift importieren …", &[]),
    ("exports", "Exporte", &[]),
    ("history", "Bearbeitungsverlauf", &[]),
    ("layout_edit", "Layout bearbeiten …", &[]),
    ("template_save", "Als Vorlage speichern …", &[]),
    ("template_insert", "Aus Vorlage einfügen …", &[]),
    ("archive", "Projekt archivieren …", &[]),
    ("relink", "Medien neu verknüpfen …", &[]),
    ("backups", "Sicherungen …", &[]),
    ("go_home", "Projektübersicht", &[]),
    ("tour", "Rundgang", &[]),
    ("onboarding", "Einführung wiederholen", &[]),
    // Schnellaktionen (Menü in der Zeitleiste)
    ("quick_pauses", "Pausen entfernen (Jump Cuts)", &[]),
    ("quick_fillers", "Füllwörter entfernen (äh, ähm, uh, um)", &[]),
    ("quick_ducking", "Musik unter Sprache absenken (Auto-Ducking)", &[]),
    ("quick_highlights", "Highlights finden (laute Momente → Marker)", &[]),
    ("quick_sync", "Auswahl per Ton synchronisieren", &[]),
    ("quick_multicam", "Multicam-Schnitt starten", &[]),
    ("quick_speakers", "Sprecher-Aufnahmen importieren …", &[]),
    ("quick_punch", "Zoom-Punch-in am Playhead", &[]),
    ("quick_vertical", "Hochkant-Layout (9:16, Facecam oben)", &[]),
    // Untermenü „Layout“ (`arrange.rs`); im Menü ohne „Layout: “
    ("quick_layout_side", "Layout: Nebeneinander", &[]),
    ("quick_layout_stack", "Layout: Übereinander", &[]),
    ("quick_layout_big_left", "Layout: Einer groß, zwei rechts", &[]),
    ("quick_layout_columns", "Layout: Drei Spalten", &[]),
    ("quick_layout_grid", "Layout: Raster 2×2", &[]),
    ("quick_layout_pip_tl", "Layout: Bild-in-Bild – oben links", &[]),
    ("quick_layout_pip_tr", "Layout: Bild-in-Bild – oben rechts", &[]),
    ("quick_layout_pip_bl", "Layout: Bild-in-Bild – unten links", &[]),
    ("quick_layout_pip_br", "Layout: Bild-in-Bild – unten rechts", &[]),
    ("quick_layout_reset", "Layout zurücksetzen", &[]),
    ("quick_thumbnail", "Thumbnail …", &[]),
    ("quick_recap", "Rückblick aus Markern erstellen", &[]),
    ("censor", "Zensieren …", &[]),
    // Brand Kit (Einstellungen → Brand Kit)
    ("brand_logo", "Logo einfügen", &[]),
];

/// Name einer Aktion (deutsch, `tr()` übersetzt), leer für unbekannte
pub fn name(id: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.0 == id).map_or("", |a| a.1)
}

/// Symbol einer Aktion (wie in Menüs und Werkzeugleiste), leer ohne
pub fn icon(id: &str) -> &'static str {
    use crate::theme::icon as i;
    match id {
        "play" => i::PLAY,
        "shuttle_stop" => i::STOP,
        "mark_in" | "mark_out" => i::BRACKETS_SQUARE,
        "clear_range" => i::X,
        "loop" => i::REPEAT,
        "marker" => i::BOOKMARK_SIMPLE,
        "split" | "cut" => i::SCISSORS,
        "del_left" => i::ARROW_LINE_LEFT,
        "del_right" => i::ARROW_LINE_RIGHT,
        "ripple_delete" => i::TRASH_SIMPLE,
        "delete" => i::TRASH,
        "copy" => i::COPY,
        "paste" => i::CLIPBOARD_TEXT,
        "paste_attrs" => i::CLIPBOARD,
        "group" => i::BOUNDING_BOX,
        "ungroup" => i::SELECTION_SLASH,
        "duplicate" | "template_save" | "template_insert" => i::COPY_SIMPLE,
        "undo" => i::ARROW_COUNTER_CLOCKWISE,
        "redo" => i::ARROW_CLOCKWISE,
        "magnet" => i::MAGNET,
        "snap" => i::ARROWS_IN_LINE_HORIZONTAL,
        "voiceover" => i::MICROPHONE,
        "new_project" => i::FILE_PLUS,
        "import" => i::FILE_ARROW_DOWN,
        "import_project" => i::DOWNLOAD_SIMPLE,
        "import_fonts" => i::TEXT_AA,
        "save" => i::FLOPPY_DISK,
        "save_as" => i::FLOPPY_DISK_BACK,
        "open" => i::FOLDER_OPEN,
        "export" => i::EXPORT,
        "exports" => i::FILE_VIDEO,
        "history" | "backups" | "quick_recap" => i::CLOCK_COUNTER_CLOCKWISE,
        "layout_edit" => i::LAYOUT,
        "archive" => i::ARCHIVE,
        "relink" => i::LINK,
        "settings" => i::GEAR_SIX,
        "brand_logo" => i::IMAGE,
        "go_home" => i::HOUSE,
        "tour" => i::COMPASS,
        "onboarding" => i::HAND_WAVING,
        "command_search" | "zoom_in" | "zoom_out" | "zoom_reset" => i::MAGNIFYING_GLASS,
        "quick_pauses" => i::SCISSORS,
        "quick_fillers" => i::CHAT_CIRCLE_DOTS,
        "quick_ducking" => i::SPEAKER_SIMPLE_LOW,
        "quick_highlights" => i::LIGHTNING,
        "quick_sync" => i::WAVEFORM,
        "quick_multicam" => i::VIDEO_CAMERA,
        "quick_speakers" => i::USERS,
        "quick_punch" => i::MAGNIFYING_GLASS_PLUS,
        "quick_vertical" => i::DEVICE_MOBILE,
        "quick_thumbnail" => i::IMAGE,
        "quick_layout_side" => i::SQUARE_SPLIT_HORIZONTAL,
        "quick_layout_stack" => i::SQUARE_SPLIT_VERTICAL,
        "quick_layout_big_left" => i::LAYOUT,
        "quick_layout_columns" => i::COLUMNS,
        "quick_layout_grid" => i::SQUARES_FOUR,
        "quick_layout_pip_tl" | "quick_layout_pip_tr" | "quick_layout_pip_bl" | "quick_layout_pip_br" => i::PICTURE_IN_PICTURE,
        "quick_layout_reset" => i::ARROW_COUNTER_CLOCKWISE,
        _ => "",
    }
}

/// Befehlssuche: Reihenfolge der Treffer. `items` = (Aktion, Namen, nach denen gesucht wird).
/// Jedes Wort der Eingabe muss in einem Namen vorkommen (Groß/Klein egal); Aktionen, deren Wörter
/// alle an einem Wortanfang stehen, kommen zuerst, sonst bleibt die Reihenfolge der Liste.
/// Leere Eingabe: zuletzt benutzte (`recent`) zuerst, darunter alle übrigen.
pub fn search(query: &str, items: &[(&'static str, Vec<String>)], recent: &[&str]) -> Vec<&'static str> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        let mut out: Vec<&'static str> = recent.iter().filter_map(|r| items.iter().find(|i| i.0 == *r).map(|i| i.0)).collect();
        let rest: Vec<&'static str> = items.iter().map(|i| i.0).filter(|id| !out.contains(id)).collect();
        out.extend(rest);
        return out;
    }
    // 0 = Wortanfang, 1 = Wortmitte, None = fehlt
    let hit = |name: &str, w: &str| -> Option<u8> {
        let n = name.to_lowercase();
        let mut best = None;
        for (i, _) in n.match_indices(w) {
            if n[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric()) {
                return Some(0);
            }
            best = Some(1);
        }
        best
    };
    let mut ranked: Vec<(u8, &'static str)> = items
        .iter()
        .filter_map(|(id, names)| {
            let mut worst = 0;
            for w in &words {
                worst = worst.max(names.iter().filter_map(|n| hit(n, w)).min()?);
            }
            Some((worst, *id))
        })
        .collect();
    ranked.sort_by_key(|r| r.0);
    ranked.into_iter().map(|r| r.1).collect()
}

/// Vorlagen: (Name, Abweichungen von der Standardbelegung)
pub const PRESETS: &[(&str, &[(&str, &str)])] = &[
    ("Cutline", &[]),
    (
        "Premiere-ähnlich",
        &[
            ("split", "Ctrl+K"),
            ("command_search", "Ctrl+Shift+K"),
            ("ripple_delete", "Shift+Delete"),
            ("del_left", "Q"),
            ("del_right", "W"),
            ("max", "Ctrl+Backslash"),
        ],
    ),
    (
        "Resolve-ähnlich",
        &[
            ("split", "Ctrl+Backslash"),
            ("disable", "D"),
            ("snap", "N"),
            ("magnet", "Ctrl+Shift+P"),
        ],
    ),
];

/// Wirksame Belegung: eigene Einträge ersetzen die Standardbelegung der Aktion. Eine Standardtaste,
/// die schon eine eigene Belegung benutzt, entfällt (eine neue Aktion nimmt niemandem seine Taste).
pub fn bindings(custom: &BTreeMap<String, String>) -> Vec<(Chord, &'static str)> {
    let taken: Vec<Chord> = custom.values().filter_map(|c| Chord::parse(c)).collect();
    let mut v: Vec<(Chord, &'static str)> = Vec::new();
    for (id, _, defaults) in ACTIONS {
        match custom.get(*id) {
            Some(c) if c.is_empty() => {}
            Some(c) => v.extend(Chord::parse(c).map(|ch| (ch, *id))),
            None => v.extend(
                defaults
                    .iter()
                    .filter_map(|d| Chord::parse(d))
                    .filter(|ch| !taken.contains(ch))
                    .map(|ch| (ch, *id)),
            ),
        }
    }
    // Kombinationen mit mehr Modifikatoren zuerst prüfen (Strg+Umschalt+Z vor Strg+Z)
    v.sort_by_key(|(c, _)| {
        std::cmp::Reverse(c.mods.command as u8 + c.mods.shift as u8 + c.mods.alt as u8)
    });
    v
}

/// Tooltip „Text  (Kürzel)“; ohne belegte Taste nur der Text. Das Wörterbuch übersetzt beide
/// Teile einzeln, `theme::tip_with_keys` zeigt das Kürzel als Tastenkappe.
pub fn with_key(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else {
        format!("{text}  ({key})")
    }
}

impl App {

    /// Einstellungsseite „Tastenkürzel“: Belegung ändern (Klick, dann Tasten drücken).
    pub fn shortcuts_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr("Vorlage")).color(theme::TEXT_DIM()));
            for (name, diff) in PRESETS {
                if ui.add(theme::secondary(*name)).clicked() {
                    self.settings.shortcuts = diff
                        .iter()
                        .map(|(a, k)| (a.to_string(), k.to_string()))
                        .collect();
                    self.settings.save();
                    self.capture_key = None;
                }
            }
        });
        ui.add_space(6.0);
        // Aufnahme einer neuen Belegung
        if let Some(id) = self.capture_key {
            let got = ui.ctx().input_mut(|i| {
                let mut r = None;
                for e in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = e
                    {
                        r = Some((*key, *modifiers));
                    }
                }
                if r.is_some() {
                    i.events.clear();
                }
                r
            });
            if let Some((key, mods)) = got {
                match key {
                    Key::Escape => {}
                    Key::Delete | Key::Backspace if mods.is_none() => {
                        self.settings
                            .shortcuts
                            .insert(id.to_string(), String::new());
                    }
                    _ => {
                        let ch = Chord {
                            mods: Modifiers {
                                command: mods.command || mods.ctrl,
                                ctrl: false,
                                mac_cmd: false,
                                ..mods
                            },
                            key,
                        };
                        // Doppelte Belegung: bei der anderen Aktion entfernen
                        let taken: Vec<&str> = bindings(&self.settings.shortcuts)
                            .into_iter()
                            .filter(|(c, a)| *c == ch && *a != id)
                            .map(|(_, a)| a)
                            .collect();
                        for a in taken {
                            self.settings.shortcuts.insert(a.to_string(), String::new());
                        }
                        self.settings.shortcuts.insert(id.to_string(), ch.store());
                    }
                }
                self.capture_key = None;
                self.settings.save();
            }
        }
        let all = bindings(&self.settings.shortcuts);
        egui::Grid::new("keys")
            .num_columns(3)
            .spacing(egui::vec2(16.0, 6.0))
            .show(ui, |ui| {
                for (id, what, _) in ACTIONS {
                    ui.label(RichText::new(crate::tr(*what)).color(theme::TEXT_DIM()));
                    let keys: Vec<String> = all
                        .iter()
                        .filter(|(_, a)| a == id)
                        .map(|(c, _)| c.label())
                        .collect();
                    let capturing = self.capture_key == Some(*id);
                    // Belegung als Tastenkappen; die ganze Fläche ist der Knopf
                    let r = ui
                        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                            egui::Frame::default()
                                .fill(if capturing { theme::ACCENT() } else { theme::BG_RAISED() })
                                .corner_radius(theme::R_CARD)
                                .inner_margin(egui::Margin::symmetric(6, 3))
                                .show(ui, |ui| {
                                    ui.set_min_size(egui::vec2(170.0, 19.0));
                                    ui.horizontal(|ui| {
                                        if capturing {
                                            ui.label(RichText::new(crate::tr("Taste drücken …")).color(theme::ACCENT_INK()));
                                        } else if keys.is_empty() {
                                            ui.label(RichText::new("—").color(theme::TEXT_FAINT()));
                                        }
                                        for (i, k) in keys.iter().enumerate().filter(|_| !capturing) {
                                            if i > 0 {
                                                ui.label(RichText::new("/").small().color(theme::TEXT_FAINT()));
                                            }
                                            theme::keycaps(ui, k);
                                        }
                                    });
                                });
                        })
                        .response;
                    theme::hover_glow(ui, &r, 6.0);
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.capture_key = Some(id);
                    }
                    if self.settings.shortcuts.contains_key(*id) {
                        if theme::icon_button(
                            ui,
                            crate::theme::icon::ARROW_COUNTER_CLOCKWISE,
                            "Standard wiederherstellen",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.settings.shortcuts.remove(*id);
                            self.settings.save();
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.label(RichText::new(crate::tr("Außerdem: Strg+C / X / V kopieren, ausschneiden, einfügen · Strg+Mausrad zoomt die Zeitleiste · Umschalt+Mausrad scrollt waagerecht · Alt+Umschalt+Z passt den Player ein")).small().color(theme::TEXT_DIM()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_with_and_without_key() {
        assert_eq!(with_key("Teilen", "Strg+K"), "Teilen  (Strg+K)");
        assert_eq!(with_key("Teilen", ""), "Teilen");
        // so zerlegt es die Tooltip-Anzeige wieder in Text und Tastenkappe
        assert_eq!(with_key("Teilen", "Q").rsplit_once("  ("), Some(("Teilen", "Q)")));
    }


    #[test]
    fn parse_store_and_override() {
        let c = Chord::parse("Ctrl+Shift+Z").unwrap();
        assert!(c.mods.command && c.mods.shift && c.key == Key::Z);
        assert_eq!(Chord::parse(&c.store()), Some(c));
        assert_eq!(c.label(), "Strg+Umschalt+Z");
        let mut custom = BTreeMap::new();
        custom.insert("split".to_string(), "Ctrl+K".to_string());
        custom.insert("disable".to_string(), String::new());
        let b = bindings(&custom);
        assert!(b.iter().any(|(c, a)| *a == "split" && c.key == Key::K));
        assert!(!b.iter().any(|(c, a)| *a == "split" && c.key == Key::B));
        assert!(!b.iter().any(|(_, a)| *a == "disable"));
        // Strg+Umschalt+Z kommt vor Strg+Z
        let redo = b
            .iter()
            .position(|(c, a)| *a == "redo" && c.mods.shift)
            .unwrap();
        let undo = b.iter().position(|(_, a)| *a == "undo").unwrap();
        assert!(redo < undo);
        for (id, _, d) in ACTIONS {
            for k in *d {
                assert!(Chord::parse(k).is_some(), "{id}: {k}");
            }
        }
    }

    /// Standard und jede Vorlage: keine Taste für zwei Aktionen, nur bekannte Aktionen,
    /// jede Belegung lesbar und verlustfrei speicherbar.
    #[test]
    fn presets_have_no_conflicts() {
        let ids: Vec<&str> = ACTIONS.iter().map(|a| a.0).collect();
        let mut seen = std::collections::HashSet::new();
        assert!(ids.iter().all(|i| seen.insert(*i)), "doppelte Aktions-Id");
        for (name, diff) in PRESETS {
            let custom: BTreeMap<String, String> = diff.iter().map(|(a, k)| (a.to_string(), k.to_string())).collect();
            for (a, k) in *diff {
                assert!(ids.contains(a), "{name}: unbekannte Aktion {a}");
                let c = Chord::parse(k).unwrap_or_else(|| panic!("{name}: {k}"));
                assert_eq!(Chord::parse(&c.store()), Some(c), "{name}: {k}");
            }
            let b = bindings(&custom);
            for (i, (c, a)) in b.iter().enumerate() {
                for (c2, a2) in &b[i + 1..] {
                    assert!(!(c == c2 && a != a2), "{name}: {} für {a} und {a2}", c.store());
                }
            }
        }
    }

    /// Befehlssuche: Standard Strg+K, im Premiere-Preset Strg+Umschalt+K (dort teilt Strg+K);
    /// wer Strg+K schon selbst belegt hat, behält die Taste.
    #[test]
    fn command_search_keys() {
        let of = |custom: &BTreeMap<String, String>, id: &str| -> Vec<String> {
            bindings(custom).into_iter().filter(|(_, a)| *a == id).map(|(c, _)| c.store()).collect()
        };
        assert_eq!(of(&BTreeMap::new(), "command_search"), ["Ctrl+K"]);
        let premiere: BTreeMap<String, String> =
            PRESETS[1].1.iter().map(|(a, k)| (a.to_string(), k.to_string())).collect();
        assert_eq!(of(&premiere, "command_search"), ["Ctrl+Shift+K"]);
        assert_eq!(of(&premiere, "split"), ["Ctrl+K"]);
        // eigene Belegung (z. B. aus dem Premiere-Preset vor 0.13, ohne Eintrag für die Suche)
        for id in ["split", "marker"] {
            let custom = BTreeMap::from([(id.to_string(), "Ctrl+K".to_string())]);
            assert_eq!(of(&custom, id), ["Ctrl+K"], "{id}");
            assert!(of(&custom, "command_search").is_empty(), "{id}");
        }
        // eigene Belegung der Suche selbst gilt
        let custom = BTreeMap::from([("command_search".to_string(), "Ctrl+P".to_string())]);
        assert_eq!(of(&custom, "command_search"), ["Ctrl+P"]);
    }

    #[test]
    fn every_action_has_a_name_and_english_text() {
        let en: std::collections::HashSet<&str> = cl_i18n::ui_entries().into_iter().map(|e| e.0).collect();
        for (id, what, _) in ACTIONS {
            assert_eq!(name(id), *what);
            assert!(en.contains(what.trim()), "{id}: „{what}“ fehlt in ui.en.tsv");
        }
        assert_eq!(name("gibt-es-nicht"), "");
    }

    #[test]
    fn search_ranks_word_starts_first() {
        let items: Vec<(&'static str, Vec<String>)> = [
            ("a", "Exportieren"),
            ("b", "Thumbnail exportieren (aktuelles Bild)"),
            ("c", "Am Playhead teilen"),
            ("d", "Projekt archivieren …"),
            ("e", "Überblendung"),
        ]
        .into_iter()
        .map(|(id, n)| (id, vec![n.to_string()]))
        .collect();
        // Wortanfang vor Wortmitte, sonst Reihenfolge der Liste
        assert_eq!(search("port", &items, &[]), ["a", "b"]);
        assert_eq!(search("EXP", &items, &[]), ["a", "b"]);
        assert_eq!(search("tieren", &items, &[]), ["a", "b"], "Wortmitte zählt auch");
        assert_eq!(search("ren", &items, &[]), ["a", "b", "d"]);
        assert_eq!(search("arch", &items, &[]), ["d"]);
        assert_eq!(search("hiv proj", &items, &[]), ["d"], "mehrere Wörter, beliebige Reihenfolge");
        assert_eq!(search("bild export", &items, &[]), ["b"]);
        assert_eq!(search("teilen x", &items, &[]), Vec::<&str>::new());
        // Umlaute wie geschrieben, Groß/Klein egal
        assert_eq!(search("über", &items, &[]), ["e"]);
        assert_eq!(search("ÜBER", &items, &[]), ["e"]);
        assert!(search("xyz", &items, &[]).is_empty());
        // Wortanfang auch innerhalb des Namens („teilen“) vor Treffern in der Wortmitte
        let more: Vec<(&'static str, Vec<String>)> =
            vec![("m", vec!["Verteilen".into()]), ("s", vec!["Am Playhead teilen".into()])];
        assert_eq!(search("teil", &more, &[]), ["s", "m"]);
        // zweiter Name (deutsch bei englischer Oberfläche) zählt auch
        let en: Vec<(&'static str, Vec<String>)> = vec![("x", vec!["Split at playhead".into(), "Am Playhead teilen".into()])];
        assert_eq!(search("teilen", &en, &[]), ["x"]);
        assert_eq!(search("split", &en, &[]), ["x"]);
    }

    #[test]
    fn empty_search_shows_recent_first() {
        let items: Vec<(&'static str, Vec<String>)> =
            ["a", "b", "c", "d"].into_iter().map(|id| (id, vec![id.to_string()])).collect();
        assert_eq!(search("", &items, &[]), ["a", "b", "c", "d"]);
        assert_eq!(search("  ", &items, &["c", "a"]), ["c", "a", "b", "d"]);
        // Unbekanntes aus dem Verlauf fällt weg
        assert_eq!(search("", &items, &["weg", "d"]), ["d", "a", "b", "c"]);
    }

    #[test]
    fn odd_chords_never_panic() {
        for s in ["", "+", "++", "Ctrl+", "Ctrl++", "Strg+Umschalt+", "Ctrl+Shift+Alt+F12", "🎬", "Ctrl+🎬", "A+B", &"Ctrl+".repeat(1000)] {
            if let Some(c) = Chord::parse(s) {
                let _ = c.label();
                assert_eq!(Chord::parse(&c.store()), Some(c), "{s:?}");
            }
        }
        assert_eq!(Chord::parse("Ctrl++").map(|c| c.key), Some(Key::Plus));
        assert_eq!(Chord::parse(""), None);
    }
}
