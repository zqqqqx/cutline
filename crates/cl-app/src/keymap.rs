//! Shortcuts: actions with default bindings, own bindings from the settings,
//! presets (Cutline, Premiere-like, Resolve-like) and the settings page for changing them.

use std::collections::BTreeMap;

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::app::App;
use crate::theme;

/// A key combination.
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
            match cl_i18n::key_english(part).to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" => mods |= Modifiers::COMMAND,
                "shift" => mods |= Modifiers::SHIFT,
                "alt" => mods |= Modifiers::ALT,
                _ => key = Key::from_name(part),
            }
        }
        // „Ctrl++“ = plus
        if key.is_none() && s.ends_with("++") {
            key = Some(Key::Plus);
        }
        Some(Self { mods, key: key? })
    }

    /// Display, e.g. „Strg+Umschalt+Z“ (translated by `tr()`)
    pub fn label(&self) -> String {
        let mut v = Vec::new();
        if self.mods.command {
            v.push("Ctrl".to_string());
        }
        if self.mods.alt {
            v.push("Alt".into());
        }
        if self.mods.shift {
            v.push("Shift".into());
        }
        v.push(
            match self.key {
                Key::Space => "Space",
                Key::Delete => "Del",
                Key::Home => "Home",
                Key::End => "End",
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

    /// Storage form, e.g. „Ctrl+Shift+Z“
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

/// (action, description, default bindings)
pub const ACTIONS: &[(&str, &str, &[&str])] = &[
    ("play", "Play / pause", &["Space"]),
    (
        "shuttle_back",
        "Play backward (press again: faster)",
        &["J"],
    ),
    ("shuttle_stop", "Stop~halt", &["K"]),
    (
        "shuttle_fwd",
        "Play forward (press again: faster)",
        &["L"],
    ),
    ("mark_in", "Set In point", &["I"]),
    ("mark_out", "Set Out point", &["O"]),
    ("clear_range", "Clear In/Out", &["Alt+X"]),
    ("loop", "Loop the range", &["Ctrl+L"]),
    ("marker", "Add marker", &["M"]),
    ("marker_next", "Next marker", &["Shift+M"]),
    ("marker_prev", "Previous marker", &["Ctrl+Shift+M"]),
    ("split", "Split at playhead", &["Ctrl+B"]),
    ("del_left", "Delete left of playhead", &["Q"]),
    ("del_right", "Delete right of playhead", &["W"]),
    (
        "ripple_delete",
        "Delete and close gap",
        &["Shift+Delete", "Shift+Backspace"],
    ),
    ("delete", "Delete selection", &["Delete", "Backspace"]),
    ("nudge_left", "Nudge clip 1 frame left", &["Alt+ArrowLeft"]),
    (
        "nudge_right",
        "Nudge clip 1 frame right",
        &["Alt+ArrowRight"],
    ),
    (
        "nudge_left10",
        "Nudge clip 10 frames left",
        &["Alt+Shift+ArrowLeft"],
    ),
    (
        "nudge_right10",
        "Nudge clip 10 frames right",
        &["Alt+Shift+ArrowRight"],
    ),
    // mostly arrive as a copy/paste event, not as a key (see `App::shortcuts`)
    ("copy", "Copy", &["Ctrl+C"]),
    ("cut", "Cut", &["Ctrl+X"]),
    ("paste", "Paste", &["Ctrl+V"]),
    ("paste_attrs", "Paste attributes", &["Ctrl+Alt+V"]),
    ("group", "Group", &["Ctrl+G"]),
    ("ungroup", "Ungroup", &["Ctrl+Shift+G"]),
    ("duplicate", "Duplicate", &["Ctrl+D"]),
    ("select_all", "Select all clips", &["Ctrl+A"]),
    ("disable", "Disable clip", &["V"]),
    ("tool_select", "Tool: select", &["A"]),
    ("tool_blade", "Tool: blade", &["B"]),
    ("undo", "Undo", &["Ctrl+Z"]),
    ("redo", "Redo", &["Ctrl+Shift+Z", "Ctrl+Y"]),
    ("prev", "Previous frame", &["ArrowLeft"]),
    ("next", "Next frame", &["ArrowRight"]),
    ("prev_edit", "Previous edit", &["ArrowUp"]),
    ("next_edit", "Next edit", &["ArrowDown"]),
    ("home", "Go to start", &["Home"]),
    ("end", "Go to end", &["End"]),
    ("magnet", "Main track magnet", &["P"]),
    ("snap", "Snapping", &["N"]),
    ("voiceover", "Voiceover", &["R"]),
    ("max", "Large player", &["F"]),
    // Playback volume (no default key: M is „Add marker“)
    ("playback_mute", "Mute playback", &[]),
    ("playback_louder", "Playback louder", &[]),
    ("playback_quieter", "Playback quieter", &[]),
    ("import", "Import~verb", &["Ctrl+I"]),
    ("save", "Save", &["Ctrl+S"]),
    ("save_as", "Save as", &["Ctrl+Shift+S"]),
    ("open", "Open", &["Ctrl+O"]),
    ("export", "Export", &["Ctrl+E"]),
    ("settings", "Settings", &["Ctrl+Comma"]),
    ("command_search", "Command search", &["Ctrl+K"]),
    (
        "zoom_in",
        "Larger interface",
        &["Ctrl+Plus", "Ctrl+Equals"],
    ),
    ("zoom_out", "Smaller interface", &["Ctrl+Minus"]),
    ("zoom_reset", "Interface 100 %", &["Ctrl+0"]),
    ("escape", "Clear selection", &["Escape"]),
    // Menu top left (names as in the menu)
    ("new_project", "New project", &[]),
    ("import_project", "Import project from another app …", &[]),
    ("import_fonts", "Import font …", &[]),
    ("exports", "Exports", &[]),
    ("history", "Edit history", &[]),
    ("layout_edit", "Edit layout …", &[]),
    ("template_save", "Save as template …", &[]),
    ("template_insert", "Insert from template …", &[]),
    ("archive", "Archive project …", &[]),
    ("relink", "Relink media …", &[]),
    ("backups", "Backups …", &[]),
    ("go_home", "Projects~overview", &[]),
    ("tour", "Tour", &[]),
    ("onboarding", "Repeat introduction", &[]),
    // Quick actions (menu in the timeline)
    ("quick_pauses", "Remove pauses (jump cuts)", &[]),
    ("quick_fillers", "Remove filler words (uh, um)", &[]),
    ("quick_ducking", "Lower music under speech (auto-ducking)", &[]),
    ("quick_highlights", "Find highlights (loud moments → markers)", &[]),
    ("quick_sync", "Sync selection by audio", &[]),
    ("quick_multicam", "Start multicam edit", &[]),
    ("quick_speakers", "Import speaker recordings …", &[]),
    ("quick_punch", "Zoom punch-in at the playhead", &[]),
    ("quick_vertical", "Vertical layout (9:16, facecam on top)", &[]),
    // „Layout“ submenu (`arrange.rs`); in the menu without „Layout: “
    ("quick_layout_side", "Layout: Side by side", &[]),
    ("quick_layout_stack", "Layout: Stacked", &[]),
    ("quick_layout_big_left", "Layout: One large, two on the right", &[]),
    ("quick_layout_columns", "Layout: Three columns", &[]),
    ("quick_layout_grid", "Layout: Grid 2×2", &[]),
    ("quick_layout_pip_tl", "Layout: Picture in picture – top left", &[]),
    ("quick_layout_pip_tr", "Layout: Picture in picture – top right", &[]),
    ("quick_layout_pip_bl", "Layout: Picture in picture – bottom left", &[]),
    ("quick_layout_pip_br", "Layout: Picture in picture – bottom right", &[]),
    ("quick_layout_reset", "Reset layout", &[]),
    ("quick_thumbnail", "Thumbnail …", &[]),
    ("quick_recap", "Create recap from markers", &[]),
    ("censor", "Censor …", &[]),
    // Brand Kit (Settings → Brand Kit)
    ("brand_logo", "Insert logo", &[]),
];

/// Name of an action (German, `tr()` translates), empty for unknown ones
pub fn name(id: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.0 == id).map_or("", |a| a.1)
}

/// Icon of an action (as in menus and toolbar), empty without one
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
        "playback_mute" => i::SPEAKER_SLASH,
        "playback_louder" => i::SPEAKER_HIGH,
        "playback_quieter" => i::SPEAKER_LOW,
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

/// Command search: order of the hits. `items` = (action, names to search in).
/// Each word of the input must occur in a name (case-insensitive); actions whose words
/// all stand at a word start come first, otherwise the list order stays.
/// Empty input: recently used (`recent`) first, all others below.
pub fn search(query: &str, items: &[(&'static str, Vec<String>)], recent: &[&str]) -> Vec<&'static str> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        let mut out: Vec<&'static str> = recent.iter().filter_map(|r| items.iter().find(|i| i.0 == *r).map(|i| i.0)).collect();
        let rest: Vec<&'static str> = items.iter().map(|i| i.0).filter(|id| !out.contains(id)).collect();
        out.extend(rest);
        return out;
    }
    // 0 = word start, 1 = mid-word, None = missing
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

/// Presets: (name, differences from the default bindings)
pub const PRESETS: &[(&str, &[(&str, &str)])] = &[
    ("Cutline", &[]),
    (
        "Premiere-like",
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
        "Resolve-like",
        &[
            ("split", "Ctrl+Backslash"),
            ("disable", "D"),
            ("snap", "N"),
            ("magnet", "Ctrl+Shift+P"),
        ],
    ),
];

/// Effective bindings: own entries replace the action's default bindings. A default key
/// already used by an own binding is dropped (a new action takes nobody's key).
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
    // Check combinations with more modifiers first (Ctrl+Shift+Z before Ctrl+Z)
    v.sort_by_key(|(c, _)| {
        std::cmp::Reverse(c.mods.command as u8 + c.mods.shift as u8 + c.mods.alt as u8)
    });
    v
}

/// Tooltip "Text  (shortcut)"; without a bound key only the text. The dictionary translates both
/// parts separately, `theme::tip_with_keys` shows the shortcut as a keycap.
pub fn with_key(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_string()
    } else {
        format!("{text}  ({key})")
    }
}

impl App {

    /// „Keyboard shortcuts“ settings page: change a binding (click, then press keys).
    pub fn shortcuts_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr("Preset")).color(theme::TEXT_DIM()));
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
        // Recording a new binding
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
                        // Duplicate binding: remove it from the other action
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
                    // Binding as keycaps; the whole area is the button
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
                                            ui.label(RichText::new(crate::tr("Press a key …")).color(theme::ACCENT_INK()));
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
                            "Restore default",
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
        ui.label(RichText::new(crate::tr("Also: Ctrl+C / X / V copy, cut, paste · Ctrl+wheel zooms the timeline · Shift+wheel scrolls horizontally · Alt+Shift+Z fits the player")).small().color(theme::TEXT_DIM()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_with_and_without_key() {
        assert_eq!(with_key("Split", "Ctrl+K"), "Split  (Ctrl+K)");
        assert_eq!(with_key("Split", ""), "Split");
        // this is how the tooltip display splits it back into text and keycap
        assert_eq!(with_key("Split", "Q").rsplit_once("  ("), Some(("Split", "Q)")));
    }


    #[test]
    fn parse_store_and_override() {
        let c = Chord::parse("Ctrl+Shift+Z").unwrap();
        assert!(c.mods.command && c.mods.shift && c.key == Key::Z);
        assert_eq!(Chord::parse(&c.store()), Some(c));
        assert_eq!(c.label(), "Ctrl+Shift+Z");
        let mut custom = BTreeMap::new();
        custom.insert("split".to_string(), "Ctrl+K".to_string());
        custom.insert("disable".to_string(), String::new());
        let b = bindings(&custom);
        assert!(b.iter().any(|(c, a)| *a == "split" && c.key == Key::K));
        assert!(!b.iter().any(|(c, a)| *a == "split" && c.key == Key::B));
        assert!(!b.iter().any(|(_, a)| *a == "disable"));
        // Ctrl+Shift+Z comes before Ctrl+Z
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

    /// Default and every preset: no key for two actions, only known actions,
    /// every binding readable and storable without loss.
    #[test]
    fn presets_have_no_conflicts() {
        let ids: Vec<&str> = ACTIONS.iter().map(|a| a.0).collect();
        let mut seen = std::collections::HashSet::new();
        assert!(ids.iter().all(|i| seen.insert(*i)), "duplicate action id");
        for (name, diff) in PRESETS {
            let custom: BTreeMap<String, String> = diff.iter().map(|(a, k)| (a.to_string(), k.to_string())).collect();
            for (a, k) in *diff {
                assert!(ids.contains(a), "{name}: unknown action {a}");
                let c = Chord::parse(k).unwrap_or_else(|| panic!("{name}: {k}"));
                assert_eq!(Chord::parse(&c.store()), Some(c), "{name}: {k}");
            }
            let b = bindings(&custom);
            for (i, (c, a)) in b.iter().enumerate() {
                for (c2, a2) in &b[i + 1..] {
                    assert!(!(c == c2 && a != a2), "{name}: {} for {a} and {a2}", c.store());
                }
            }
        }
    }

    /// Command search: default Ctrl+K, in the Premiere preset Ctrl+Shift+K (Ctrl+K splits there);
    /// whoever already bound Ctrl+K themselves keeps the key.
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
        // own binding (e.g. from the Premiere preset before 0.13, without an entry for the search)
        for id in ["split", "marker"] {
            let custom = BTreeMap::from([(id.to_string(), "Ctrl+K".to_string())]);
            assert_eq!(of(&custom, id), ["Ctrl+K"], "{id}");
            assert!(of(&custom, "command_search").is_empty(), "{id}");
        }
        // an own binding of the search itself applies
        let custom = BTreeMap::from([("command_search".to_string(), "Ctrl+P".to_string())]);
        assert_eq!(of(&custom, "command_search"), ["Ctrl+P"]);
    }

    #[test]
    fn every_action_has_a_name_and_a_german_text() {
        let en: std::collections::HashSet<&str> = cl_i18n::ui_entries().into_iter().map(|e| e.0).collect();
        for (id, what, _) in ACTIONS {
            assert_eq!(name(id), *what);
            assert!(en.contains(what.trim()), "{id}: „{what}“ missing in translations/de_DE/ui.tsv");
        }
        assert_eq!(name("does-not-exist"), "");
    }

    #[test]
    fn search_ranks_word_starts_first() {
        let items: Vec<(&'static str, Vec<String>)> = [
            ("a", "Export"),
            ("b", "Export thumbnail (current frame)"),
            ("c", "Split at playhead"),
            ("d", "Archive project …"),
            ("e", "Überblendung"),
        ]
        .into_iter()
        .map(|(id, n)| (id, vec![n.to_string()]))
        .collect();
        // word start before mid-word, otherwise list order
        assert_eq!(search("port", &items, &[]), ["a", "b"]);
        assert_eq!(search("EXP", &items, &[]), ["a", "b"]);
        assert_eq!(search("xpo", &items, &[]), ["a", "b"], "mid-word counts too");
        assert_eq!(search("o", &items, &[]), ["a", "b", "d"]);
        assert_eq!(search("arch", &items, &[]), ["d"]);
        assert_eq!(search("hiv proj", &items, &[]), ["d"], "several words, any order");
        assert_eq!(search("frame export", &items, &[]), ["b"]);
        assert_eq!(search("split x", &items, &[]), Vec::<&str>::new());
        // Umlauts as written, case-insensitive
        assert_eq!(search("über", &items, &[]), ["e"]);
        assert_eq!(search("ÜBER", &items, &[]), ["e"]);
        assert!(search("xyz", &items, &[]).is_empty());
        // Word start also inside the name („teilen“) before mid-word hits
        let more: Vec<(&'static str, Vec<String>)> =
            vec![("m", vec!["Verteilen".into()]), ("s", vec!["Am Abspielkopf teilen".into()])];
        assert_eq!(search("teil", &more, &[]), ["s", "m"]);
        // the name in the other language counts too
        let en: Vec<(&'static str, Vec<String>)> = vec![("x", vec!["Split at playhead".into(), "Am Abspielkopf teilen".into()])];
        assert_eq!(search("teilen", &en, &[]), ["x"]);
        assert_eq!(search("split", &en, &[]), ["x"]);
    }

    #[test]
    fn empty_search_shows_recent_first() {
        let items: Vec<(&'static str, Vec<String>)> =
            ["a", "b", "c", "d"].into_iter().map(|id| (id, vec![id.to_string()])).collect();
        assert_eq!(search("", &items, &[]), ["a", "b", "c", "d"]);
        assert_eq!(search("  ", &items, &["c", "a"]), ["c", "a", "b", "d"]);
        // Unknown entries from the history are dropped
        assert_eq!(search("", &items, &["weg", "d"]), ["d", "a", "b", "c"]);
    }

    #[test]
    fn odd_chords_never_panic() {
        for s in ["", "+", "++", "Ctrl+", "Ctrl++", "Ctrl+Shift+", "Ctrl+Shift+Alt+F12", "🎬", "Ctrl+🎬", "A+B", &"Ctrl+".repeat(1000)] {
            if let Some(c) = Chord::parse(s) {
                let _ = c.label();
                assert_eq!(Chord::parse(&c.store()), Some(c), "{s:?}");
            }
        }
        assert_eq!(Chord::parse("Ctrl++").map(|c| c.key), Some(Key::Plus));
        assert_eq!(Chord::parse(""), None);
    }
}
