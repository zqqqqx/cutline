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
    /// Größte Lage/Länge für Clips (~35 Jahre): Summen vieler Clips laufen nicht über.
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
    /// Rastet auf das nächste Frame-Raster der Bildrate ein.
    pub fn snap_to_frame(self, fps: Rational) -> Self {
        // Bildrate 0 (kaputte Projektdatei) würde jede Zeit auf 0 ziehen
        if fps.num == 0 {
            return self;
        }
        let frame = (self.as_secs_f64() * fps.as_f64()).round();
        Time::from_secs_f64(frame / fps.as_f64())
    }
    /// Ganze Sekunden und das Bild darin, aufs nächste Bild gerundet – eine Regel für alle
    /// Zeitanzeigen (Zeitcode, Inspektor), sonst lagen sie um ein Bild auseinander.
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

    /// Zeitcode `HH:MM:SS:FF`.
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
        // sättigend: riesige Zeiten aus Dateien/MCP dürfen nicht überlaufen (Debug: Absturz)
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
    /// Parst `30`, `29.97`, `30000/1001`, `12.5`. Null, negative und unendliche Raten → `None`.
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
        // knapp vor einem Bild (Gleitkomma): aufs nächste Bild, mit Übertrag in die Sekunde
        let f30 = Rational::new(30, 1);
        assert_eq!(Time::from_secs_f64(3.0 + 19.0 / 30.0 - 1e-6).timecode(f30), "00:00:03:19");
        assert_eq!(Time::from_secs_f64(3.0 + 29.6 / 30.0).secs_frames(f30), (4, 0));
    }
}
