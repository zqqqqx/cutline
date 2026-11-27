//! Proxy in parts: the source is built split into parts of equal length ([`PART_SECS`]); each
//! finished part is usable at once for single frames and streams. Stored per proxy in a folder
//! next to the finished file (`<proxy name>.parts/`): `0007.mp4` (finished), `0007.<pid>.part.mp4`
//! (in progress; write fully first, then rename), `0003-0012.ffconcat` (list for a stream
//! over parts 3…11) and `source.txt` (path of the source, for cleaning up after a change).
//! Once all parts are there, they are joined losslessly into the usual single-file proxy.
//!
//! Frame-exact: each part computes on the same frame grid as a build in one go (`-copyts`,
//! `fps=…:start_time=<source start + part start>`, exactly `per` frames per part). A part
//! without a single frame (video ends before the file length) is an empty file.

use std::ops::Range;
use std::path::{Path, PathBuf};

use cl_core::Rational;

/// Part length: measured (1080p60 → 540p30, NVENC, computer under load) the first part takes
/// 1.3 s at 10 s (5 s: 0.9 s; 20 s: 2.4 s), the whole build 10.2 s per minute of source (5 s: 13.9 s;
/// 20 s: 8.3 s; in one go 9.5 s) – 10 s meets the goal "first part < 2 s" with little cost per start.
pub const PART_SECS: f64 = 10.0;
/// A part may fail this often, then it is skipped (the original stays there)
pub const MAX_FAILS: u8 = 2;
/// Stream from parts: this long (source seconds) before the end of the available parts it switches to the
/// original (or further parts) – the replacement stream must start up before …
pub const SWITCH_MARGIN: f64 = 3.0;
/// … and from the original to parts only when this much is available in one piece (no back and forth)
pub const SWITCH_AHEAD: f64 = 6.0;

/// Division of a source into parts: proxy frame rate, frames per part, count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parting {
    pub fps: Rational,
    pub per: u32,
    pub count: u32,
}

impl Parting {
    /// `None`: duration or frame rate unknown (then it is built in one go)
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

    /// Start of part `k` (source seconds from the file start)
    pub fn start(&self, k: u32) -> f64 {
        self.secs(k as u64 * self.per as u64)
    }

    /// Proxy frame number at `t` (rounded down)
    fn frame_at(&self, t: f64) -> u64 {
        (t.max(0.0) * self.fps.num as f64 / self.fps.den as f64 + 1e-6).floor() as u64
    }

    /// Part in which `t` lies (at the end: the last)
    pub fn part_of(&self, t: f64) -> u32 {
        ((self.frame_at(t) / self.per as u64) as u32).min(self.count.saturating_sub(1))
    }

    /// Parts touching the source range `[a, b)`
    pub fn parts_in(&self, a: f64, b: f64) -> Range<u32> {
        if b <= a || self.count == 0 {
            return 0..0;
        }
        self.part_of(a)..self.part_of(b - 1e-6) + 1
    }
}

/// File name of a finished part in the parts folder
pub fn part_name(k: u32) -> String {
    format!("{k:04}.mp4")
}

/// Temp file FFmpeg writes to before it is renamed to `out`. With process id:
/// otherwise two Cutline windows with the same source write to the same file, and whoever renames
/// first stores a broken one as finished (`0007.mp4` → `0007.<pid>.part.mp4`).
pub fn tmp_of(out: &Path) -> PathBuf {
    out.with_extension(format!("{}.part.mp4", std::process::id()))
}

/// Extension of a parts folder and the file in it that names the source
pub const PARTS_EXT: &str = "parts";
pub const SOURCE_FILE: &str = "source.txt";

/// Parts folders written by versions up to 0.19.3 (`<name>.teile` with `quelle.txt`) get the
/// current names, so their finished parts are used on and not left behind in the cache.
fn rename_old_parts(dir: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && p.extension().is_some_and(|x| x == cl_core::legacy::old("parts_ext")) {
            let new = p.with_extension(PARTS_EXT);
            if !new.exists() && std::fs::rename(&p, &new).is_ok() {
                let _ = std::fs::rename(new.join(cl_core::legacy::old("source_file")), new.join(SOURCE_FILE));
            }
        }
    }
}

/// Parts folder of a proxy (`…/abc-540-30_1fps.mp4` → `…/abc-540-30_1fps.parts`)
pub fn parts_dir(out: &Path) -> PathBuf {
    out.with_extension(PARTS_EXT)
}

/// Finished parts in the folder (per part: file present; empty file = part without a frame)
pub fn scan_ready(dir: &Path, count: u32) -> Vec<bool> {
    (0..count).map(|k| dir.join(part_name(k)).is_file()).collect()
}

/// Leftovers of the last run: unfinished files (`.part.mp4`, also after a hard exit), stream lists
/// and parts folders whose proxy is already joined. Finished parts stay.
/// Deleting the `.part` of a second, running window is harmless: on Windows it fails
/// (FFmpeg keeps the file open), on Unix that window's rename fails afterwards – the part counts
/// as failed there, a broken file never comes about.
pub fn cleanup(dir: &Path) {
    rename_old_parts(dir);
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".part.mp4") {
            let _ = std::fs::remove_file(&p);
        } else if p.is_dir() && name.ends_with(".parts") {
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

/// Remove parts folders of the same source from an older version of the file (other hash);
/// folders of the same version at another height/frame rate stay (the setting can switch back).
pub fn remove_stale(dir: &Path, src: &Path, keep: &Path) {
    let want = src.to_string_lossy();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p == keep || !p.is_dir() || !e.file_name().to_string_lossy().ends_with(".parts") {
            continue;
        }
        let same_src = std::fs::read_to_string(p.join(SOURCE_FILE)).is_ok_and(|s| s.trim() == want);
        // same hash part of the name = same file version (only other height/frame rate): stays
        let same_hash = |a: &Path, b: &Path| {
            let h = |p: &Path| p.file_name().map(|n| n.to_string_lossy().chars().take(16).collect::<String>());
            h(a) == h(b)
        };
        if same_src && !same_hash(&p, keep) {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

/// List for the concat reader over parts `first..end` (relative names: in the same folder,
/// safe without `-safe 0`). Empty parts (without a frame) are left out. Written only once.
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
        return Err(std::io::Error::other("no frames"));
    }
    // write fully first, then rename (a second writer writes the same)
    let tmp = dir.join(format!("{first:04}-{end:04}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, s)?;
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    if path.is_file() {
        Ok(path)
    } else {
        Err(std::io::Error::other("list not written"))
    }
}

/// Single frame at source time `t` from a part: FFmpeg delivers the first frame from `-ss`, i.e. the
/// proxy frame `ceil(t·fps)`; result (part, time in the part – half a frame before, so rounding
/// does not drop the frame). `None` past the last frame.
pub fn still_part(pt: &Parting, t: f64) -> Option<(u32, f64)> {
    let j = (t.max(0.0) * pt.fps.num as f64 / pt.fps.den as f64 - 1e-6).ceil().max(0.0) as u64;
    let k = (j / pt.per as u64) as u32;
    if k >= pt.count {
        return None;
    }
    let rel = pt.secs(j) - pt.start(k) - pt.secs(1) / 2.0;
    Some((k, rel.max(0.0)))
}

/// Where a stream comes from (part of the stream key: a change opens a new stream via the replacement
/// path of the player core, which only takes over when it is not behind the old one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    /// as before: finished proxy, otherwise original (media without part build)
    Whole,
    /// Original (parts missing), chosen at source time `since_ms`
    Original { since_ms: i64 },
    /// finished, joined proxy
    Full,
    /// Parts `first..end`
    Parts { first: u32, end: u32 },
}

impl Pick {
    /// Share in the stream key: `Whole` and original open the same file (0 = key unchanged)
    pub fn tag(self) -> u64 {
        match self {
            Pick::Whole | Pick::Original { .. } => 0,
            Pick::Full => 0x6a09_e667_f3bc_c909,
            Pick::Parts { first, end } => ((first as u64) << 32 | end as u64 | 1 << 63).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
        }
    }
}

/// Source of a stream from source time `p` (end of the clip section `clip_end`), with the previous
/// choice `prev` of the same layer. Rules (no back and forth):
/// - parts keep running while more than `margin` lies before their end or they reach the clip end;
/// - shortly before the end: continue with the parts finished meanwhile, otherwise original;
/// - from original to parts at the earliest [`SWITCH_AHEAD`] after the switch to the original and only
///   when at least that much (or 2 × `margin`) is available in one piece;
/// - new stream: parts, if they reach beyond `margin`.
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
                fresh(margin) // jumped back: choose anew
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

/// Where the work should go (source seconds): playhead place with end of the clip section (only
/// clips played forwards), visible and in the project used ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    pub at: Option<(f64, f64)>,
    pub visible: Vec<(f64, f64)>,
    pub used: Vec<(f64, f64)>,
}

/// Next part: the one at the playhead, then forwards to the end of the clip section, then
/// visible, then ranges used in the project, then the rest (from the playhead). Asked again after each
/// part – if the playhead jumps, the new place applies, the running part is finished.
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

/// Finished ranges (source seconds), neighbours merged
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

/// Shares of the clip width (0…1) whose source is already available as a part. The clip shows the source
/// `[src_in, src_in + span)` (speed is included in `span`), reversed from right to left.
/// shortcut: speed curves are converted linearly, more exactly only once they are common
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
        // divided exactly: no empty last part
        assert_eq!(pt(20.0).count, 2);
        // half a frame above the grid does not count as its own part
        assert_eq!(pt(20.0 + 0.4 / 30.0).count, 2);
        assert_eq!(pt(20.0 + 1.0 / 30.0).count, 3);
        // NTSC: part boundaries on the frame grid
        let n = Parting::new(60.0, Rational::new(30000, 1001), 10.0).unwrap();
        assert_eq!(n.per, 300);
        assert!((n.start(1) - 10.01).abs() < 1e-9);
        // very short: one part
        assert_eq!(pt(0.01).count, 1);
        // unknown/broken: no division
        assert_eq!(Parting::new(0.0, Rational::new(30, 1), 10.0), None);
        assert_eq!(Parting::new(f64::NAN, Rational::new(30, 1), 10.0), None);
        assert_eq!(Parting::new(10.0, Rational::new(0, 1), 10.0), None);
        assert_eq!(Parting::new(10.0, Rational::new(30, 0), 10.0), None);
        // ranges
        assert_eq!(p.parts_in(5.0, 15.0), 0..2);
        assert_eq!(p.parts_in(10.0, 20.0), 1..2);
        assert_eq!(p.parts_in(3.0, 3.0), 0..0);
    }

    #[test]
    fn still_comes_from_the_part_ffmpeg_would_pick() {
        let p = pt(25.0);
        // in the middle of the part: frame 30 (1.0 s) → part 0, half a frame before
        let (k, rel) = still_part(&p, 1.0).unwrap();
        assert_eq!(k, 0);
        assert!((rel - (1.0 - 1.0 / 60.0)).abs() < 1e-9);
        // just before the seam: the first frame from t is the first of the next part
        let (k, rel) = still_part(&p, 9.99).unwrap();
        assert_eq!((k, rel), (1, 0.0));
        // past the last part: none
        assert_eq!(still_part(&p, 30.0), None);
    }

    #[test]
    fn order_playhead_then_forward_then_visible_then_rest() {
        let p = pt(100.0); // 10 parts
        let none = |_| false;
        let f = |at: Option<(f64, f64)>, vis: &[(f64, f64)], used: &[(f64, f64)]| Focus { at, visible: vis.to_vec(), used: used.to_vec() };
        // playhead at 42 s, clip up to 65 s → 4, then 5, 6
        let focus = f(Some((42.0, 65.0)), &[(80.0, 90.0)], &[(10.0, 20.0)]);
        assert_eq!(next_part(&p, none, &focus), Some(4));
        assert_eq!(next_part(&p, |i| i <= 4, &focus), Some(5));
        assert_eq!(next_part(&p, |i| (4..=6).contains(&i), &focus), Some(8), "then visible");
        assert_eq!(next_part(&p, |i| (4..=6).contains(&i) || i == 8, &focus), Some(1), "then used");
        // then the rest from the playhead, the start last
        let done = |i| [1, 4, 5, 6, 8].contains(&i);
        assert_eq!(next_part(&p, done, &focus), Some(7));
        assert_eq!(next_part(&p, |i| done(i) || i >= 7, &focus), Some(0));
        assert_eq!(next_part(&p, |_| true, &focus), None);
        // jump: replanned from the new place
        assert_eq!(next_part(&p, |i| i == 4, &f(Some((95.0, 100.0)), &[], &[])), Some(9));
        // without playhead in the medium: visible ones first, otherwise from the start
        assert_eq!(next_part(&p, none, &f(None, &[(55.0, 58.0)], &[])), Some(5));
        assert_eq!(next_part(&p, none, &Focus::default()), Some(0));
        // failed parts are skipped
        assert_eq!(next_part(&p, |i| i == 0, &Focus::default()), Some(1));
    }

    #[test]
    fn stream_source_switches_rarely() {
        let p = pt(100.0);
        let mut ready = vec![false; 10];
        let m = SWITCH_MARGIN;
        let orig = |s: f64| Pick::Original { since_ms: (s * 1000.0) as i64 };
        // nothing there: original
        assert_eq!(choose_stream(None, &ready, &p, 12.0, 100.0, m), orig(12.0));
        // parts 1–3 finished: a new stream at 12 s runs from them up to 40 s
        ready[1..4].fill(true);
        let s = choose_stream(None, &ready, &p, 12.0, 100.0, m);
        assert_eq!(s, Pick::Parts { first: 1, end: 4 });
        // keep going while enough lies before the end, even when new parts are added
        ready[4] = true;
        assert_eq!(choose_stream(Some(s), &ready, &p, 30.0, 100.0, m), s);
        // shortly before the end: continue with the parts finished meanwhile
        let s2 = choose_stream(Some(s), &ready, &p, 37.5, 100.0, m);
        assert_eq!(s2, Pick::Parts { first: 3, end: 5 });
        // shortly before their end and nothing added: original …
        let o = choose_stream(Some(s2), &ready, &p, 47.5, 100.0, m);
        assert_eq!(o, orig(47.5));
        // … and not straight back just because the next part finished right after
        ready[5] = true;
        assert_eq!(choose_stream(Some(o), &ready, &p, 48.0, 100.0, m), o);
        assert_eq!(choose_stream(Some(o), &ready, &p, 52.0, 100.0, m), o);
        // after SWITCH_AHEAD with enough supply: back to parts
        assert_eq!(choose_stream(Some(o), &ready, &p, 53.5, 100.0, m), Pick::Parts { first: 5, end: 6 });
    }

    #[test]
    fn stream_source_switches_back_when_enough_is_ready_and_respects_clip_end() {
        let p = pt(100.0);
        let mut ready = vec![false; 10];
        ready[5] = true;
        let m = SWITCH_MARGIN;
        // from the original back to parts only from SWITCH_AHEAD supply
        let long_ago = Pick::Original { since_ms: 40_000 };
        assert_eq!(choose_stream(Some(long_ago), &ready, &p, 55.0, 100.0, m), long_ago);
        assert_eq!(choose_stream(Some(long_ago), &ready, &p, 53.0, 100.0, m), Pick::Parts { first: 5, end: 6 });
        // jumped back behind the switch: choose anew at once
        assert_eq!(choose_stream(Some(Pick::Original { since_ms: 70_000 }), &ready, &p, 51.0, 100.0, m), Pick::Parts { first: 5, end: 6 });
        // clip ends in the available range: from parts until the end, no switching
        let s = choose_stream(None, &ready, &p, 58.0, 59.5, m);
        assert_eq!(s, Pick::Parts { first: 5, end: 6 });
        assert_eq!(choose_stream(Some(s), &ready, &p, 59.4, 59.5, m), s);
        // last part of the source: always reaches the end
        ready[9] = true;
        assert_eq!(choose_stream(None, &ready, &p, 99.0, 100.0, m), Pick::Parts { first: 9, end: 10 });
        // jump out of the range: choose anew
        let far = choose_stream(Some(s), &ready, &p, 20.0, 100.0, m);
        assert_eq!(far, Pick::Original { since_ms: 20_000 });
        // key: original and `Whole` equal, everything else different
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
        // clip shows 10–50 s: 20–40 s is the middle
        assert_eq!(clip_coverage(&r, 10.0, 40.0, false), vec![(0.25, 0.75)]);
        // trimmed to 30–35 s: whole
        assert_eq!(clip_coverage(&r, 30.0, 5.0, false), vec![(0.0, 1.0)]);
        // speed 2: a 10 s clip shows 20 s of source (span = 20)
        assert_eq!(clip_coverage(&r, 30.0, 20.0, false), vec![(0.0, 0.5)]);
        // reverse: from the right
        assert_eq!(clip_coverage(&r, 30.0, 20.0, true), vec![(0.5, 1.0)]);
        // none of it in the section; empty clip
        assert!(clip_coverage(&r, 50.0, 10.0, false).is_empty());
        assert!(clip_coverage(&r, 50.0, 0.0, false).is_empty());
    }

    #[test]
    fn cleanup_removes_unfinished_and_merged_leftovers() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let touch = |p: PathBuf| std::fs::write(p, b"x").unwrap();
        let a = dir.join("aaaaaaaaaaaaaaaa-540.parts");
        let b = dir.join("bbbbbbbbbbbbbbbb-540.parts");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        touch(a.join("0000.mp4"));
        touch(tmp_of(&a.join(part_name(1))));
        touch(a.join("0002.12345.part.mp4"));
        touch(a.join("0000-0001.ffconcat"));
        touch(dir.join("aaaaaaaaaaaaaaaa-540.part.mp4"));
        touch(dir.join("cccccccccccccccc-540.mp4"));
        // b is joined (finished file there), folder stayed (Windows: file was open)
        touch(dir.join("bbbbbbbbbbbbbbbb-540.mp4"));
        touch(b.join("0000.mp4"));
        cleanup(dir);
        assert!(a.join("0000.mp4").is_file(), "finished parts stay");
        assert!(!tmp_of(&a.join(part_name(1))).exists() && !a.join("0002.12345.part.mp4").exists() && !a.join("0000-0001.ffconcat").exists());
        assert!(!dir.join("aaaaaaaaaaaaaaaa-540.part.mp4").exists());
        assert!(!b.exists());
        assert!(dir.join("cccccccccccccccc-540.mp4").is_file() && dir.join("bbbbbbbbbbbbbbbb-540.mp4").is_file());
        assert_eq!(scan_ready(&a, 3), vec![true, false, false]);
    }

    /// Two windows with the same source: different temp files per process, the same finished
    #[test]
    fn unfinished_file_names_differ_per_process() {
        let out = Path::new("t").join(part_name(7));
        let tmp = tmp_of(&out);
        assert_eq!(tmp, Path::new("t").join(format!("0007.{}.part.mp4", std::process::id())));
        assert_ne!(tmp.file_name().unwrap(), "0007.part.mp4");
        assert_eq!(tmp_of(Path::new("abc-540.mp4")), PathBuf::from(format!("abc-540.{}.part.mp4", std::process::id())));
    }

    #[test]
    fn changed_source_drops_old_parts() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let src = Path::new("x/quelle.mp4");
        let mk = |name: &str, s: &Path| {
            let p = dir.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join(SOURCE_FILE), s.to_string_lossy().as_bytes()).unwrap();
            p
        };
        let old = mk("1111111111111111-540.parts", src);
        let other_h = mk("2222222222222222-720.parts", src);
        let same_hash = mk("2222222222222222-540-30_1fps.parts", src);
        let foreign = mk("3333333333333333-540.parts", Path::new("y/anders.mp4"));
        remove_stale(dir, src, &dir.join("2222222222222222-720.parts"));
        assert!(!old.exists(), "old hash gone");
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
        assert!(concat_list(dir, 1, 2).is_err(), "only empty parts");
        assert!(concat_list(dir, 2, 3).is_err(), "missing part");
    }
    /// Parts of a proxy that version 0.19.3 or older began are used on under the current names
    #[test]
    fn parts_folders_of_older_versions_are_renamed() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("aaaaaaaaaaaaaaaa-540.teile");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("0000.mp4"), b"x").unwrap();
        std::fs::write(old.join("quelle.txt"), b"x/y.mp4").unwrap();
        // a folder under the new name already: the old one stays untouched beside it
        let both = d.path().join("bbbbbbbbbbbbbbbb-540.teile");
        std::fs::create_dir_all(&both).unwrap();
        std::fs::create_dir_all(both.with_extension(PARTS_EXT)).unwrap();
        cleanup(d.path());
        let new = parts_dir(&d.path().join("aaaaaaaaaaaaaaaa-540.mp4"));
        assert!(new.join("0000.mp4").is_file() && !old.exists());
        assert_eq!(std::fs::read_to_string(new.join(SOURCE_FILE)).unwrap(), "x/y.mp4");
        assert_eq!(scan_ready(&new, 2), [true, false]);
        assert!(both.is_dir());
    }
}
