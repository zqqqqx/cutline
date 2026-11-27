//! Export in sections for large projects.
//!
//! FFmpeg gets one input per clip (`-ss … -i <path>`). Windows limits the command line
//! to 32,767 characters, and FFmpeg opens all inputs before the first frame (seconds to minutes
//! with thousands). Above the thresholds the timeline is therefore split into sections, each is
//! computed with the range export, the results are concatenated without re-encoding.
//! Boundaries lie on a grid that hits whole frames, whole microseconds and whole
//! samples (48 kHz) at once; so picture and audio fit exactly at every seam.

use cl_core::*;

use crate::graph;

/// Safe command line length (Windows: 32,767 characters, the rest for program path and buffer).
pub const MAX_CMD: usize = 24_000;
/// At most this many inputs per FFmpeg (startup time: FFmpeg opens all before the first frame;
/// with many inputs the graph's branches also wait for each other for a long time).
pub const MAX_INPUTS: usize = 40;
/// Up to this many inputs it stays with the single pass as before.
pub const SINGLE_INPUTS: usize = 150;
/// Hard limit of the Windows command line (characters).
pub const HARD_CMD: usize = 32_000;

/// Characters of the command line (arguments separated by spaces, quoted if needed).
pub fn cmd_len(args: &[String]) -> usize {
    args.iter().map(|a| a.len() + 3).sum()
}

/// Characters of one input (`-ss x -t y -i <path>` or `-loop 1 -t y -i <path>`), generously.
fn input_len(m: &MediaRef) -> usize {
    m.path.as_os_str().len() + 48
}

/// Part of the timeline that costs inputs: a transition group, a clip or an audio clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub start: Time,
    pub end: Time,
    pub inputs: usize,
    pub chars: usize,
    /// Audio (own FFmpeg per section) instead of picture
    pub audio: bool,
}

/// Inputs of the export of `start..end` as [`graph::video_part`] and [`graph::audio_part`]
/// create them (picture: visible tracks, transition groups as a whole; audio: audible clips).
pub(crate) fn units(p: &Project, start: Time, end: Time, video: bool, audio: bool) -> Vec<Unit> {
    let mut v = Vec::new();
    if video {
        for tr in p.timeline.tracks.iter().filter(|t| !t.hidden && !matches!(t.kind, TrackKind::Effect | TrackKind::Filter)) {
            for g in graph::transition_groups(tr) {
                let (a, b) = (g[0].start, g.last().expect("not empty").end());
                if b <= start || a >= end {
                    continue;
                }
                let mut u = Unit { start: a, end: b, inputs: 0, chars: 0, audio: false };
                for c in g {
                    let ClipContent::Media { media, kind: kind @ (MediaKind::Video | MediaKind::Image) } = &c.content else { continue };
                    let Some(m) = p.media(*media) else { continue };
                    u.inputs += 1;
                    u.chars += input_len(m);
                    if let Some((mp, _)) = graph::matte_input(c, *kind == MediaKind::Image) {
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

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn lcm(a: u128, b: u128) -> u128 {
    a / gcd(a, b) * b
}

/// Grid of the section boundaries: smallest frame count that gives whole microseconds and whole samples
/// (48 kHz) (60 fps: 3 frames = 50 ms; 29.97 fps: 15 frames ≈ 0.5 s). With odd rates
/// that would be huge (12.345 fps: 2469 frames ≈ 200 s): then one frame, rounded to µs (the rest
/// in the audio is evened out by `append_exact`).
pub fn grid_step(fps: Rational) -> Time {
    let (num, den) = (fps.num.max(1) as u128, fps.den.max(1) as u128);
    let need = |rate: u128| num / gcd(num, den * rate);
    let step = lcm(need(1_000_000), need(48_000)) * den * 1_000_000 / num;
    if step > 1_000_000 {
        Time(((den * 1_000_000 + num / 2) / num).max(1) as i64)
    } else {
        Time(step as i64)
    }
}

/// Limits for a section.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub inputs: usize,
    pub chars: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { inputs: MAX_INPUTS, chars: MAX_CMD };
}

/// (inputs, characters) of the units touching `a..b` – per FFmpeg (picture and audio run
/// separately), the larger value counts.
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

/// Does the whole range fit in one pass (one FFmpeg for picture and audio together)?
pub fn single_pass(units: &[Unit], start: Time, end: Time, base: usize, lim: Limits) -> bool {
    let (i, c) = units
        .iter()
        .take_while(|u| u.start < end)
        .filter(|u| u.end > start)
        .fold((0, 0), |(i, c), u| (i + u.inputs, c + u.chars));
    i <= lim.inputs && c + base <= lim.chars
}

/// Splits `start..end` into sections that each cost at most `lim` (`base` = characters without
/// inputs). Boundaries lie on `start + k·step`; within the rear half of the possible
/// section the position is chosen where the least is cut through (preferably a
/// cut of all tracks; transition groups count with their inputs). `Err(t)`: even one
/// grid step from `t` has too many simultaneous clips.
pub fn plan(units: &[Unit], start: Time, end: Time, step: Time, base: usize, lim: Limits) -> std::result::Result<Vec<(Time, Time)>, Time> {
    let step = Time(step.0.max(1));
    let at = |k: i64| Time(start.0 + k * step.0);
    let mut out = Vec::new();
    let mut ka = 0i64;
    // last grid point before the end
    let k_end = (end.0 - start.0 - 1) / step.0;
    loop {
        let a = at(ka);
        // units from here (sorted by start; earlier ones that already end drop out)
        let first = units.partition_point(|u| u.start < a);
        let back = units[..first].iter().filter(|u| u.end > a).copied();
        let local: Vec<Unit> = back.chain(units[first..].iter().copied()).collect();
        if fits(&local, a, end, base, lim) {
            out.push((a, end));
            return Ok(out);
        }
        // largest k with a fitting section (cost grows with k)
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
        // preferred boundary in the rear half: fewest inputs reaching across the boundary
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

/// Time as `m:ss` or `h:mm:ss` (for error texts).
pub fn clock(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Free space on the drive of `dir` in bytes (`None`: unknown).
#[cfg(windows)]
pub fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let w: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: null-terminated path, valid pointers to local variables
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some(avail)
}

/// shortcut: no check outside Windows (statvfs would need libc); add it if it is missing there
#[cfg(not(windows))]
pub fn free_space(_dir: &std::path::Path) -> Option<u64> {
    None
}

/// Size in readable form (MB/GB).
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
        // broken rate: no crash, positive step
        assert!(grid_step(Rational::new(0, 0)).0 > 0);
    }

    /// Odd rates (import, project file, MCP): no overflow, step of one frame instead of minutes.
    #[test]
    fn odd_rates_fall_back_to_whole_frames() {
        assert_eq!(grid_step(Rational::new(12345, 1000)), Time(81_004), "1 frame instead of 2469");
        assert_eq!(grid_step(Rational::new(1_000_000_007, 1_000_000)), Time(1_000));
        assert!(grid_step(Rational::new(u32::MAX, 1)).0 > 0);
        assert_eq!(grid_step(Rational::new(1, u32::MAX)).0, u32::MAX as i64 * 1_000_000);
    }

    #[test]
    fn small_projects_stay_one_pass() {
        let units = vec![u(0.0, 1.0, 1), u(1.0, 2.0, 1)];
        assert!(single_pass(&units, s(0.0), s(2.0), 500, LIM));
        assert_eq!(plan(&units, s(0.0), s(2.0), s(0.05), 500, LIM).unwrap(), vec![(s(0.0), s(2.0))]);
    }

    /// Checks a split: gapless, on the grid, every section within the limits.
    fn check(units: &[Unit], start: Time, end: Time, step: Time, plan: &[(Time, Time)], lim: Limits) {
        assert_eq!(plan.first().unwrap().0, start);
        assert_eq!(plan.last().unwrap().1, end);
        for w in plan.windows(2) {
            assert_eq!(w[0].1, w[1].0, "gapless");
        }
        for (a, b) in plan {
            assert!(a < b);
            assert_eq!((a.0 - start.0) % step.0, 0, "Raster");
            let (i, c) = cost(units, *a, *b);
            assert!(i <= lim.inputs && c <= lim.chars, "{a:?}..{b:?}: {i} inputs");
        }
    }

    #[test]
    fn cuts_prefer_places_without_running_clips() {
        // two tracks with cuts at 1 s, 2 s …; one track is offset (cuts at x.5)
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
        // boundaries lie on cuts of both dense tracks (whole seconds)
        for (_, b) in &p[..p.len() - 1] {
            assert_eq!(b.0 % 1_000_000, 0, "{b:?}");
        }
    }

    #[test]
    fn long_and_short_clips_and_gaps() {
        // long background (whole length), many very short clips, an empty stretch in between
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
        // the empty stretch is not split needlessly
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
        // 12 tracks stacked from 5 s, limit 10 inputs
        let mut units: Vec<Unit> = (0..12).map(|_| u(5.0, 6.0, 1)).collect();
        units.insert(0, u(0.0, 5.0, 1));
        let e = plan(&units, s(0.0), s(6.0), s(0.05), 0, LIM).unwrap_err();
        assert_eq!(e, s(5.0));
    }

    #[test]
    fn long_paths_keep_every_section_below_the_limit() {
        // 2500 clips on 4 tracks with 260-character paths: characters limit before inputs
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
