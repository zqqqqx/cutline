//! Proxy in Stücken: Die Quelle wird in gleich lange Stücke ([`PART_SECS`]) zerlegt gebaut, jedes
//! fertige Stück ist sofort für Einzelbilder und Ströme nutzbar. Ablage je Proxy in einem Ordner
//! neben der fertigen Datei (`<Proxy-Name>.teile/`): `0007.mp4` (fertig), `0007.part.mp4`
//! (entsteht; erst fertig schreiben, dann umbenennen), `0003-0012.ffconcat` (Liste für einen Strom
//! über die Stücke 3…11) und `quelle.txt` (Pfad der Quelle, zum Aufräumen nach einer Änderung).
//! Sind alle Stücke da, werden sie verlustfrei zur gewohnten Ein-Datei-Proxy zusammengefügt.
//!
//! Bildgenau: Jedes Stück rechnet auf demselben Bildraster wie ein Bau in einem Zug (`-copyts`,
//! `fps=…:start_time=<Start der Quelle + Stückanfang>`, genau `per` Bilder je Stück). Ein Stück
//! ohne ein einziges Bild (Video endet vor der Dateilänge) ist eine leere Datei.

use std::ops::Range;
use std::path::{Path, PathBuf};

use cl_core::Rational;

/// Stücklänge: gemessen (1080p60 → 540p30, NVENC, Rechner ausgelastet) braucht das erste Stück
/// bei 10 s 1,3 s (5 s: 0,9 s; 20 s: 2,4 s), der ganze Bau 10,2 s je Minute Quelle (5 s: 13,9 s;
/// 20 s: 8,3 s; in einem Zug 9,5 s) – 10 s hält das Ziel „erstes Stück < 2 s“ bei wenig Aufwand je Start.
pub const PART_SECS: f64 = 10.0;
/// So oft darf ein Stück scheitern, dann wird es übersprungen (dort bleibt das Original)
pub const MAX_FAILS: u8 = 2;
/// Strom aus Stücken: so lange (Quellsekunden) vor dem Ende der vorhandenen Stücke wird auf das
/// Original (bzw. weitere Stücke) umgeschaltet – der Ersatzstrom muss vorher anlaufen …
pub const SWITCH_MARGIN: f64 = 3.0;
/// … und vom Original auf Stücke erst, wenn so viel am Stück vorliegt (kein Hin und Her)
pub const SWITCH_AHEAD: f64 = 6.0;

/// Einteilung einer Quelle in Stücke: Bildrate des Proxys, Bilder je Stück, Anzahl.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parting {
    pub fps: Rational,
    pub per: u32,
    pub count: u32,
}

impl Parting {
    /// `None`: Dauer oder Bildrate unbekannt (dann wird in einem Zug gebaut)
    pub fn new(dur: f64, fps: Rational, part_secs: f64) -> Option<Self> {
        let f = fps.as_f64();
        if !(dur.is_finite() && dur > 0.0 && fps.num > 0 && fps.den > 0 && f > 0.0 && part_secs > 0.0) {
            return None;
        }
        let per = ((part_secs * f).round() as u32).max(1);
        let frames = ((dur * f).round() as u64).max(1);
        Some(Self { fps, per, count: frames.div_ceil(per as u64) as u32 })
    }

    fn secs(&self, frames: u64) -> f64 {
        frames as f64 * self.fps.den as f64 / self.fps.num as f64
    }

    /// Anfang von Stück `k` (Quellsekunden ab Dateianfang)
    pub fn start(&self, k: u32) -> f64 {
        self.secs(k as u64 * self.per as u64)
    }

    /// Bildnummer des Proxys bei `t` (abgerundet)
    fn frame_at(&self, t: f64) -> u64 {
        (t.max(0.0) * self.fps.num as f64 / self.fps.den as f64 + 1e-6).floor() as u64
    }

    /// Stück, in dem `t` liegt (am Ende: das letzte)
    pub fn part_of(&self, t: f64) -> u32 {
        ((self.frame_at(t) / self.per as u64) as u32).min(self.count.saturating_sub(1))
    }

    /// Stücke, die den Quellbereich `[a, b)` berühren
    pub fn parts_in(&self, a: f64, b: f64) -> Range<u32> {
        if b <= a || self.count == 0 {
            return 0..0;
        }
        self.part_of(a)..self.part_of(b - 1e-6) + 1
    }
}

/// Dateiname eines fertigen Stücks im Stück-Ordner
pub fn part_name(k: u32) -> String {
    format!("{k:04}.mp4")
}

/// Stück-Ordner eines Proxys (`…/abc-540-30_1fps.mp4` → `…/abc-540-30_1fps.teile`)
pub fn parts_dir(out: &Path) -> PathBuf {
    out.with_extension("teile")
}

/// Fertige Stücke im Ordner (je Stück: Datei vorhanden; leere Datei = Stück ohne Bild)
pub fn scan_ready(dir: &Path, count: u32) -> Vec<bool> {
    (0..count).map(|k| dir.join(part_name(k)).is_file()).collect()
}

/// Reste vom letzten Lauf: unfertige Dateien (`.part.mp4`, auch nach hartem Beenden), Strom-Listen
/// und Stück-Ordner, deren Proxy schon zusammengefügt ist. Fertige Stücke bleiben.
pub fn cleanup(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".part.mp4") {
            let _ = std::fs::remove_file(&p);
        } else if p.is_dir() && name.ends_with(".teile") {
            if p.with_extension("mp4").is_file() {
                let _ = std::fs::remove_dir_all(&p);
                continue;
            }
            for f in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if n.ends_with(".part.mp4") || n.ends_with(".ffconcat") || n.ends_with(".tmp") {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
    }
}

/// Stück-Ordner derselben Quelle aus einer älteren Fassung der Datei (anderer Hash) entfernen;
/// Ordner derselben Fassung in anderer Höhe/Bildrate bleiben (Einstellung kann zurückwechseln).
pub fn remove_stale(dir: &Path, src: &Path, keep: &Path) {
    let want = src.to_string_lossy();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p == keep || !p.is_dir() || !e.file_name().to_string_lossy().ends_with(".teile") {
            continue;
        }
        let same_src = std::fs::read_to_string(p.join("quelle.txt")).is_ok_and(|s| s.trim() == want);
        // derselbe Hash-Teil des Namens = dieselbe Dateifassung (nur andere Höhe/Bildrate): bleibt
        let same_hash = |a: &Path, b: &Path| {
            let h = |p: &Path| p.file_name().map(|n| n.to_string_lossy().chars().take(16).collect::<String>());
            h(a) == h(b)
        };
        if same_src && !same_hash(&p, keep) {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

/// Liste für den concat-Leser über die Stücke `first..end` (relative Namen: im selben Ordner,
/// sicher ohne `-safe 0`). Leere Stücke (ohne Bild) fehlen. Wird nur einmal geschrieben.
pub fn concat_list(dir: &Path, first: u32, end: u32) -> std::io::Result<PathBuf> {
    let path = dir.join(format!("{first:04}-{end:04}.ffconcat"));
    if path.is_file() {
        return Ok(path);
    }
    let mut s = String::from("ffconcat version 1.0\n");
    let mut any = false;
    for k in first..end {
        let f = dir.join(part_name(k));
        if std::fs::metadata(&f).map(|m| m.len() > 0)? {
            s.push_str(&format!("file {}\n", part_name(k)));
            any = true;
        }
    }
    if !any {
        return Err(std::io::Error::other("keine Bilder"));
    }
    // erst fertig schreiben, dann umbenennen (ein zweiter Schreiber schreibt dasselbe)
    let tmp = dir.join(format!("{first:04}-{end:04}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, s)?;
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    if path.is_file() {
        Ok(path)
    } else {
        Err(std::io::Error::other("Liste nicht geschrieben"))
    }
}

/// Einzelbild an Quellzeit `t` aus einem Stück: FFmpeg liefert das erste Bild ab `-ss`, also das
/// Proxybild `ceil(t·fps)`; Ergebnis (Stück, Zeit im Stück – ein halbes Bild davor, damit Rundung
/// das Bild nicht verwirft). `None` hinter dem letzten Bild.
pub fn still_part(pt: &Parting, t: f64) -> Option<(u32, f64)> {
    let j = (t.max(0.0) * pt.fps.num as f64 / pt.fps.den as f64 - 1e-6).ceil().max(0.0) as u64;
    let k = (j / pt.per as u64) as u32;
    if k >= pt.count {
        return None;
    }
    let rel = pt.secs(j) - pt.start(k) - pt.secs(1) / 2.0;
    Some((k, rel.max(0.0)))
}

/// Woher ein Strom kommt (Teil des Stromschlüssels: ein Wechsel öffnet über den Ersatzstrom-Weg
/// des Player-Kerns einen neuen Strom, der erst übernimmt, wenn er nicht hinter dem alten liegt).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    /// wie bisher: fertiger Proxy, sonst Original (Medien ohne Stück-Bau)
    Whole,
    /// Original (Stücke fehlen), gewählt an Quellzeit `since_ms`
    Original { since_ms: i64 },
    /// fertiger, zusammengefügter Proxy
    Full,
    /// Stücke `first..end`
    Parts { first: u32, end: u32 },
}

impl Pick {
    /// Anteil am Stromschlüssel: „wie bisher“ und Original öffnen dieselbe Datei (0 = Schlüssel unverändert)
    pub fn tag(self) -> u64 {
        match self {
            Pick::Whole | Pick::Original { .. } => 0,
            Pick::Full => 0x6a09_e667_f3bc_c909,
            Pick::Parts { first, end } => ((first as u64) << 32 | end as u64 | 1 << 63).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
        }
    }
}

/// Quelle eines Stroms ab Quellzeit `p` (Ende des Clip-Ausschnitts `clip_end`), mit der bisherigen
/// Wahl `prev` derselben Ebene. Regeln (kein Hin und Her):
/// - Stücke laufen weiter, solange mehr als `margin` vor ihrem Ende liegt oder sie bis zum Clipende reichen;
/// - kurz vor dem Ende: weiter mit den inzwischen fertigen Stücken, sonst Original;
/// - vom Original auf Stücke frühestens [`SWITCH_AHEAD`] nach dem Wechsel aufs Original und nur,
///   wenn mindestens so viel (bzw. 2 × `margin`) am Stück vorliegt;
/// - neuer Strom: Stücke, wenn sie über `margin` hinaus reichen.
pub fn choose_stream(prev: Option<Pick>, ready: &[bool], pt: &Parting, p: f64, clip_end: f64, margin: f64) -> Pick {
    let k = pt.part_of(p);
    let is_ready = |i: u32| ready.get(i as usize).copied().unwrap_or(false);
    let run_end = (k..pt.count).take_while(|i| is_ready(*i)).last().map(|i| i + 1);
    let covers = |end: u32| end >= pt.count || pt.start(end) >= clip_end - 1e-6;
    let left = |end: u32| pt.start(end) - p;
    let original = Pick::Original { since_ms: (p * 1000.0).round() as i64 };
    let fresh = |need: f64| match run_end {
        Some(e) if covers(e) || left(e) >= need => Pick::Parts { first: k, end: e },
        _ => original,
    };
    match prev {
        Some(Pick::Parts { first, end }) if pt.start(first) <= p + 1e-9 && p < pt.start(end) => {
            if covers(end) || left(end) > margin {
                Pick::Parts { first, end }
            } else {
                match run_end {
                    Some(e) if e > end && (covers(e) || left(e) > margin) => Pick::Parts { first: k, end: e },
                    _ => original,
                }
            }
        }
        Some(Pick::Original { since_ms }) => {
            let since = since_ms as f64 / 1000.0;
            if p + 1e-3 < since {
                fresh(margin) // zurückgesprungen: neu wählen
            } else if p - since >= SWITCH_AHEAD {
                match fresh(SWITCH_AHEAD.max(2.0 * margin)) {
                    Pick::Parts { first, end } => Pick::Parts { first, end },
                    _ => Pick::Original { since_ms },
                }
            } else {
                Pick::Original { since_ms }
            }
        }
        _ => fresh(margin),
    }
}

/// Wo die Arbeit hin soll (Quellsekunden): Playhead-Stelle mit Ende des Clip-Ausschnitts (nur
/// vorwärts abgespielte Clips), sichtbare und im Projekt benutzte Bereiche.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    pub at: Option<(f64, f64)>,
    pub visible: Vec<(f64, f64)>,
    pub used: Vec<(f64, f64)>,
}

/// Nächstes Stück: das am Playhead, dann vorwärts bis zum Ende des Clip-Ausschnitts, dann
/// sichtbare, dann im Projekt benutzte Bereiche, dann der Rest (ab dem Playhead). Wird nach jedem
/// Stück neu gefragt – springt der Playhead, gilt die neue Stelle, das laufende Stück wird fertig.
pub fn next_part(pt: &Parting, skip: impl Fn(u32) -> bool, f: &Focus) -> Option<u32> {
    let free = |r: Range<u32>| r.into_iter().find(|i| *i < pt.count && !skip(*i));
    if let Some((t, end)) = f.at {
        let k = pt.part_of(t);
        if let Some(i) = free(k..pt.parts_in(t, end).end.max(k + 1)) {
            return Some(i);
        }
    }
    for (a, b) in f.visible.iter().chain(&f.used) {
        if let Some(i) = free(pt.parts_in(*a, *b)) {
            return Some(i);
        }
    }
    let k = f.at.map_or(0, |(t, _)| pt.part_of(t));
    free(k..pt.count).or_else(|| free(0..k))
}

/// Fertige Bereiche (Quellsekunden), benachbarte zusammengefasst
pub fn ready_ranges(pt: &Parting, ready: &[bool], dur: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for k in (0..pt.count).filter(|k| ready.get(*k as usize).copied().unwrap_or(false)) {
        let (a, b) = (pt.start(k), pt.start(k + 1).min(dur.max(pt.start(k))));
        match out.last_mut() {
            Some(l) if (l.1 - a).abs() < 1e-9 => l.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// Anteile der Clipbreite (0…1), deren Quelle schon als Stück vorliegt. Der Clip zeigt die Quelle
/// `[src_in, src_in + span)` (Tempo ist in `span` enthalten), rückwärts von rechts nach links.
/// shortcut: Tempo-Kurven werden linear umgerechnet, genauer erst, wenn sie häufig vorkommen
pub fn clip_coverage(ranges: &[(f64, f64)], src_in: f64, span: f64, reverse: bool) -> Vec<(f32, f32)> {
    if span <= 0.0 {
        return Vec::new();
    }
    ranges
        .iter()
        .filter_map(|(a, b)| {
            let (a, b) = (((a - src_in) / span).clamp(0.0, 1.0), ((b - src_in) / span).clamp(0.0, 1.0));
            (b > a).then(|| if reverse { (1.0 - b, 1.0 - a) } else { (a, b) })
        })
        .map(|(a, b)| (a as f32, b as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(dur: f64) -> Parting {
        Parting::new(dur, Rational::new(30, 1), 10.0).unwrap()
    }

    #[test]
    fn parting_covers_source_with_short_last_part() {
        let p = pt(25.0);
        assert_eq!((p.per, p.count), (300, 3));
        assert_eq!((p.start(0), p.start(2), p.start(3)), (0.0, 20.0, 30.0));
        assert_eq!((p.part_of(0.0), p.part_of(9.99), p.part_of(10.0), p.part_of(24.9), p.part_of(99.0)), (0, 0, 1, 2, 2));
        // genau aufgeteilt: kein leeres letztes Stück
        assert_eq!(pt(20.0).count, 2);
        // ein halbes Bild über dem Raster zählt nicht als eigenes Stück
        assert_eq!(pt(20.0 + 0.4 / 30.0).count, 2);
        assert_eq!(pt(20.0 + 1.0 / 30.0).count, 3);
        // NTSC: Stückgrenzen auf dem Bildraster
        let n = Parting::new(60.0, Rational::new(30000, 1001), 10.0).unwrap();
        assert_eq!(n.per, 300);
        assert!((n.start(1) - 10.01).abs() < 1e-9);
        // sehr kurz: ein Stück
        assert_eq!(pt(0.01).count, 1);
        // unbekannt/kaputt: keine Einteilung
        assert_eq!(Parting::new(0.0, Rational::new(30, 1), 10.0), None);
        assert_eq!(Parting::new(f64::NAN, Rational::new(30, 1), 10.0), None);
        assert_eq!(Parting::new(10.0, Rational::new(0, 1), 10.0), None);
        assert_eq!(Parting::new(10.0, Rational::new(30, 0), 10.0), None);
        // Bereiche
        assert_eq!(p.parts_in(5.0, 15.0), 0..2);
        assert_eq!(p.parts_in(10.0, 20.0), 1..2);
        assert_eq!(p.parts_in(3.0, 3.0), 0..0);
    }

    #[test]
    fn still_comes_from_the_part_ffmpeg_would_pick() {
        let p = pt(25.0);
        // mitten im Stück: Bild 30 (1,0 s) → Stück 0, ein halbes Bild davor
        let (k, rel) = still_part(&p, 1.0).unwrap();
        assert_eq!(k, 0);
        assert!((rel - (1.0 - 1.0 / 60.0)).abs() < 1e-9);
        // knapp vor der Naht: das erste Bild ab t ist das erste des nächsten Stücks
        let (k, rel) = still_part(&p, 9.99).unwrap();
        assert_eq!((k, rel), (1, 0.0));
        // hinter dem letzten Stück: keins
        assert_eq!(still_part(&p, 30.0), None);
    }

    #[test]
    fn order_playhead_then_forward_then_visible_then_rest() {
        let p = pt(100.0); // 10 Stücke
        let none = |_| false;
        let f = |at: Option<(f64, f64)>, vis: &[(f64, f64)], used: &[(f64, f64)]| Focus { at, visible: vis.to_vec(), used: used.to_vec() };
        // Playhead bei 42 s, Clip bis 65 s → 4, dann 5, 6
        let focus = f(Some((42.0, 65.0)), &[(80.0, 90.0)], &[(10.0, 20.0)]);
        assert_eq!(next_part(&p, none, &focus), Some(4));
        assert_eq!(next_part(&p, |i| i <= 4, &focus), Some(5));
        assert_eq!(next_part(&p, |i| (4..=6).contains(&i), &focus), Some(8), "dann sichtbar");
        assert_eq!(next_part(&p, |i| (4..=6).contains(&i) || i == 8, &focus), Some(1), "dann benutzt");
        // dann der Rest ab dem Playhead, zuletzt der Anfang
        let done = |i| [1, 4, 5, 6, 8].contains(&i);
        assert_eq!(next_part(&p, done, &focus), Some(7));
        assert_eq!(next_part(&p, |i| done(i) || i >= 7, &focus), Some(0));
        assert_eq!(next_part(&p, |_| true, &focus), None);
        // Sprung: neu geplant ab der neuen Stelle
        assert_eq!(next_part(&p, |i| i == 4, &f(Some((95.0, 100.0)), &[], &[])), Some(9));
        // ohne Playhead im Medium: sichtbare zuerst, sonst von vorn
        assert_eq!(next_part(&p, none, &f(None, &[(55.0, 58.0)], &[])), Some(5));
        assert_eq!(next_part(&p, none, &Focus::default()), Some(0));
        // gescheiterte Stücke werden übersprungen
        assert_eq!(next_part(&p, |i| i == 0, &Focus::default()), Some(1));
    }

    #[test]
    fn stream_source_switches_rarely() {
        let p = pt(100.0);
        let mut ready = vec![false; 10];
        let m = SWITCH_MARGIN;
        let orig = |s: f64| Pick::Original { since_ms: (s * 1000.0) as i64 };
        // nichts da: Original
        assert_eq!(choose_stream(None, &ready, &p, 12.0, 100.0, m), orig(12.0));
        // Stücke 1–3 fertig: neuer Strom bei 12 s läuft aus ihnen bis 40 s
        ready[1..4].fill(true);
        let s = choose_stream(None, &ready, &p, 12.0, 100.0, m);
        assert_eq!(s, Pick::Parts { first: 1, end: 4 });
        // weiter so, solange genug vor dem Ende liegt, auch wenn neue Stücke dazukommen
        ready[4] = true;
        assert_eq!(choose_stream(Some(s), &ready, &p, 30.0, 100.0, m), s);
        // kurz vor dem Ende: mit den inzwischen fertigen Stücken weiter
        let s2 = choose_stream(Some(s), &ready, &p, 37.5, 100.0, m);
        assert_eq!(s2, Pick::Parts { first: 3, end: 5 });
        // kurz vor deren Ende und nichts dazu: Original …
        let o = choose_stream(Some(s2), &ready, &p, 47.5, 100.0, m);
        assert_eq!(o, orig(47.5));
        // … und nicht gleich zurück, nur weil das nächste Stück knapp danach fertig wurde
        ready[5] = true;
        assert_eq!(choose_stream(Some(o), &ready, &p, 48.0, 100.0, m), o);
        assert_eq!(choose_stream(Some(o), &ready, &p, 52.0, 100.0, m), o);
        // nach SWITCH_AHEAD mit genug Vorrat: zurück auf Stücke
        assert_eq!(choose_stream(Some(o), &ready, &p, 53.5, 100.0, m), Pick::Parts { first: 5, end: 6 });
    }

    #[test]
    fn stream_source_switches_back_when_enough_is_ready_and_respects_clip_end() {
        let p = pt(100.0);
        let mut ready = vec![false; 10];
        ready[5] = true;
        let m = SWITCH_MARGIN;
        // vom Original zurück auf Stücke erst ab SWITCH_AHEAD Vorrat
        let long_ago = Pick::Original { since_ms: 40_000 };
        assert_eq!(choose_stream(Some(long_ago), &ready, &p, 55.0, 100.0, m), long_ago);
        assert_eq!(choose_stream(Some(long_ago), &ready, &p, 53.0, 100.0, m), Pick::Parts { first: 5, end: 6 });
        // zurückgesprungen hinter den Wechsel: gleich neu wählen
        assert_eq!(choose_stream(Some(Pick::Original { since_ms: 70_000 }), &ready, &p, 51.0, 100.0, m), Pick::Parts { first: 5, end: 6 });
        // Clip endet im vorhandenen Bereich: bis zum Schluss aus Stücken, kein Umschalten
        let s = choose_stream(None, &ready, &p, 58.0, 59.5, m);
        assert_eq!(s, Pick::Parts { first: 5, end: 6 });
        assert_eq!(choose_stream(Some(s), &ready, &p, 59.4, 59.5, m), s);
        // letztes Stück der Quelle: reicht immer bis zum Ende
        ready[9] = true;
        assert_eq!(choose_stream(None, &ready, &p, 99.0, 100.0, m), Pick::Parts { first: 9, end: 10 });
        // Sprung aus dem Bereich heraus: neu wählen
        let far = choose_stream(Some(s), &ready, &p, 20.0, 100.0, m);
        assert_eq!(far, Pick::Original { since_ms: 20_000 });
        // Schlüssel: Original und „wie bisher“ gleich, alles andere verschieden
        assert_eq!(Pick::Original { since_ms: 5 }.tag(), Pick::Whole.tag());
        assert_ne!(Pick::Full.tag(), 0);
        assert_ne!(Pick::Parts { first: 1, end: 4 }.tag(), Pick::Parts { first: 1, end: 5 }.tag());
        assert_ne!(Pick::Parts { first: 0, end: 1 }.tag(), 0);
    }

    #[test]
    fn coverage_follows_trim_speed_and_reverse() {
        let p = pt(100.0);
        let mut ready = vec![false; 10];
        ready[2] = true;
        ready[3] = true;
        ready[9] = true;
        let r = ready_ranges(&p, &ready, 95.0);
        assert_eq!(r, vec![(20.0, 40.0), (90.0, 95.0)]);
        // Clip zeigt 10–50 s: 20–40 s ist die Mitte
        assert_eq!(clip_coverage(&r, 10.0, 40.0, false), vec![(0.25, 0.75)]);
        // getrimmt auf 30–35 s: ganz
        assert_eq!(clip_coverage(&r, 30.0, 5.0, false), vec![(0.0, 1.0)]);
        // Tempo 2: 10 s Clip zeigen 20 s Quelle (span = 20)
        assert_eq!(clip_coverage(&r, 30.0, 20.0, false), vec![(0.0, 0.5)]);
        // rückwärts: von rechts
        assert_eq!(clip_coverage(&r, 30.0, 20.0, true), vec![(0.5, 1.0)]);
        // nichts davon im Ausschnitt; leerer Clip
        assert!(clip_coverage(&r, 50.0, 10.0, false).is_empty());
        assert!(clip_coverage(&r, 50.0, 0.0, false).is_empty());
    }

    #[test]
    fn cleanup_removes_unfinished_and_merged_leftovers() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let touch = |p: PathBuf| std::fs::write(p, b"x").unwrap();
        let a = dir.join("aaaaaaaaaaaaaaaa-540.teile");
        let b = dir.join("bbbbbbbbbbbbbbbb-540.teile");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        touch(a.join("0000.mp4"));
        touch(a.join("0001.part.mp4"));
        touch(a.join("0000-0001.ffconcat"));
        touch(dir.join("aaaaaaaaaaaaaaaa-540.part.mp4"));
        touch(dir.join("cccccccccccccccc-540.mp4"));
        // b ist zusammengefügt (fertige Datei da), Ordner blieb (Windows: Datei war offen)
        touch(dir.join("bbbbbbbbbbbbbbbb-540.mp4"));
        touch(b.join("0000.mp4"));
        cleanup(dir);
        assert!(a.join("0000.mp4").is_file(), "fertige Stücke bleiben");
        assert!(!a.join("0001.part.mp4").exists() && !a.join("0000-0001.ffconcat").exists());
        assert!(!dir.join("aaaaaaaaaaaaaaaa-540.part.mp4").exists());
        assert!(!b.exists());
        assert!(dir.join("cccccccccccccccc-540.mp4").is_file() && dir.join("bbbbbbbbbbbbbbbb-540.mp4").is_file());
        assert_eq!(scan_ready(&a, 3), vec![true, false, false]);
    }

    #[test]
    fn changed_source_drops_old_parts() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let src = Path::new("x/quelle.mp4");
        let mk = |name: &str, s: &Path| {
            let p = dir.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("quelle.txt"), s.to_string_lossy().as_bytes()).unwrap();
            p
        };
        let old = mk("1111111111111111-540.teile", src);
        let other_h = mk("2222222222222222-720.teile", src);
        let same_hash = mk("2222222222222222-540-30_1fps.teile", src);
        let foreign = mk("3333333333333333-540.teile", Path::new("y/anders.mp4"));
        remove_stale(dir, src, &dir.join("2222222222222222-720.teile"));
        assert!(!old.exists(), "alter Hash weg");
        assert!(other_h.exists() && same_hash.exists() && foreign.exists());
    }

    #[test]
    fn concat_list_skips_empty_parts_and_is_written_once() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(dir.join(part_name(0)), b"x").unwrap();
        std::fs::write(dir.join(part_name(1)), b"").unwrap();
        let l = concat_list(dir, 0, 2).unwrap();
        assert_eq!(std::fs::read_to_string(&l).unwrap(), "ffconcat version 1.0\nfile 0000.mp4\n");
        assert_eq!(concat_list(dir, 0, 2).unwrap(), l);
        assert!(concat_list(dir, 1, 2).is_err(), "nur leere Stücke");
        assert!(concat_list(dir, 2, 3).is_err(), "fehlendes Stück");
    }
}
