use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

/// Point in time or duration in microseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);
    /// Largest position/length for clips (~35 years): sums of many clips don't overflow.
    pub const LIMIT: Time = Time(1 << 50);

    pub fn from_secs_f64(s: f64) -> Self {
        Time((s * 1_000_000.0).round() as i64)
    }
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }
    pub fn from_millis(ms: i64) -> Self {
        Time(ms * 1000)
    }
    pub fn as_millis(self) -> i64 {
        self.0 / 1000
    }
    /// Snaps to the nearest frame grid of the frame rate.
    pub fn snap_to_frame(self, fps: Rational) -> Self {
        // frame rate 0 (broken project file) would pull every time to 0
        if fps.num == 0 {
            return self;
        }
        let frame = (self.as_secs_f64() * fps.as_f64()).round();
        Time::from_secs_f64(frame / fps.as_f64())
    }
    /// First frame `n` whose time (in whole µs, as [`Time::snap_to_frame`] places it) is at or
    /// after `self`. Coverage rule of preview and export: a clip `[start, end)` shows on frames
    /// `start.first_frame_from(fps)..end.first_frame_from(fps)`, so edge-to-edge clips share no
    /// frame and leave none out.
    pub fn first_frame_from(self, fps: Rational) -> i64 {
        if fps.num == 0 || fps.den == 0 {
            return 0;
        }
        let at = |n: i64| Time::from_secs_f64(n as f64 / fps.as_f64());
        let mut n = (self.as_secs_f64() * fps.as_f64()).ceil() as i64;
        while at(n - 1) >= self {
            n -= 1;
        }
        while at(n) < self {
            n += 1;
        }
        n
    }
    /// Whole seconds and the frame within, rounded to the nearest frame – one rule for all
    /// time displays (timecode, inspector), otherwise they were one frame apart.
    pub fn secs_frames(self, fps: Rational) -> (i64, i64) {
        let total = self.as_secs_f64().max(0.0);
        let fps_f = fps.as_f64();
        let mut secs = total.floor() as i64;
        let mut frames = ((total - secs as f64) * fps_f).round() as i64;
        if frames >= (fps_f.round() as i64).max(1) {
            secs += 1;
            frames = 0;
        }
        (secs, frames)
    }

    /// Timecode `HH:MM:SS:FF`.
    pub fn timecode(self, fps: Rational) -> String {
        let (secs, frames) = self.secs_frames(fps);
        format!(
            "{:02}:{:02}:{:02}:{:02}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60,
            frames
        )
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, o: Time) -> Time {
        // saturating: huge times from files/MCP must not overflow (debug: crash)
        Time(self.0.saturating_add(o.0))
    }
}
impl Sub for Time {
    type Output = Time;
    fn sub(self, o: Time) -> Time {
        Time(self.0.saturating_sub(o.0))
    }
}
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.as_secs_f64())
    }
}

/// Exact frame rate, e.g. 30000/1001 for 29.97 fps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }
    /// Parses `30`, `29.97`, `30000/1001`, `12.5`. Zero, negative and infinite rates → `None`.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some((n, d)) = s.split_once('/') {
            let (n, d) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            return (d != 0 && n != 0).then_some(Self::new(n, d));
        }
        let f: f64 = s.trim().parse().ok()?;
        if !(f > 0.0 && f < 1e6) {
            return None;
        }
        let ntsc = f * 1001.0 / 1000.0;
        let r = if (f - f.round()).abs() < 1e-6 {
            Self::new(f.round() as u32, 1)
        } else if (ntsc - ntsc.round()).abs() < 0.01 {
            // 23.976, 29.97, 59.94 …
            Self::new((ntsc.round() * 1000.0) as u32, 1001)
        } else {
            Self::new((f * 1000.0).round() as u32, 1000)
        };
        (r.num != 0).then_some(r)
    }
}

impl Default for Rational {
    fn default() -> Self {
        Self::new(30, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_and_parse() {
        let fps = Rational::parse("30000/1001").unwrap();
        assert!((fps.as_f64() - 29.97).abs() < 0.01);
        assert_eq!(
            Rational::parse("29.97").unwrap(),
            Rational::new(30000, 1001)
        );
        assert_eq!(
            Time::from_secs_f64(3661.5).timecode(Rational::new(30, 1)),
            "01:01:01:15"
        );
        // just before a frame (floating point): to the nearest frame, carrying into the second
        let f30 = Rational::new(30, 1);
        assert_eq!(Time::from_secs_f64(3.0 + 19.0 / 30.0 - 1e-6).timecode(f30), "00:00:03:19");
        assert_eq!(Time::from_secs_f64(3.0 + 29.6 / 30.0).secs_frames(f30), (4, 0));
    }

    /// Clip showing on frame `n` by the preview's rule (start <= frame time < end, frame time in
    /// µs as the playhead has it) – index into `cuts` windows, `None` = nothing.
    fn preview_clip(cuts: &[i64], n: i64, fps: Rational) -> Option<usize> {
        let t = Time::from_secs_f64(n as f64 / fps.as_f64()).0;
        cuts.windows(2).position(|w| w[0] <= t && t < w[1])
    }

    #[test]
    fn coverage_rule_off_grid_cuts() {
        for fps in [Rational::new(30, 1), Rational::new(30000, 1001), Rational::new(60000, 1001)] {
            let fd = 1e6 / fps.as_f64();
            // cuts on the grid (µs rounded up and down), a few µs and up to almost a frame off it
            let mut cuts = vec![0i64];
            for (k, off) in [0.0, 3.0, -3.0, 0.25 * fd, 0.5 * fd, -0.5 * fd, 0.75 * fd, 0.999 * fd, 1.0].iter().enumerate() {
                cuts.push(((5 * k + 6) as f64 * fd + off).round() as i64);
            }
            cuts.push(Time::from_secs_f64(2.0).0);
            let spans: Vec<(i64, i64)> = cuts.windows(2).map(|w| (Time(w[0]).first_frame_from(fps), Time(w[1]).first_frame_from(fps))).collect();
            for n in 0..(2.0 * fps.as_f64()) as i64 {
                let by_span = spans.iter().position(|s| s.0 <= n && n < s.1);
                assert_eq!(by_span, preview_clip(&cuts, n, fps), "frame {n} at {fps:?}, cuts {cuts:?}");
                assert!(by_span.is_some(), "frame {n} shows no clip");
            }
        }
        // the failing cut from the export: frame 23 at 30 fps lies at 766_667 µs (exactly 766_666.67)
        let f30 = Rational::new(30, 1);
        assert_eq!(Time(766_667).first_frame_from(f30), 23);
        assert_eq!(Time(766_668).first_frame_from(f30), 24);
        assert_eq!(Time(400_000).first_frame_from(f30), 12);
        assert_eq!(Time(-40_000).first_frame_from(f30), -1);
        assert_eq!(Time(5).first_frame_from(Rational::new(0, 1)), 0);
    }
}
