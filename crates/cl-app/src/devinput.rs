//! Entwicklerhilfe für Klicktests: Mit `CUTLINE_SHOTS=<Ordner>` liest Cutline Eingaben aus der Datei
//! `<Ordner>/input` (eine Zeile je Schritt) und speist sie als egui-Ereignisse ein – ohne echten
//! Mauszeiger, also auch bei verdecktem Fenster oder ausgeschaltetem Monitor. Ist alles abgespielt,
//! entsteht `<Ordner>/input.done`. Ohne die Variable passiert nichts.
//!
//! Zeilen (Koordinaten = Pixel im Fensterbild, wie in den Bildern aus `CUTLINE_SHOTS`):
//! `move X Y` · `click X Y [mods]` · `rclick X Y` · `dclick X Y` · `drag X1 Y1 X2 Y2 [mods]` ·
//! `wheel X Y DX DY [mods]` · `key NAME [mods]` · `text …` — `mods` z. B. `ctrl+shift`.
//! `drop X Y PFAD[|PFAD…]`: Dateien wie aus dem Explorer über die Stelle ziehen und dort ablegen.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use eframe::egui::{self, pos2, Event, Key, Modifiers, PointerButton, Pos2};

/// Ein Bild lang gültig: Ereignisse und gehaltene Zusatztasten
type Ev = (Vec<Event>, Modifiers);
/// … dazu Dateien, die von außen über dem Fenster schweben bzw. abgelegt werden
type Step = (Vec<Event>, Modifiers, Option<Files>);

#[derive(Clone, Debug, PartialEq)]
enum Files {
    Hover(Vec<PathBuf>, Pos2),
    Drop(Vec<PathBuf>, Pos2),
    Done,
}

static QUEUE: Mutex<VecDeque<Step>> = Mutex::new(VecDeque::new());

fn dir() -> Option<&'static PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("CUTLINE_SHOTS").filter(|d| !d.is_empty()).map(PathBuf::from))
        .as_ref()
}

fn mods(s: Option<&str>) -> Modifiers {
    let mut m = Modifiers::NONE;
    for p in s.unwrap_or("").split('+') {
        match p.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "strg" => {
                m.ctrl = true;
                m.command = true;
            }
            "shift" | "umschalt" => m.shift = true,
            "alt" => m.alt = true,
            _ => {}
        }
    }
    m
}

fn key(name: &str) -> Option<Key> {
    let alias = match name.to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape",
        "enter" | "return" => "Enter",
        "space" => "Space",
        "delete" | "entf" => "Delete",
        "backspace" => "Backspace",
        "tab" => "Tab",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "plus" => "Plus",
        "minus" => "Minus",
        _ => "",
    };
    if !alias.is_empty() {
        return Key::from_name(alias);
    }
    // Buchstaben, Ziffern, F-Tasten: egui kennt sie groß geschrieben
    Key::from_name(name).or_else(|| Key::from_name(&name.to_ascii_uppercase()))
}

/// Eine Zeile in Schritte (je Bild einer) übersetzen. `ppp` = Pixel je egui-Punkt. Unbekanntes: leer.
fn parse(line: &str, ppp: f32) -> Vec<Step> {
    if let Some(rest) = line.trim_start().strip_prefix("drop ") {
        let mut it = rest.trim_start().splitn(3, ' ');
        let (x, y) = (it.next().and_then(|v| v.parse::<f32>().ok()), it.next().and_then(|v| v.parse::<f32>().ok()));
        let files: Vec<PathBuf> = it.next().unwrap_or("").split('|').map(str::trim).filter(|f| !f.is_empty()).map(PathBuf::from).collect();
        let (Some(x), Some(y), false) = (x, y, files.is_empty()) else { return Vec::new() };
        let p = pos2(x / ppp, y / ppp);
        // einige Bilder schweben (Vorschau), dann ablegen, dann ist der Zeiger wieder frei
        let mut v: Vec<Step> = (0..4).map(|_| (Vec::new(), Modifiers::NONE, Some(Files::Hover(files.clone(), p)))).collect();
        v.push((Vec::new(), Modifiers::NONE, Some(Files::Drop(files, p))));
        v.push((Vec::new(), Modifiers::NONE, Some(Files::Done)));
        return v;
    }
    parse_events(line, ppp).into_iter().map(|(e, m)| (e, m, None)).collect()
}

fn parse_events(line: &str, ppp: f32) -> Vec<Ev> {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    if cmd == "text" {
        let t = line.trim_start().strip_prefix("text").unwrap_or("").trim_start().to_string();
        return vec![(vec![Event::Text(t)], Modifiers::NONE)];
    }
    let rest: Vec<&str> = it.collect();
    let num = |i: usize| rest.get(i).and_then(|v| v.parse::<f32>().ok());
    let pt = |i: usize| Some(pos2(num(i)? / ppp, num(i + 1)? / ppp));
    let btn = |pos: Pos2, button, pressed, modifiers| Event::PointerButton { pos, button, pressed, modifiers };
    let press = |p: Pos2, b: PointerButton, m: Modifiers| -> Vec<Ev> {
        vec![(vec![Event::PointerMoved(p)], m), (vec![btn(p, b, true, m)], m), (vec![btn(p, b, false, m)], m)]
    };
    let out = match cmd {
        "move" => pt(0).map(|p| vec![(vec![Event::PointerMoved(p)], Modifiers::NONE)]),
        "click" => pt(0).map(|p| press(p, PointerButton::Primary, mods(rest.get(2).copied()))),
        "rclick" => pt(0).map(|p| press(p, PointerButton::Secondary, mods(rest.get(2).copied()))),
        "dclick" => pt(0).map(|p| {
            let mut v = press(p, PointerButton::Primary, Modifiers::NONE);
            v.extend(press(p, PointerButton::Primary, Modifiers::NONE).into_iter().skip(1));
            v
        }),
        "drag" => pt(0).zip(pt(2)).map(|(a, b)| {
            let m = mods(rest.get(4).copied());
            let mut v = vec![(vec![Event::PointerMoved(a)], m), (vec![btn(a, PointerButton::Primary, true, m)], m)];
            for i in 1..=12 {
                v.push((vec![Event::PointerMoved(a + (b - a) * (i as f32 / 12.0))], m));
            }
            // ein Bild am Ziel stehen, damit Ablege-Ziele den Zeiger sehen
            v.push((vec![Event::PointerMoved(b)], m));
            v.push((vec![btn(b, PointerButton::Primary, false, m)], m));
            v
        }),
        "wheel" => pt(0).zip(num(2).zip(num(3))).map(|(p, (dx, dy))| {
            let m = mods(rest.get(4).copied());
            let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(dx, dy), modifiers: m };
            vec![(vec![Event::PointerMoved(p)], m), (vec![wheel], m)]
        }),
        "key" => rest.first().and_then(|n| key(n)).map(|k| {
            let m = mods(rest.get(1).copied());
            let ev = |pressed| Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: m };
            vec![(vec![ev(true)], m), (vec![ev(false)], m)]
        }),
        _ => None,
    };
    out.unwrap_or_default()
}

/// Aus `App::raw_input_hook`: wartende Schritte einspeisen, einen je Bild.
pub fn feed(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(dir) = dir() else { return };
    // das Testfenster hat oft keinen Fokus: Textfelder würden die Eingabe sonst verlieren
    raw.focused = true;
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let file = dir.join("input");
    if q.is_empty() {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let _ = std::fs::remove_file(&file);
            let ppp = ctx.pixels_per_point();
            q.extend(text.lines().flat_map(|l| parse(l, ppp)));
            if q.is_empty() {
                let _ = std::fs::write(dir.join("input.done"), "");
            }
        }
    }
    if let Some((events, m, files)) = q.pop_front() {
        raw.events.extend(events);
        raw.modifiers = m;
        let mut pos = crate::file_drop::DEV_POS.lock().unwrap_or_else(|e| e.into_inner());
        match files {
            Some(Files::Hover(f, p)) => {
                *pos = Some(p);
                raw.hovered_files = f.into_iter().map(|f| egui::HoveredFile { path: Some(f), ..Default::default() }).collect();
            }
            Some(Files::Drop(f, p)) => {
                *pos = Some(p);
                raw.hovered_files.clear();
                raw.dropped_files = f.into_iter().map(|f| egui::DroppedFile { path: Some(f), ..Default::default() }).collect();
            }
            Some(Files::Done) => *pos = None,
            None => {}
        }
        if q.is_empty() {
            let _ = std::fs::write(dir.join("input.done"), "");
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_become_events_in_points() {
        // Klick: bewegen, drücken, loslassen – Koordinaten durch die Skalierung geteilt
        let c = parse("click 200 100 ctrl+shift", 2.0);
        assert_eq!(c.len(), 3);
        assert!(matches!(c[0].0[0], Event::PointerMoved(p) if p == pos2(100.0, 50.0)));
        assert!(matches!(c[1].0[0], Event::PointerButton { pressed: true, button: PointerButton::Primary, .. }));
        assert!(matches!(c[2].0[0], Event::PointerButton { pressed: false, .. }));
        assert!(c[1].1.ctrl && c[1].1.shift && !c[1].1.alt);
        assert!(matches!(parse("rclick 1 1", 1.0)[1].0[0], Event::PointerButton { button: PointerButton::Secondary, .. }));
        assert_eq!(parse("dclick 5 5", 1.0).len(), 5);
        // Ziehen endet am Ziel und lässt dort los
        let d = parse("drag 0 0 120 60", 1.0);
        assert!(matches!(d.last().unwrap().0[0], Event::PointerButton { pos, pressed: false, .. } if pos == pos2(120.0, 60.0)));
        assert!(d.len() > 10);
        // Tasten mit Aliasnamen und Zusatztasten
        let k = parse("key z strg", 1.0);
        assert!(matches!(k[0].0[0], Event::Key { key: Key::Z, pressed: true, modifiers, .. } if modifiers.ctrl && modifiers.command));
        assert!(matches!(parse("key escape", 1.0)[0].0[0], Event::Key { key: Key::Escape, .. }));
        assert!(matches!(parse("key left", 1.0)[0].0[0], Event::Key { key: Key::ArrowLeft, .. }));
        assert!(matches!(&parse("text  Hallo Welt", 1.0)[0].0[0], Event::Text(t) if t == "Hallo Welt"));
        assert!(matches!(parse("wheel 10 10 0 -120", 1.0)[1].0[0], Event::MouseWheel { delta, .. } if delta.y == -120.0));
        // Dateien ablegen: erst schweben, dann ablegen (Pfade mit Leerzeichen, durch | getrennt)
        let f = parse("drop 200 100 C:/a b/x.mp4|y.wav", 2.0);
        let want = vec![PathBuf::from("C:/a b/x.mp4"), PathBuf::from("y.wav")];
        assert_eq!(f.len(), 6);
        assert_eq!(f[0].2, Some(Files::Hover(want.clone(), pos2(100.0, 50.0))));
        assert_eq!(f[4].2, Some(Files::Drop(want, pos2(100.0, 50.0))));
        assert_eq!(f[5].2, Some(Files::Done));
        // Unbekanntes und Unvollständiges wird übersprungen statt abzustürzen
        for bad in ["", "click", "click a b", "key nix", "tanzen 1 2", "drag 1 2 3", "drop 1 2", "drop a b c.mp4"] {
            assert!(parse(bad, 1.0).is_empty(), "{bad}");
        }
    }
}
