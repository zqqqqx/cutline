use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

/// Zeitpunkt bzw. Dauer in Mikrosekunden.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);

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
    /// Rastet auf das nächste Frame-Raster der Bildrate ein.
    pub fn snap_to_frame(self, fps: Rational) -> Self {
        let frame = (self.as_secs_f64() * fps.as_f64()).round();
        Time::from_secs_f64(frame / fps.as_f64())
    }
    /// Zeitcode `HH:MM:SS:FF`.
    pub fn timecode(self, fps: Rational) -> String {
        let total = self.as_secs_f64().max(0.0);
        let fps_f = fps.as_f64();
        let secs = total.floor() as i64;
        let frames = ((total - secs as f64) * fps_f).floor() as i64;
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
        Time(self.0 + o.0)
    }
}
impl Sub for Time {
    type Output = Time;
    fn sub(self, o: Time) -> Time {
        Time(self.0 - o.0)
    }
}
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.as_secs_f64())
    }
}

/// Exakte Bildrate, z. B. 30000/1001 für 29,97 fps.
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
    /// Parst `30`, `29.97`, `30000/1001`.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some((n, d)) = s.split_once('/') {
            let (n, d) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            return (d != 0).then_some(Self::new(n, d));
        }
        let f: f64 = s.trim().parse().ok()?;
        if (f - f.round()).abs() < 1e-6 {
            Some(Self::new(f.round() as u32, 1))
        } else {
            Some(Self::new((f * 1001.0).round() as u32, 1001))
        }
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
    }
}
