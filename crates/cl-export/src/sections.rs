//! Export in Abschnitten für große Projekte.
//!
//! FFmpeg bekommt je Clip einen Eingang (`-ss … -i <Pfad>`). Windows begrenzt die Befehlszeile
//! auf 32 767 Zeichen, und FFmpeg öffnet alle Eingänge vor dem ersten Bild (Sekunden bis Minuten
//! bei tausenden). Über den Schwellen wird die Zeitleiste darum in Abschnitte zerlegt, jeder wird
//! mit dem Bereichs-Export gerechnet, die Ergebnisse werden ohne Neukodierung aneinandergehängt.
//! Grenzen liegen auf einem Raster, das zugleich ganze Bilder, ganze Mikrosekunden und ganze
//! Samples (48 kHz) trifft; so passen Bild und Ton an jeder Naht genau.

use cl_core::*;

use crate::graph;

/// Sichere Länge der Befehlszeile (Windows: 32 767 Zeichen, Rest für Programmpfad und Puffer).
pub const MAX_CMD: usize = 24_000;
/// Höchstens so viele Eingänge je FFmpeg (Anlaufzeit: FFmpeg öffnet alle vor dem ersten Bild;
/// mit vielen Eingängen warten die Zweige des Graphen zudem lange aufeinander).
pub const MAX_INPUTS: usize = 40;
/// Bis zu so vielen Eingängen bleibt es beim einen Durchgang wie bisher.
pub const SINGLE_INPUTS: usize = 150;
/// Harte Grenze der Windows-Befehlszeile (Zeichen).
pub const HARD_CMD: usize = 32_000;

/// Zeichen der Befehlszeile (Argumente mit Leerzeichen getrennt, ggf. in Anführungszeichen).
pub fn cmd_len(args: &[String]) -> usize {
    args.iter().map(|a| a.len() + 3).sum()
}

/// Zeichen eines Eingangs (`-ss x -t y -i <Pfad>` bzw. `-loop 1 -t y -i <Pfad>`), großzügig.
fn input_len(m: &MediaRef) -> usize {
    m.path.as_os_str().len() + 48
}

/// Teil der Zeitleiste, der Eingänge kostet: eine Übergangsgruppe, ein Clip oder ein Tonclip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub start: Time,
    pub end: Time,
    pub inputs: usize,
    pub chars: usize,
    /// Ton (eigener FFmpeg je Abschnitt) statt Bild
    pub audio: bool,
}

/// Eingänge des Exports von `start..end`, wie [`graph::video_part`] und [`graph::audio_part`]
/// sie anlegen (Bild: sichtbare Spuren, Übergangsgruppen als Ganzes; Ton: hörbare Clips).
pub(crate) fn units(p: &Project, start: Time, end: Time, video: bool, audio: bool) -> Vec<Unit> {
    let mut v = Vec::new();
    if video {
        for tr in p.timeline.tracks.iter().filter(|t| !t.hidden && !matches!(t.kind, TrackKind::Effect | TrackKind::Filter)) {
            for g in graph::transition_groups(tr) {
                let (a, b) = (g[0].start, g.last().expect("nicht leer").end());
                if b <= start || a >= end {
                    continue;
                }
                let mut u = Unit { start: a, end: b, inputs: 0, chars: 0, audio: false };
                for c in g {
                    let ClipContent::Media { media, kind: MediaKind::Video | MediaKind::Image } = &c.content else { continue };
                    let Some(m) = p.media(*media) else { continue };
                    u.inputs += 1;
                    u.chars += input_len(m);
                    if let Some(mp) = c.fx.cutout.as_ref().filter(|mp| std::path::Path::new(mp).is_file()) {
                        u.inputs += 1;
                        u.chars += mp.len() + 48;
                    }
                }
                if u.inputs > 0 {
                    v.push(u);
                }
            }
        }
    }
    if audio {
        v.extend(graph::audible_clips(p, start, end).map(|(c, m)| Unit { start: c.start, end: c.end(), inputs: 1, chars: input_len(m), audio: true }));
    }
    v.sort_by_key(|u| (u.start, u.end));
    v
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn lcm(a: u64, b: u64) -> u64 {
    a / gcd(a, b) * b
}

/// Raster der Abschnittsgrenzen: kleinste Bildzahl, die ganze Mikrosekunden und ganze Samples
/// (48 kHz) ergibt (60 fps: 3 Bilder = 50 ms; 29,97 fps: 15 Bilder ≈ 0,5 s).
pub fn grid_step(fps: Rational) -> Time {
    let (num, den) = (fps.num.max(1) as u64, fps.den.max(1) as u64);
    let need = |rate: u64| num / gcd(num, den * rate);
    let g = lcm(need(1_000_000), need(48_000));
    Time((g * den * 1_000_000 / num) as i64)
}

/// Grenzen für einen Abschnitt.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub inputs: usize,
    pub chars: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { inputs: MAX_INPUTS, chars: MAX_CMD };
}

/// (Eingänge, Zeichen) der Einheiten, die `a..b` berühren – je FFmpeg (Bild und Ton laufen
/// getrennt), der größere Wert zählt.
fn cost(units: &[Unit], a: Time, b: Time) -> (usize, usize) {
    let (v, au) = units
        .iter()
        .take_while(|u| u.start < b)
        .filter(|u| u.end > a)
        .fold(((0, 0), (0, 0)), |(v, au), u| {
            if u.audio {
                (v, (au.0 + u.inputs, au.1 + u.chars))
            } else {
                ((v.0 + u.inputs, v.1 + u.chars), au)
            }
        });
    (v.0.max(au.0), v.1.max(au.1))
}

fn fits(units: &[Unit], a: Time, b: Time, base: usize, lim: Limits) -> bool {
    let (i, c) = cost(units, a, b);
    i <= lim.inputs && c + base <= lim.chars
}

/// Passt der ganze Bereich in einen Durchgang (ein FFmpeg für Bild und Ton zusammen)?
pub fn single_pass(units: &[Unit], start: Time, end: Time, base: usize, lim: Limits) -> bool {
    let (i, c) = units
        .iter()
        .take_while(|u| u.start < end)
        .filter(|u| u.end > start)
        .fold((0, 0), |(i, c), u| (i + u.inputs, c + u.chars));
    i <= lim.inputs && c + base <= lim.chars
}

/// Zerlegt `start..end` in Abschnitte, die je höchstens `lim` kosten (`base` = Zeichen ohne
/// Eingänge). Grenzen liegen auf `start + k·step`; innerhalb der hinteren Hälfte des möglichen
/// Abschnitts wird die Stelle gewählt, an der am wenigsten durchgeschnitten wird (am liebsten ein
/// Schnitt aller Spuren; Übergangsgruppen zählen mit ihren Eingängen). `Err(t)`: schon ein
/// Rasterschritt ab `t` hat zu viele gleichzeitige Clips.
pub fn plan(units: &[Unit], start: Time, end: Time, step: Time, base: usize, lim: Limits) -> std::result::Result<Vec<(Time, Time)>, Time> {
    let step = Time(step.0.max(1));
    let at = |k: i64| Time(start.0 + k * step.0);
    let mut out = Vec::new();
    let mut ka = 0i64;
    // letzter Rasterpunkt vor dem Ende
    let k_end = (end.0 - start.0 - 1) / step.0;
    loop {
        let a = at(ka);
        // Einheiten ab hier (sortiert nach Start; frühere, die schon enden, fallen weg)
        let first = units.partition_point(|u| u.start < a);
        let back = units[..first].iter().filter(|u| u.end > a).copied();
        let local: Vec<Unit> = back.chain(units[first..].iter().copied()).collect();
        if fits(&local, a, end, base, lim) {
            out.push((a, end));
            return Ok(out);
        }
        // größtes k mit passendem Abschnitt (Kosten wachsen mit k)
        let (mut lo, mut hi) = (ka, k_end);
        if !fits(&local, a, at(ka + 1).min(end), base, lim) {
            return Err(a);
        }
        lo += 1;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if fits(&local, a, at(mid), base, lim) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let k_max = lo;
        // bevorzugte Grenze in der hinteren Hälfte: wenigste Eingänge, die über die Grenze reichen
        let k_min = ka + ((k_max - ka) / 2).max(1);
        let span = |b: Time| local.iter().take_while(|u| u.start < b).filter(|u| u.end > b).map(|u| u.inputs).sum::<usize>();
        let mut cands: Vec<i64> = vec![k_max];
        for u in local.iter().take_while(|u| u.start < at(k_max)) {
            for e in [u.start, u.end] {
                let k = (e.0 - start.0).div_euclid(step.0);
                cands.extend([k, k + 1]);
            }
        }
        let kb = cands
            .into_iter()
            .filter(|k| (k_min..=k_max).contains(k))
            .min_by_key(|&k| (span(at(k)), std::cmp::Reverse(k)))
            .unwrap_or(k_max);
        out.push((a, at(kb)));
        ka = kb;
    }
}

/// Zeit als `m:ss` bzw. `h:mm:ss` (für Fehlertexte).
pub fn clock(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Freier Platz auf dem Laufwerk von `dir` in Bytes (`None`: unbekannt).
#[cfg(windows)]
pub fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let w: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: nullterminierter Pfad, gültige Zeiger auf lokale Variablen
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some(avail)
}

/// shortcut: außerhalb von Windows keine Prüfung (statvfs bräuchte libc); nachrüsten, wenn es dort fehlt
#[cfg(not(windows))]
pub fn free_space(_dir: &std::path::Path) -> Option<u64> {
    None
}

/// Größe in lesbarer Form (MB/GB).
pub fn size_text(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", b.div_ceil(1 << 20))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f64) -> Time {
        Time::from_secs_f64(x)
    }

    fn u(a: f64, b: f64, inputs: usize) -> Unit {
        Unit { start: s(a), end: s(b), inputs, chars: inputs * 200, audio: false }
    }

    const LIM: Limits = Limits { inputs: 10, chars: 1_000_000 };

    #[test]
    fn grid_hits_frames_microseconds_and_samples() {
        for (n, d, want_frames) in [(60, 1, 3u64), (30, 1, 3), (25, 1, 1), (24, 1, 3), (30000, 1001, 15), (60000, 1001, 15), (24000, 1001, 3), (50, 1, 1)] {
            let st = grid_step(Rational::new(n, d));
            let frames = st.as_secs_f64() * n as f64 / d as f64;
            assert!((frames - want_frames as f64).abs() < 1e-6, "{n}/{d}: {frames}");
            assert_eq!((st.0 as u64 * 48_000) % 1_000_000, 0, "{n}/{d}: Samples");
        }
        // kaputte Rate: kein Absturz, positiver Schritt
        assert!(grid_step(Rational::new(0, 0)).0 > 0);
    }

    #[test]
    fn small_projects_stay_one_pass() {
        let units = vec![u(0.0, 1.0, 1), u(1.0, 2.0, 1)];
        assert!(single_pass(&units, s(0.0), s(2.0), 500, LIM));
        assert_eq!(plan(&units, s(0.0), s(2.0), s(0.05), 500, LIM).unwrap(), vec![(s(0.0), s(2.0))]);
    }

    /// Prüft eine Zerlegung: lückenlos, auf dem Raster, jeder Abschnitt in den Grenzen.
    fn check(units: &[Unit], start: Time, end: Time, step: Time, plan: &[(Time, Time)], lim: Limits) {
        assert_eq!(plan.first().unwrap().0, start);
        assert_eq!(plan.last().unwrap().1, end);
        for w in plan.windows(2) {
            assert_eq!(w[0].1, w[1].0, "lückenlos");
        }
        for (a, b) in plan {
            assert!(a < b);
            assert_eq!((a.0 - start.0) % step.0, 0, "Raster");
            let (i, c) = cost(units, *a, *b);
            assert!(i <= lim.inputs && c <= lim.chars, "{a:?}..{b:?}: {i} Eingänge");
        }
    }

    #[test]
    fn cuts_prefer_places_without_running_clips() {
        // zwei Spuren mit Schnitten bei 1 s, 2 s …; eine Spur ist versetzt (Schnitte bei x.5)
        let mut units = Vec::new();
        for i in 0..40 {
            units.push(u(i as f64, i as f64 + 1.0, 1));
            units.push(u(i as f64, i as f64 + 1.0, 1));
            if i % 4 == 0 {
                units.push(u(i as f64 + 0.5, i as f64 + 4.5, 1));
            }
        }
        units.sort_by_key(|u| (u.start, u.end));
        let step = s(0.05);
        let p = plan(&units, s(0.0), s(40.0), step, 0, LIM).unwrap();
        check(&units, s(0.0), s(40.0), step, &p, LIM);
        assert!(p.len() > 5);
        // Grenzen liegen auf Schnitten der beiden dichten Spuren (ganze Sekunden)
        for (_, b) in &p[..p.len() - 1] {
            assert_eq!(b.0 % 1_000_000, 0, "{b:?}");
        }
    }

    #[test]
    fn long_and_short_clips_and_gaps() {
        // langer Hintergrund (ganze Länge), viele sehr kurze Clips, dazwischen eine leere Strecke
        let mut units = vec![u(0.0, 600.0, 1)];
        for i in 0..300 {
            units.push(u(i as f64 * 0.1, i as f64 * 0.1 + 0.1, 1));
        }
        for i in 0..100 {
            units.push(u(400.0 + i as f64 * 0.02, 400.0 + i as f64 * 0.02 + 0.02, 1));
        }
        units.sort_by_key(|u| (u.start, u.end));
        let step = grid_step(Rational::new(60, 1));
        let p = plan(&units, s(0.0), s(600.0), step, 0, LIM).unwrap();
        check(&units, s(0.0), s(600.0), step, &p, LIM);
        // die leere Strecke wird nicht unnötig zerteilt
        assert!(p.iter().any(|(a, b)| a.as_secs_f64() <= 31.0 && b.as_secs_f64() >= 399.0), "{p:?}");
    }

    #[test]
    fn user_range_and_offgrid_start() {
        let units: Vec<Unit> = (0..100).map(|i| u(i as f64 * 0.5, i as f64 * 0.5 + 0.5, 1)).collect();
        let (a, b) = (s(3.123457), s(41.0));
        let step = grid_step(Rational::new(30000, 1001));
        let p = plan(&units, a, b, step, 0, LIM).unwrap();
        check(&units, a, b, step, &p, LIM);
    }

    #[test]
    fn too_many_at_once_is_an_error_with_time() {
        // 12 Spuren übereinander ab 5 s, Grenze 10 Eingänge
        let mut units: Vec<Unit> = (0..12).map(|_| u(5.0, 6.0, 1)).collect();
        units.insert(0, u(0.0, 5.0, 1));
        let e = plan(&units, s(0.0), s(6.0), s(0.05), 0, LIM).unwrap_err();
        assert_eq!(e, s(5.0));
    }

    #[test]
    fn long_paths_keep_every_section_below_the_limit() {
        // 2500 Clips auf 4 Spuren mit 260-Zeichen-Pfaden: Zeichen begrenzen vor den Eingängen
        let mut units = Vec::new();
        for t in 0..4 {
            for i in 0..625 {
                let a = i as f64 * 2.5 + t as f64 * 0.3;
                units.push(Unit { start: s(a), end: s(a + 2.5), inputs: 1, chars: 260 + 48, audio: t == 3 });
            }
        }
        units.sort_by_key(|u| (u.start, u.end));
        let base = 2_000;
        let p = plan(&units, s(0.0), s(1563.0), grid_step(Rational::new(60, 1)), base, Limits::DEFAULT).unwrap();
        for (a, b) in &p {
            let (i, c) = cost(&units, *a, *b);
            assert!(i <= MAX_INPUTS && c + base <= MAX_CMD, "{i} {c}");
        }
        assert!(p.len() >= 1875 / MAX_INPUTS, "{}", p.len());
    }

    #[test]
    fn clock_text() {
        assert_eq!(clock(s(65.4)), "1:05");
        assert_eq!(clock(s(3725.0)), "1:02:05");
    }
}
