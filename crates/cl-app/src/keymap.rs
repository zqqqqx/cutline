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
    ("paste_attrs", "Attribute einfügen", &["Ctrl+Alt+V"]),
    ("group", "Gruppieren", &["Ctrl+G"]),
    ("ungroup", "Gruppe auflösen", &["Ctrl+Shift+G"]),
    ("duplicate", "Duplizieren", &["Ctrl+D"]),
    ("select_all", "Alle Clips auswählen", &["Ctrl+A"]),
    ("disable", "Clip deaktivieren", &["V"]),
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
    (
        "zoom_in",
        "Oberfläche größer",
        &["Ctrl+Plus", "Ctrl+Equals"],
    ),
    ("zoom_out", "Oberfläche kleiner", &["Ctrl+Minus"]),
    ("zoom_reset", "Oberfläche 100 %", &["Ctrl+0"]),
    ("escape", "Auswahl aufheben", &["Escape"]),
];

/// Vorlagen: (Name, Abweichungen von der Standardbelegung)
pub const PRESETS: &[(&str, &[(&str, &str)])] = &[
    ("Cutline", &[]),
    (
        "Premiere-ähnlich",
        &[
            ("split", "Ctrl+K"),
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

/// Wirksame Belegung: eigene Einträge ersetzen die Standardbelegung der Aktion.
pub fn bindings(custom: &BTreeMap<String, String>) -> Vec<(Chord, &'static str)> {
    let mut v: Vec<(Chord, &'static str)> = Vec::new();
    for (id, _, defaults) in ACTIONS {
        match custom.get(*id) {
            Some(c) if c.is_empty() => {}
            Some(c) => v.extend(Chord::parse(c).map(|ch| (ch, *id))),
            None => v.extend(
                defaults
                    .iter()
                    .filter_map(|d| Chord::parse(d))
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

impl App {
    /// Einstellungsseite „Tastenkürzel“: Belegung ändern (Klick, dann Tasten drücken).
    pub fn shortcuts_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr("Vorlage")).color(theme::TEXT_DIM));
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
        ui.label(RichText::new(crate::tr("Klicke auf eine Belegung und drücke die neue Taste. Entf entfernt die Belegung, Esc bricht ab.")).small().color(theme::TEXT_FAINT));
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
                    ui.label(RichText::new(crate::tr(*what)).color(theme::TEXT_DIM));
                    let keys: Vec<String> = all
                        .iter()
                        .filter(|(_, a)| a == id)
                        .map(|(c, _)| c.label())
                        .collect();
                    let capturing = self.capture_key == Some(*id);
                    let text = if capturing {
                        "Taste drücken …".to_string()
                    } else if keys.is_empty() {
                        "—".to_string()
                    } else {
                        keys.join(" / ")
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(crate::tr(text)).monospace().color(
                                if capturing {
                                    theme::ACCENT_INK
                                } else {
                                    theme::ACCENT
                                },
                            ))
                            .fill(if capturing {
                                theme::ACCENT
                            } else {
                                theme::BG_RAISED
                            })
                            .min_size(egui::vec2(150.0, 22.0)),
                        )
                        .clicked()
                    {
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
        ui.label(RichText::new(crate::tr("Außerdem: Strg+C / X / V kopieren, ausschneiden, einfügen · Strg+Mausrad zoomt die Zeitleiste · Umschalt+Mausrad scrollt waagerecht · Alt+Umschalt+Z passt den Player ein")).small().color(theme::TEXT_FAINT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
