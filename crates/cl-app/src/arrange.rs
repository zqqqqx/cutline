//! Schnellaktion „Layout“: Splitscreen und Bild-in-Bild für übereinanderliegende Bildclips.
//! Das Ergebnis ist nur Transform + Zuschnitt je Clip (kein eigenes Format im Projekt).

use cl_core::*;

use crate::app::App;

/// Aktionen in Menüreihenfolge (Kennungen aus `keymap::ACTIONS`)
pub const IDS: &[&str] = &[
    "quick_layout_side",
    "quick_layout_stack",
    "quick_layout_big_left",
    "quick_layout_columns",
    "quick_layout_grid",
    "quick_layout_pip_tl",
    "quick_layout_pip_tr",
    "quick_layout_pip_bl",
    "quick_layout_pip_br",
    "quick_layout_reset",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arrange {
    /// 2: links/rechts
    Side,
    /// 2: oben/unten
    Stack,
    /// 3: einer groß links, zwei rechts übereinander
    BigLeft,
    /// 3: drei Spalten
    Columns,
    /// 4: 2×2
    Grid,
    /// 2: oberer Clip klein in der Ecke (0 oben links, 1 oben rechts, 2 unten links, 3 unten rechts)
    Pip(u8),
    Reset,
}

impl Arrange {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id.strip_prefix("quick_layout_")? {
            "side" => Self::Side,
            "stack" => Self::Stack,
            "big_left" => Self::BigLeft,
            "columns" => Self::Columns,
            "grid" => Self::Grid,
            "pip_tl" => Self::Pip(0),
            "pip_tr" => Self::Pip(1),
            "pip_bl" => Self::Pip(2),
            "pip_br" => Self::Pip(3),
            "reset" => Self::Reset,
            _ => return None,
        })
    }

    /// Passt das Layout zu `n` Clips?
    pub fn fits(self, n: usize) -> bool {
        match self {
            Self::Side | Self::Stack | Self::Pip(_) => n == 2,
            Self::BigLeft | Self::Columns => n == 3,
            Self::Grid => n == 4,
            Self::Reset => n >= 1,
        }
    }
}

/// Zellen (x, y, b, h) in Projektpixeln, Ursprung oben links; Reihenfolge = untere Spur zuerst
fn cells(a: Arrange, w: f32, h: f32) -> Vec<[f32; 4]> {
    let (w2, h2, w3) = (w / 2.0, h / 2.0, w / 3.0);
    match a {
        Arrange::Side => vec![[0.0, 0.0, w2, h], [w2, 0.0, w2, h]],
        Arrange::Stack => vec![[0.0, 0.0, w, h2], [0.0, h2, w, h2]],
        Arrange::BigLeft => vec![[0.0, 0.0, w2, h], [w2, 0.0, w2, h2], [w2, h2, w2, h2]],
        Arrange::Columns => (0..3).map(|i| [w3 * i as f32, 0.0, w3, h]).collect(),
        Arrange::Grid => (0..4).map(|i| [w2 * (i % 2) as f32, h2 * (i / 2) as f32, w2, h2]).collect(),
        Arrange::Pip(_) | Arrange::Reset => vec![[0.0, 0.0, w, h]],
    }
}

/// Quelle `sw`×`sh` füllt die Zelle: bedecken (gleicher Faktor in x und y), mittig zuschneiden.
/// Skalierung 1 = ins Projekt `pw`×`ph` eingepasst (wie Player und Export).
fn fill(pw: f32, ph: f32, (sw, sh): (f32, f32), [x, y, cw, ch]: [f32; 4]) -> (Transform, Crop) {
    let fit = (pw / sw).min(ph / sh);
    let k = (cw / sw).max(ch / sh);
    let (cx, cy) = ((1.0 - cw / (k * sw)) / 2.0, (1.0 - ch / (k * sh)) / 2.0);
    (
        Transform { x: x + cw / 2.0 - pw / 2.0, y: y + ch / 2.0 - ph / 2.0, scale: k / fit, rotation_deg: 0.0 },
        Crop { left: cx, right: cx, top: cy, bottom: cy },
    )
}

/// Quelle `sw`×`sh` klein in einer Ecke des Projekts `pw`×`ph` (0 oben links, 1 oben rechts,
/// 2 unten links, 3 unten rechts): Breite `share` × Projektbreite, Rand 4 % der kürzeren Seite.
/// Skalierung 1 = eingepasst (wie Player und Export).
pub fn in_corner(pw: f32, ph: f32, (sw, sh): (f32, f32), share: f32, corner: u8) -> Transform {
    let (sw, sh) = (sw.max(1.0), sh.max(1.0));
    let (w, m) = (share * pw, 0.04 * pw.min(ph));
    let h = w * sh / sw;
    let x = if corner % 2 == 0 { m } else { pw - m - w };
    let y = if corner < 2 { m } else { ph - m - h };
    let fit = (pw / sw).min(ph / sh);
    Transform { x: x + w / 2.0 - pw / 2.0, y: y + h / 2.0 - ph / 2.0, scale: w / sw / fit, rotation_deg: 0.0 }
}

/// Geometrie eines Layouts: Projekt `pw`×`ph`, angezeigte Quellgrößen (untere Spur zuerst) →
/// je Clip (Transform, Zuschnitt). Passt die Zahl nicht, leer.
pub fn arrange(pw: f32, ph: f32, sizes: &[(f32, f32)], a: Arrange) -> Vec<(Transform, Crop)> {
    if !a.fits(sizes.len()) {
        return Vec::new();
    }
    match a {
        Arrange::Reset => vec![(Transform::default(), Crop::default()); sizes.len()],
        Arrange::Pip(corner) => {
            // unten bildfüllend (Überstand liegt außerhalb des Bilds, kein Zuschnitt nötig)
            let back = (fill(pw, ph, sizes[0], [0.0, 0.0, pw, ph]).0, Crop::default());
            vec![back, (in_corner(pw, ph, sizes[1], 0.28, corner), Crop::default())]
        }
        _ => cells(a, pw, ph).into_iter().zip(sizes).map(|(c, s)| fill(pw, ph, *s, c)).collect(),
    }
}

impl App {
    /// Clips für ein Layout mit angezeigter Quellgröße (gedrehte Videos sind beim Einlesen
    /// schon getauscht): ausgewählte Bildclips, sonst alle am Playhead; untere Spur zuerst.
    pub fn layout_clips(&self) -> Vec<(ClipId, (f32, f32))> {
        let sel = self.selection();
        let pick = |only_sel: bool| -> Vec<(ClipId, (f32, f32))> {
            let mut v = Vec::new();
            for t in self.project.timeline.tracks.iter().filter(|t| !t.hidden) {
                for c in t.clips.iter().filter(|c| !c.disabled) {
                    let ClipContent::Media { media, kind: MediaKind::Video | MediaKind::Image } = &c.content else {
                        continue;
                    };
                    let Some(m) = self.project.media(*media).filter(|m| m.width > 0 && m.height > 0) else {
                        continue;
                    };
                    let hit = if only_sel { sel.contains(&c.id) } else { c.start <= self.playhead && self.playhead < c.end() };
                    if hit {
                        v.push((c.id, (m.width as f32, m.height as f32)));
                    }
                }
            }
            v
        };
        let v = pick(true);
        if v.len() >= 2 { v } else { pick(false) }
    }

    /// Layout `id` anwenden: ein Verlaufsschritt; Keyframes auf Position, Größe und Drehung
    /// fallen weg, weil das Layout für den ganzen Clip gilt.
    pub fn tool_layout(&mut self, id: &str) {
        let Some(a) = Arrange::from_id(id) else {
            return;
        };
        let clips = self.layout_clips();
        let sizes: Vec<(f32, f32)> = clips.iter().map(|c| c.1).collect();
        let (pw, ph) = (self.project.settings.width as f32, self.project.settings.height as f32);
        let geo = arrange(pw, ph, &sizes, a);
        if geo.is_empty() {
            return;
        }
        let mut ops = Vec::new();
        for ((clip, _), (transform, crop)) in clips.iter().zip(geo) {
            ops.push(EditOp::SetTransform { clip: *clip, transform });
            ops.push(EditOp::SetCrop { clip: *clip, crop });
            if let Some((_, c)) = self.project.find_clip(*clip) {
                let mut keyframes = c.keyframes.clone();
                for p in [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation] {
                    keyframes.0.remove(&p);
                }
                if keyframes != c.keyframes {
                    ops.push(EditOp::SetKeyframes { clip: *clip, keyframes });
                }
            }
        }
        let label = if a == Arrange::Reset { "Layout zurücksetzen" } else { "Layout" };
        self.apply(EditOp::Batch { label: label.into(), ops });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTS: [(f32, f32); 3] = [(1920.0, 1080.0), (1080.0, 1920.0), (1080.0, 1080.0)];
    const SOURCES: [(f32, f32); 3] = [(1920.0, 1080.0), (1080.0, 1920.0), (640.0, 480.0)];

    /// Sichtbares Rechteck (x0, y0, x1, y1) im Projekt und Faktoren je Achse, wie Player/Export
    /// rechnen: eingepasst × Skalierung, Mitte + Position, Zuschnitt an seiner Stelle.
    fn shown(pw: f32, ph: f32, (sw, sh): (f32, f32), t: &Transform, k: &Crop) -> ([f32; 4], f32, f32) {
        let fit = (pw / sw).min(ph / sh) * t.scale;
        let (w, h) = (sw * fit, sh * fit);
        let (x0, y0) = (pw / 2.0 + t.x - w / 2.0, ph / 2.0 + t.y - h / 2.0);
        let r = [x0 + k.left * w, y0 + k.top * h, x0 + (1.0 - k.right) * w, y0 + (1.0 - k.bottom) * h];
        (r, w / sw, h / sh)
    }

    fn near(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1.0)
    }

    #[test]
    fn splitscreen_cells_are_covered_exactly_without_distortion() {
        let srcs = |n: usize, i: usize| (0..n).map(|j| SOURCES[(i + j) % 3]).collect::<Vec<_>>();
        for (pw, ph) in PROJECTS {
            for a in [Arrange::Side, Arrange::Stack, Arrange::BigLeft, Arrange::Columns, Arrange::Grid] {
                let n = cells(a, pw, ph).len();
                // die Zellen füllen das Bild lückenlos
                let area: f32 = cells(a, pw, ph).iter().map(|c| c[2] * c[3]).sum();
                assert!((area - pw * ph).abs() < 1.0, "{a:?} in {pw}×{ph}");
                for i in 0..3 {
                    let sizes = srcs(n, i);
                    let geo = arrange(pw, ph, &sizes, a);
                    assert_eq!(geo.len(), n);
                    for ((s, (t, k)), c) in sizes.iter().zip(&geo).zip(cells(a, pw, ph)) {
                        let (r, fx, fy) = shown(pw, ph, *s, t, k);
                        assert!(near(r, [c[0], c[1], c[0] + c[2], c[1] + c[3]]), "{a:?} {pw}×{ph} Quelle {s:?}: {r:?} statt {c:?}");
                        assert!((fx - fy).abs() < 1e-4, "verzerrt");
                        assert_eq!(t.rotation_deg, 0.0);
                        // Zuschnitt bleibt unter der Grenze, die `Crop::clamped` erzwingt
                        assert_eq!(k.clamped(), *k);
                    }
                }
            }
        }
    }

    #[test]
    fn picture_in_picture_sits_in_the_corner_and_back_fills_the_frame() {
        for (pw, ph) in PROJECTS {
            for back in SOURCES {
                for top in SOURCES {
                    for corner in 0..4u8 {
                        let geo = arrange(pw, ph, &[back, top], Arrange::Pip(corner));
                        let (b, _, _) = shown(pw, ph, back, &geo[0].0, &geo[0].1);
                        assert!(b[0] <= 0.5 && b[1] <= 0.5 && b[2] >= pw - 0.5 && b[3] >= ph - 0.5, "Hintergrund deckt nicht: {b:?}");
                        // eine Achse liegt genau auf dem Bildrand: nicht größer als nötig
                        assert!(near([b[0], b[2], 0.0, 0.0], [0.0, pw, 0.0, 0.0]) || near([b[1], b[3], 0.0, 0.0], [0.0, ph, 0.0, 0.0]), "{b:?}");
                        let (r, fx, fy) = shown(pw, ph, top, &geo[1].0, &geo[1].1);
                        assert!(geo[1].1.is_none() && (fx - fy).abs() < 1e-4);
                        assert!(((r[2] - r[0]) - 0.28 * pw).abs() < 0.5);
                        let m = 0.04 * pw.min(ph);
                        let (dx, dy) = if corner % 2 == 0 { (r[0], 0.0) } else { (pw - r[2], 0.0) };
                        let dy = dy + if corner < 2 { r[1] } else { ph - r[3] };
                        assert!((dx - m).abs() < 0.5 && (dy - m).abs() < 0.5, "Ecke {corner} {pw}×{ph}: {r:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_need_the_right_number_of_clips() {
        let s = (1920.0, 1080.0);
        assert!(arrange(1920.0, 1080.0, &[s], Arrange::Side).is_empty());
        assert!(arrange(1920.0, 1080.0, &[s; 5], Arrange::Grid).is_empty());
        assert!(arrange(1920.0, 1080.0, &[s; 3], Arrange::Pip(0)).is_empty());
        assert_eq!(arrange(1920.0, 1080.0, &[s], Arrange::Reset), vec![(Transform::default(), Crop::default())]);
        assert!(IDS.iter().all(|id| Arrange::from_id(id).is_some() && !crate::keymap::name(id).is_empty()));
    }

    /// Anwenden ist ein Verlaufsschritt (samt entfernten Keyframes), Rückgängig stellt alles her.
    #[test]
    fn layout_is_one_undo_step() {
        let (mut app, _home, _g) = crate::saving::tests::test_app();
        let mut p = Project::new("Layout");
        (p.settings.width, p.settings.height) = (1920, 1080);
        let sec = Time::from_secs_f64;
        let m = MediaRef {
            id: MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: sec(10.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mid = m.id;
        p.media.push(m);
        for i in 0..2 {
            let mut t = Track::new(TrackKind::Video);
            let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, sec(0.0), sec(5.0));
            c.keyframes.set(Prop::Scale, sec(1.0), 2.0);
            c.keyframes.set(Prop::Opacity, sec(1.0), 0.5);
            c.transform.rotation_deg = 10.0 * i as f32;
            t.clips.push(c);
            p.timeline.tracks.push(t);
        }
        // Text darüber bleibt unberührt
        let mut tt = Track::new(TrackKind::Text);
        tt.clips.push(Clip::new(ClipContent::Text(TextStyle::default()), sec(0.0), sec(5.0)));
        p.timeline.tracks.push(tt);
        app.set_doc(p);
        app.playhead = sec(1.0);
        let before = app.project.clone();
        let n0 = app.doc.lock().unwrap().undo_entries().len();
        assert!(app.quick_check("quick_layout_side").0 && !app.quick_check("quick_layout_grid").0);
        app.tool_layout("quick_layout_side");
        assert_eq!(app.doc.lock().unwrap().undo_entries().len(), n0 + 1);
        let clips: Vec<Clip> = app.project.timeline.tracks.iter().flat_map(|t| t.clips.clone()).collect();
        assert_eq!(clips[0].transform.x, -480.0);
        assert_eq!(clips[1].transform.x, 480.0);
        assert!(clips[..2].iter().all(|c| c.crop.left == 0.25 && c.transform.rotation_deg == 0.0));
        assert!(clips[..2].iter().all(|c| c.keyframes.get(Prop::Scale).is_empty() && !c.keyframes.get(Prop::Opacity).is_empty()));
        assert_eq!(clips[2], before.timeline.tracks[2].clips[0]);
        app.undo();
        app.sync_project();
        assert_eq!(app.project.timeline, before.timeline);
    }
}
