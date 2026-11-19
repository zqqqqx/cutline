//! Kreativ-Bibliothek von Cutline (Phase 2).
//!
//! Alles ist als FFmpeg-Filterkette beschrieben. Dieselbe Kette läuft in der Vorschau
//! (Einzelbild und Wiedergabe-Strom) und im Export, dadurch sieht der Export genau so
//! aus wie der Player. Jeder Katalog-Eintrag wird in den Tests gegen FFmpeg ausgeführt.
//!
//! - [`EFFECTS`]: Video-Effekte (Retro, Bewegung, Licht, Verzerrung, Stil)
//! - [`FILTERS`]: Farb-Looks; Stärke über Überblendung mit dem Original
//! - [`TRANSITIONS`]: Übergänge (FFmpeg `xfade`)
//! - [`fx_graph`]: komplette Clip-Kette aus [`ClipFx`] (Spiegeln, Anpassen, HSL, Filter,
//!   Effekte, LUT, Maske)
//! - [`text`]: Textvorlagen und Texteffekte, [`stickers`]: SVG-Sticker, [`sfx`]: Soundeffekte

pub mod audio;
pub mod catalog;
pub mod sfx;
pub mod stickers;
pub mod text;
pub mod xfade;

use cl_core::*;

/// Eigenschaften des Bildes, das in die Kette läuft.
#[derive(Debug, Clone, Copy)]
pub struct Src {
    pub w: u32,
    pub h: u32,
    pub has_alpha: bool,
}

/// Ein Katalog-Eintrag: `build(stärke 0…1, quelle)` liefert eine einfache Filterkette
/// (mit Kommas verbunden, ohne Labels).
pub struct FxDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    /// Die Kette kann ihre Stärke selbst abbilden; sonst wird mit dem Original überblendet.
    pub native_intensity: bool,
    pub build: fn(f32, Src) -> String,
}

impl std::fmt::Debug for FxDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FxDef({})", self.id)
    }
}

fn ev(w: u32) -> u32 {
    (w.max(2)) & !1
}

pub const EFFECT_CATEGORIES: [&str; 5] = ["Basis", "Retro", "Bewegung", "Licht", "Stil"];

pub static EFFECTS: &[FxDef] = &[
    // Basis
    FxDef {
        id: "blur",
        name: "Unschärfe",
        category: "Basis",
        native_intensity: true,
        build: |i, _| format!("gblur=sigma={:.2}", 1.0 + 14.0 * i),
    },
    FxDef {
        id: "sharpen",
        name: "Schärfen",
        category: "Basis",
        native_intensity: true,
        build: |i, _| format!("unsharp=5:5:{:.2}", 0.4 + 2.0 * i),
    },
    FxDef {
        id: "bw",
        name: "Schwarzweiß",
        category: "Basis",
        native_intensity: true,
        build: |i, _| format!("hue=s={:.3}", 1.0 - i),
    },
    FxDef {
        id: "invert",
        name: "Negativ",
        category: "Basis",
        native_intensity: false,
        build: |_, _| "negate".into(),
    },
    FxDef {
        id: "pixelate",
        name: "Verpixeln",
        category: "Basis",
        native_intensity: true,
        build: |i, s| {
            let f = 4.0 + 36.0 * i;
            let (w, h) = (
                ((s.w as f32 / f).round() as u32).max(2),
                ((s.h as f32 / f).round() as u32).max(2),
            );
            format!(
                "scale={w}:{h}:flags=area,scale={}:{}:flags=neighbor",
                s.w, s.h
            )
        },
    },
    FxDef {
        id: "vignette",
        name: "Vignette",
        category: "Basis",
        native_intensity: true,
        build: |i, _| format!("vignette=angle={:.3}", 0.2 + 0.9 * i),
    },
    FxDef {
        id: "grain",
        name: "Filmkorn",
        category: "Basis",
        native_intensity: true,
        build: |i, _| format!("noise=alls={:.0}:allf=t", 6.0 + 34.0 * i),
    },
    FxDef {
        id: "edges",
        name: "Konturen",
        category: "Basis",
        native_intensity: false,
        build: |_, _| "edgedetect=mode=colormix:high=0.2:low=0.05".into(),
    },
    // Retro
    FxDef {
        id: "vhs",
        name: "VHS",
        category: "Retro",
        native_intensity: false,
        // Pixelmaße auf 1080 Zeilen bezogen: die Vorschau rechnet die Kette auf ihrem kleineren
        // Bild, sonst wären Farbversatz und Zeilen dort viel kräftiger als im Export
        build: |_, s| {
            let k = s.w.min(s.h).max(1) as f32 / 1080.0;
            let shift = (5.0 * k).round() as i32;
            // Zeilen: ¹⁄₁₈ Abdunklung im Mittel; zu klein für 1-px-Linien → gleichmäßig
            let lines = if 3.0 * k >= 2.0 {
                let (t, h) = ((k.round() as i32).max(1), (3.0 * k).round() as i32);
                format!("drawgrid=w=0:h={h}:t={t}:c=black@{:.3}", 0.06 * h as f32 / t as f32)
            } else {
                "drawbox=c=black@0.06:t=fill".into()
            };
            format!(
                "rgbashift=rh={}:bh={shift}:gv={},noise=alls=18:allf=t,eq=saturation=1.35:contrast=1.08,gblur=sigma={:.2},{lines}",
                -shift,
                k.round() as i32,
                (0.7 * k).max(0.1)
            )
        },
    },
    FxDef {
        id: "old-film",
        name: "Alter Film",
        category: "Retro",
        native_intensity: false,
        build: |_, _| {
            "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131,noise=alls=24:allf=t,vignette=angle=0.9,eq=brightness='0.04*sin(t*37)':eval=frame".into()
        },
    },
    FxDef {
        id: "sepia",
        name: "Sepia",
        category: "Retro",
        native_intensity: false,
        build: |_, _| "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131".into(),
    },
    FxDef {
        id: "scanlines",
        name: "Bildschirmzeilen",
        category: "Retro",
        native_intensity: true,
        build: |i, _| format!("drawgrid=w=0:h=4:t=1:c=black@{:.2}", 0.15 + 0.6 * i),
    },
    FxDef {
        id: "posterize",
        name: "Poster",
        category: "Retro",
        native_intensity: true,
        build: |i, _| {
            let step = (24.0 + 72.0 * i).round();
            format!("lutrgb=r='trunc(val/{step})*{step}':g='trunc(val/{step})*{step}':b='trunc(val/{step})*{step}'")
        },
    },
    FxDef {
        id: "comic",
        name: "Comic",
        category: "Retro",
        native_intensity: false,
        build: |_, _| {
            "lutrgb=r='trunc(val/64)*64':g='trunc(val/64)*64':b='trunc(val/64)*64',unsharp=7:7:2.5,eq=saturation=1.4".into()
        },
    },
    // Bewegung
    FxDef {
        id: "shake",
        name: "Wackeln",
        category: "Bewegung",
        native_intensity: true,
        build: |i, s| {
            let a = 0.03 * i;
            let (cw, ch) = (ev((s.w as f32 * 0.9) as u32), ev((s.h as f32 * 0.9) as u32));
            format!("crop={cw}:{ch}:x='(iw-ow)/2+sin(t*47)*iw*{a:.4}':y='(ih-oh)/2+cos(t*39)*ih*{a:.4}',scale={}:{}", s.w, s.h)
        },
    },
    FxDef {
        id: "zoom-pulse",
        name: "Zoom-Puls",
        category: "Bewegung",
        native_intensity: true,
        build: |i, s| {
            // nach der Zeit des Bildes skalieren und mittig zuschneiden: Bildzahl und Zeitstempel
            // bleiben, wie sie sind (`zoompan` vergäbe eigene mit fester Bildrate)
            let z = format!("(1+{:.3}*(0.5+0.5*sin(t*6.2832)))", 0.15 * i);
            format!("scale=w='2*ceil(iw*{z}/2)':h='2*ceil(ih*{z}/2)':eval=frame,crop={}:{},setsar=1", s.w, s.h)
        },
    },
    FxDef {
        id: "motion-blur",
        name: "Bewegungsunschärfe",
        category: "Bewegung",
        native_intensity: false,
        build: |_, _| "tmix=frames=4:weights='1 1 1 1'".into(),
    },
    FxDef {
        id: "echo",
        name: "Nachzieher",
        category: "Bewegung",
        native_intensity: false,
        build: |_, _| "tmix=frames=8:weights='1 1 1 2 2 3 4 8'".into(),
    },
    FxDef {
        id: "glitch",
        name: "Glitch",
        category: "Bewegung",
        native_intensity: false,
        build: |_, _| {
            "rgbashift=rh=-8:bh=8:gv=3,hue=h='if(lt(mod(t,1.1),0.12),120,0)',noise=alls=20:allf=t+u"
                .into()
        },
    },
    FxDef {
        id: "rainbow",
        name: "Regenbogen",
        category: "Bewegung",
        native_intensity: true,
        build: |i, _| format!("hue=h='t*{:.0}'", 60.0 + 240.0 * i),
    },
    // Licht
    FxDef {
        id: "strobe",
        name: "Stroboskop",
        category: "Licht",
        native_intensity: true,
        build: |i, _| {
            format!(
                "eq=brightness='{:.3}*gt(mod(t,0.25),0.125)':eval=frame",
                0.15 + 0.45 * i
            )
        },
    },
    FxDef {
        id: "flash",
        name: "Blitz",
        category: "Licht",
        native_intensity: true,
        build: |i, _| {
            format!(
                "eq=brightness='max(0,{:.3}*(1-t*3))':eval=frame",
                0.3 + 0.6 * i
            )
        },
    },
    FxDef {
        id: "dreamy",
        name: "Verträumt",
        category: "Licht",
        native_intensity: false,
        build: |_, _| {
            "gblur=sigma=3,eq=brightness=0.05:contrast=0.92:saturation=1.15,colortemperature=temperature=5600".into()
        },
    },
    FxDef {
        id: "glow-edges",
        name: "Leuchtkanten",
        category: "Licht",
        native_intensity: false,
        build: |_, _| {
            "edgedetect=mode=colormix:high=0.15:low=0.04,eq=saturation=2.2:brightness=0.06,gblur=sigma=1.2".into()
        },
    },
    FxDef {
        id: "warm-leak",
        name: "Lichteinfall",
        category: "Licht",
        native_intensity: false,
        build: |_, _| {
            "colorbalance=rh=0.25:gh=0.08:rm=0.12,eq=brightness='0.06+0.05*sin(t*1.7)':eval=frame"
                .into()
        },
    },
    FxDef {
        id: "chroma",
        name: "Farbversatz",
        category: "Licht",
        native_intensity: true,
        build: |i, _| {
            let px = (2.0 + 10.0 * i).round() as i32;
            format!("rgbashift=rh=-{px}:bh={px}")
        },
    },
    // Stil
    FxDef {
        id: "heat",
        name: "Wärmebild",
        category: "Stil",
        native_intensity: false,
        build: |_, _| "format=gbrp,pseudocolor=preset=heat".into(),
    },
    FxDef {
        id: "night-vision",
        name: "Nachtsicht",
        category: "Stil",
        native_intensity: false,
        build: |_, _| {
            "hue=s=0,colorchannelmixer=0.2:0:0:0:0.3:1.2:0.2:0:0.1:0.2:0.1,noise=alls=22:allf=t,vignette=angle=0.8".into()
        },
    },
    FxDef {
        id: "duotone",
        name: "Duoton",
        category: "Stil",
        native_intensity: false,
        build: |_, _| "hue=s=0,colorbalance=rs=0.35:gs=-0.1:bs=0.45:rh=0.4:gh=0.1:bh=-0.25".into(),
    },
    FxDef {
        id: "emboss",
        name: "Relief",
        category: "Stil",
        native_intensity: false,
        build: |_, _| {
            "convolution='-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2'".into()
        },
    },
    FxDef {
        id: "lens",
        name: "Linse",
        category: "Stil",
        native_intensity: true,
        build: |i, _| {
            format!(
                "lenscorrection=k1={:.3}:k2={:.3}",
                -0.15 - 0.35 * i,
                -0.05 - 0.1 * i
            )
        },
    },
    FxDef {
        id: "cinema-bars",
        name: "Kinobalken",
        category: "Stil",
        native_intensity: true,
        build: |i, _| {
            let f = 0.06 + 0.1 * i;
            format!("drawbox=x=0:y=0:w=iw:h=ih*{f:.3}:color=black:t=fill,drawbox=x=0:y=ih*{:.3}:w=iw:h=ih*{f:.3}:color=black:t=fill", 1.0 - f)
        },
    },
];

pub static FILTERS: &[FxDef] = &[
    FxDef {
        id: "natural",
        name: "Natürlich",
        category: "Porträt",
        native_intensity: false,
        build: |_, _| "eq=saturation=1.1:contrast=1.05".into(),
    },
    FxDef {
        id: "vivid",
        name: "Lebendig",
        category: "Porträt",
        native_intensity: false,
        build: |_, _| "eq=saturation=1.45:contrast=1.1,vibrance=intensity=0.3".into(),
    },
    FxDef {
        id: "pastel",
        name: "Pastell",
        category: "Porträt",
        native_intensity: false,
        build: |_, _| "eq=saturation=0.7:brightness=0.05,curves=all='0/0.1 1/1'".into(),
    },
    FxDef {
        id: "bright",
        name: "Hell",
        category: "Porträt",
        native_intensity: false,
        build: |_, _| "curves=preset=lighter".into(),
    },
    FxDef {
        id: "warm",
        name: "Warm",
        category: "Landschaft",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=4600".into(),
    },
    FxDef {
        id: "cool",
        name: "Kühl",
        category: "Landschaft",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=9000".into(),
    },
    FxDef {
        id: "golden",
        name: "Goldene Stunde",
        category: "Landschaft",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=3800,eq=saturation=1.2".into(),
    },
    FxDef {
        id: "forest",
        name: "Wald",
        category: "Landschaft",
        native_intensity: false,
        build: |_, _| "colorbalance=gm=0.12:rs=-0.05:bs=0.03,eq=saturation=1.1".into(),
    },
    FxDef {
        id: "sunset",
        name: "Sonnenuntergang",
        category: "Landschaft",
        native_intensity: false,
        build: |_, _| "colorbalance=rm=0.18:gm=0.02:bm=-0.12:rh=0.1,eq=saturation=1.15".into(),
    },
    FxDef {
        id: "cinematic",
        name: "Kino",
        category: "Film",
        native_intensity: false,
        build: |_, _| {
            "colorbalance=rs=-0.12:bs=0.16:rh=0.14:bh=-0.12,curves=preset=medium_contrast,eq=saturation=0.9".into()
        },
    },
    FxDef {
        id: "teal-orange",
        name: "Teal & Orange",
        category: "Film",
        native_intensity: false,
        build: |_, _| "colorbalance=rs=-0.15:gs=0.02:bs=0.18:rh=0.15:gh=0.03:bh=-0.15".into(),
    },
    FxDef {
        id: "vintage",
        name: "Vintage",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=vintage".into(),
    },
    FxDef {
        id: "cross",
        name: "Crossentwicklung",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=cross_process".into(),
    },
    FxDef {
        id: "matte",
        name: "Matt",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=all='0/0.12 1/0.92',eq=saturation=0.85".into(),
    },
    FxDef {
        id: "bleach",
        name: "Bleach Bypass",
        category: "Film",
        native_intensity: false,
        build: |_, _| "eq=saturation=0.45:contrast=1.3".into(),
    },
    FxDef {
        id: "polaroid",
        name: "Sofortbild",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=vintage,eq=saturation=0.9,vignette=angle=0.5".into(),
    },
    FxDef {
        id: "mono",
        name: "Mono",
        category: "Schwarzweiß",
        native_intensity: false,
        build: |_, _| "hue=s=0,eq=contrast=1.15".into(),
    },
    FxDef {
        id: "noir",
        name: "Noir",
        category: "Schwarzweiß",
        native_intensity: false,
        build: |_, _| "hue=s=0,curves=preset=strong_contrast".into(),
    },
    FxDef {
        id: "silver",
        name: "Silber",
        category: "Schwarzweiß",
        native_intensity: false,
        build: |_, _| "hue=s=0,curves=all='0/0.08 0.5/0.55 1/0.95'".into(),
    },
    FxDef {
        id: "moody",
        name: "Düster",
        category: "Stimmung",
        native_intensity: false,
        build: |_, _| "eq=brightness=-0.05:saturation=0.75,curves=preset=darker".into(),
    },
    FxDef {
        id: "cyber",
        name: "Cyber",
        category: "Stimmung",
        native_intensity: false,
        build: |_, _| "colorbalance=rs=0.1:bs=0.25:rh=0.2:bh=0.2:gm=-0.1,eq=saturation=1.3".into(),
    },
    FxDef {
        id: "contrast",
        name: "Kontrast",
        category: "Stimmung",
        native_intensity: false,
        build: |_, _| "curves=preset=strong_contrast".into(),
    },
];

pub const FILTER_CATEGORIES: [&str; 5] =
    ["Porträt", "Landschaft", "Film", "Schwarzweiß", "Stimmung"];

/// Anzeigename eines Effekts/Filters (eingebaut oder aus einem Paket).
pub fn fx_name(id: &str) -> String {
    effect(id)
        .or_else(|| filter(id))
        .map(|d| d.name.to_string())
        .or_else(|| catalog::name_any(id))
        .unwrap_or_else(|| id.to_string())
}

pub fn effect(id: &str) -> Option<&'static FxDef> {
    EFFECTS.iter().find(|e| e.id == id)
}
pub fn filter(id: &str) -> Option<&'static FxDef> {
    FILTERS.iter().find(|e| e.id == id)
}

/// Übergänge: (id, Name, Kategorie). Die id ist der Name der `xfade`-Variante.
pub static TRANSITIONS: &[(&str, &str, &str)] = &[
    ("fade", "Überblenden", "Basis"),
    ("dissolve", "Auflösen", "Basis"),
    ("fadeblack", "Über Schwarz", "Basis"),
    ("fadewhite", "Über Weiß", "Basis"),
    ("fadegrays", "Über Grau", "Basis"),
    ("fadefast", "Schnell blenden", "Basis"),
    ("fadeslow", "Langsam blenden", "Basis"),
    ("distance", "Abstand", "Basis"),
    ("wipeleft", "Wischen links", "Wischen"),
    ("wiperight", "Wischen rechts", "Wischen"),
    ("wipeup", "Wischen hoch", "Wischen"),
    ("wipedown", "Wischen runter", "Wischen"),
    ("wipetl", "Wischen ↖", "Wischen"),
    ("wipetr", "Wischen ↗", "Wischen"),
    ("wipebl", "Wischen ↙", "Wischen"),
    ("wipebr", "Wischen ↘", "Wischen"),
    ("smoothleft", "Weich links", "Wischen"),
    ("smoothright", "Weich rechts", "Wischen"),
    ("smoothup", "Weich hoch", "Wischen"),
    ("smoothdown", "Weich runter", "Wischen"),
    ("slideleft", "Schieben links", "Bewegung"),
    ("slideright", "Schieben rechts", "Bewegung"),
    ("slideup", "Schieben hoch", "Bewegung"),
    ("slidedown", "Schieben runter", "Bewegung"),
    ("coverleft", "Überdecken links", "Bewegung"),
    ("coverright", "Überdecken rechts", "Bewegung"),
    ("coverup", "Überdecken hoch", "Bewegung"),
    ("coverdown", "Überdecken runter", "Bewegung"),
    ("revealleft", "Aufdecken links", "Bewegung"),
    ("revealright", "Aufdecken rechts", "Bewegung"),
    ("revealup", "Aufdecken hoch", "Bewegung"),
    ("revealdown", "Aufdecken runter", "Bewegung"),
    ("zoomin", "Hineinzoomen", "Bewegung"),
    ("squeezeh", "Stauchen horizontal", "Bewegung"),
    ("hlwind", "Wind links", "Bewegung"),
    ("hrwind", "Wind rechts", "Bewegung"),
    ("vuwind", "Wind hoch", "Bewegung"),
    ("vdwind", "Wind runter", "Bewegung"),
    ("circleopen", "Kreis öffnen", "Form"),
    ("circleclose", "Kreis schließen", "Form"),
    ("circlecrop", "Kreisblende", "Form"),
    ("rectcrop", "Rechteckblende", "Form"),
    ("radial", "Radial", "Form"),
    ("vertopen", "Vertikal öffnen", "Form"),
    ("vertclose", "Vertikal schließen", "Form"),
    ("horzopen", "Horizontal öffnen", "Form"),
    ("horzclose", "Horizontal schließen", "Form"),
    ("diagtl", "Diagonal ↖", "Form"),
    ("diagbr", "Diagonal ↘", "Form"),
    ("diagtr", "Diagonal ↗", "Form"),
    ("diagbl", "Diagonal ↙", "Form"),
    ("hlslice", "Streifen links", "Effekt"),
    ("hrslice", "Streifen rechts", "Effekt"),
    ("vuslice", "Streifen hoch", "Effekt"),
    ("vdslice", "Streifen runter", "Effekt"),
    ("pixelize", "Verpixeln", "Effekt"),
    ("hblur", "Verwischen", "Effekt"),
];

pub const TRANSITION_CATEGORIES: [&str; 5] = ["Basis", "Wischen", "Bewegung", "Form", "Effekt"];

/// Name eines eingebauten oder Paket-Übergangs (eingebaut: deutsch, die Oberfläche übersetzt).
pub fn transition_name(id: &str) -> String {
    TRANSITIONS
        .iter()
        .find(|t| t.0 == id)
        .map(|t| t.1.to_string())
        .or_else(|| catalog::name_any(id))
        .unwrap_or_else(|| "Übergang".into())
}

/// Wirksame Länge (s) des Übergangs am Ende von `a` zum Nachbarn `b`: höchstens so lang wie jeder
/// der beiden Clips. Je die Hälfte ragt in die Clips hinein, so teilen sich zwei Übergänge an
/// einem Clip dessen Länge. Vorschau und Export rechnen beide hiermit.
pub fn transition_secs(a: &Clip, b: &Clip) -> f64 {
    a.transition_out
        .as_ref()
        .map_or(0.0, |t| t.duration.as_secs_f64())
        .min(a.duration.as_secs_f64())
        .min(b.duration.as_secs_f64())
        .max(0.0)
}

/// Strecke von (x, y) nach (x, y)
type Seg = ((f64, f64), (f64, f64));

/// Stückweise lineare Funktion als FFmpeg-Ausdruck (Variable `var`), Punkte (x, y) sortiert.
///
/// Punkte auf einer Geraden werden zusammengefasst (lange konstante Abschnitte kosten nichts),
/// und die Fallunterscheidung ist ein ausgeglichener Baum statt einer Kette: FFmpeg bricht bei
/// zu tief verschachtelten Ausdrücken ab („Missing ')' or too many args“).
pub fn piecewise_expr(points: &[(f64, f64)], var: &str) -> String {
    let slope = |a: (f64, f64), b: (f64, f64)| {
        if (b.0 - a.0).abs() < 1e-9 {
            0.0
        } else {
            (b.1 - a.1) / (b.0 - a.0)
        }
    };
    // doppelte x-Werte entfernen, Punkte auf einer Geraden zusammenfassen
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if pts
            .last()
            .is_some_and(|l: &(f64, f64)| (p.0 - l.0).abs() < 1e-9)
        {
            *pts.last_mut().expect("nicht leer") = p;
            continue;
        }
        while pts.len() >= 2 {
            let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
            let (k1, k2) = (slope(a, b), slope(b, p));
            if (k1 - k2).abs() <= 1e-6 * k1.abs().max(k2.abs()).max(1.0) {
                pts.pop();
            } else {
                break;
            }
        }
        pts.push(p);
    }
    match pts.as_slice() {
        [] => "0".into(),
        [p] => format!("{:.5}", p.1),
        _ => {
            let segs: Vec<Seg> = pts.windows(2).map(|w| (w[0], w[1])).collect();
            fn tree(
                segs: &[Seg],
                var: &str,
                slope: &dyn Fn((f64, f64), (f64, f64)) -> f64,
            ) -> String {
                if let [(a, b)] = segs {
                    let k = slope(*a, *b);
                    if k == 0.0 {
                        return format!("{:.5}", a.1);
                    }
                    return format!("{:.5}+({var}-{:.5})*{k:.6}", a.1, a.0);
                }
                let mid = segs.len() / 2;
                format!(
                    "if(lt({var},{:.5}),{},{})",
                    segs[mid].0 .0,
                    tree(&segs[..mid], var, slope),
                    tree(&segs[mid..], var, slope)
                )
            }
            let (first, last) = (pts[0], pts[pts.len() - 1]);
            format!(
                "if(lt({var},{:.5}),{:.5},if(lt({var},{:.5}),{},{:.5}))",
                first.0,
                first.1,
                last.0,
                tree(&segs, var, &slope),
                last.1
            )
        }
    }
}

/// Wertet einen von [`piecewise_expr`] erzeugten Verlauf direkt aus (für Tests und Vorschau).
#[cfg(test)]
fn piecewise_eval(points: &[(f64, f64)], x: f64) -> f64 {
    if x < points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        if x < w[1].0 {
            let k = if (w[1].0 - w[0].0).abs() < 1e-9 {
                0.0
            } else {
                (w[1].1 - w[0].1) / (w[1].0 - w[0].0)
            };
            return w[0].1 + (x - w[0].0) * k;
        }
    }
    points[points.len() - 1].1
}

/// Hilfsbau für Filtergraphen mit eindeutigen Labels.
pub struct GraphBuilder<'a> {
    pub lines: Vec<String>,
    cur: String,
    pending: Vec<String>,
    uniq: &'a str,
    n: usize,
}

impl<'a> GraphBuilder<'a> {
    pub fn new(input: &str, uniq: &'a str) -> Self {
        Self {
            lines: Vec::new(),
            cur: input.to_string(),
            pending: Vec::new(),
            uniq,
            n: 0,
        }
    }
    fn label(&mut self, what: &str) -> String {
        self.n += 1;
        format!("{}{what}{}", self.uniq, self.n)
    }
    pub fn push(&mut self, f: impl Into<String>) {
        let f = f.into();
        if !f.is_empty() {
            self.pending.push(f);
        }
    }
    pub fn flush(&mut self) -> String {
        if !self.pending.is_empty() {
            let l = self.label("c");
            self.lines
                .push(format!("[{}]{}[{l}]", self.cur, self.pending.join(",")));
            self.pending.clear();
            self.cur = l;
        }
        self.cur.clone()
    }
    /// Kette mit Stärke `i` über das Original legen.
    pub fn blended(&mut self, chain: &str, i: f32) {
        if i >= 0.999 {
            self.push(chain.to_string());
            return;
        }
        if i <= 0.001 {
            return;
        }
        self.push("format=gbrap");
        let cur = self.flush();
        let (a, b, f, o) = (
            self.label("a"),
            self.label("b"),
            self.label("f"),
            self.label("o"),
        );
        self.lines.push(format!("[{cur}]split[{a}][{b}]"));
        self.lines.push(format!("[{b}]{chain},format=gbrap[{f}]"));
        self.lines.push(format!(
            "[{f}][{a}]blend=all_mode=normal:all_opacity={i:.3}[{o}]"
        ));
        self.cur = o;
    }
    pub fn finish(mut self, output: &str) -> Vec<String> {
        let cur = self.flush();
        self.lines.push(format!("[{cur}]null[{output}]"));
        self.lines
    }
    pub fn current(&mut self) -> String {
        self.flush()
    }
    pub fn set_current(&mut self, l: String) {
        self.cur = l;
    }
    pub fn next_label(&mut self, what: &str) -> String {
        self.label(what)
    }
}

/// Ausgangslage für die Kette.
#[derive(Debug, Clone)]
pub struct ChainOpts {
    /// Zeitversatz (s): `t` in Effekten zählt ab Clipbeginn
    pub t_offset: f64,
    /// Rückwärts im Graphen ausführen (Export; die Vorschau mappt die Zeit selbst)
    pub apply_reverse: bool,
    /// LUT-Datei, schon für Filteroptionen escaped
    pub lut: Option<String>,
    /// Label eines Masken-Stroms (Graustufen, gleiche Größe/Zeit) für „Hintergrund entfernen“
    pub matte: Option<String>,
}

/// Anpassungen als Filterkette (leer bei Neutralstellung).
pub fn adjust_chain(a: &Adjust) -> Vec<String> {
    let mut v = Vec::new();
    if a.is_identity() {
        return v;
    }
    let (b, c, s) = (a.brightness, a.contrast, a.saturation);
    if b != 0.0 || c != 0.0 || s != 0.0 {
        v.push(format!(
            "eq=brightness={:.3}:contrast={:.3}:saturation={:.3}",
            b * 0.3,
            1.0 + c * 0.6,
            (1.0 + s).max(0.0)
        ));
    }
    if a.temperature != 0.0 {
        v.push(format!(
            "colortemperature=temperature={:.0}",
            6500.0 - a.temperature * 3500.0
        ));
    }
    if a.tint != 0.0 {
        let t = a.tint;
        v.push(format!(
            "colorbalance=rm={:.3}:gm={:.3}:bm={:.3}",
            t * 0.1,
            -t * 0.15,
            t * 0.1
        ));
    }
    if a.hue != 0.0 {
        v.push(format!("hue=h={:.1}", a.hue));
    }
    for (k, (code, ..)) in HSL_COLORS.iter().enumerate() {
        let [h, s, l] = a.hsl[k];
        if h != 0.0 || s != 0.0 || l != 0.0 {
            v.push(format!(
                "huesaturation=hue={:.1}:saturation={:.3}:intensity={:.3}:colors={code}:strength=6",
                h * 60.0,
                s,
                l * 0.5
            ));
        }
    }
    if a.highlights != 0.0 || a.shadows != 0.0 || a.fade != 0.0 {
        let lo = (0.15 * a.fade.max(0.0)).clamp(0.0, 0.4);
        let hi = (1.0 - 0.05 * a.fade.max(0.0)).clamp(0.6, 1.0);
        let sh = (0.25 + 0.12 * a.shadows).clamp(0.01, 0.49);
        let hl = (0.75 + 0.12 * a.highlights).clamp(0.51, 0.99);
        v.push(format!(
            "curves=all='0/{lo:.3} 0.25/{sh:.3} 0.75/{hl:.3} 1/{hi:.3}'"
        ));
    }
    if a.sharpen > 0.0 {
        v.push(format!("unsharp=5:5:{:.2}", a.sharpen * 2.0));
    } else if a.sharpen < 0.0 {
        v.push(format!("gblur=sigma={:.2}", -a.sharpen * 6.0));
    }
    if a.vignette > 0.0 {
        v.push(format!("vignette=angle={:.3}", a.vignette * 1.2));
    }
    if a.grain > 0.0 {
        v.push(format!("noise=alls={:.0}:allf=t", a.grain * 40.0));
    }
    v
}

/// Masken-Ausdruck (0…1) für `geq` in Pixelkoordinaten X/Y, Bildgröße W/H.
pub fn mask_expr(m: &Mask) -> String {
    let (s, c) = m.rotation_deg.to_radians().sin_cos();
    let (cx, cy) = (format!("{:.4}*W", m.cx), format!("{:.4}*H", m.cy));
    // gedrehte Koordinaten relativ zum Mittelpunkt
    let u = format!("((X-{cx})*{c:.5}+(Y-{cy})*{s:.5})");
    let v = format!("(-(X-{cx})*{s:.5}+(Y-{cy})*{c:.5})");
    let f = (m.feather.max(0.0005) * 2.0) as f64;
    let base = match m.shape {
        MaskShape::Linear => format!("clip(0.5-{v}/({f:.4}*H+1),0,1)"),
        MaskShape::Mirror => format!("clip(({:.4}*H/2-abs({v}))/({f:.4}*H+1)+0.5,0,1)", m.h),
        MaskShape::Circle => {
            format!(
                "clip((1-hypot({u}/({:.4}*W/2+0.001),{v}/({:.4}*H/2+0.001)))/{f:.4}+0.5,0,1)",
                m.w, m.h
            )
        }
        MaskShape::Rect => {
            format!("clip((1-max(abs({u})/({:.4}*W/2+0.001),abs({v})/({:.4}*H/2+0.001)))/{f:.4}+0.5,0,1)", m.w, m.h)
        }
    };
    if m.invert {
        format!("(1-{base})")
    } else {
        base
    }
}

/// Komplette Clip-Kette: Eingang `input` → Ausgang `output` (RGBA bei Maske, sonst offen).
pub fn fx_graph(
    fx: &ClipFx,
    src: Src,
    o: &ChainOpts,
    input: &str,
    output: &str,
    uniq: &str,
) -> Vec<String> {
    let mut g = GraphBuilder::new(input, uniq);
    if o.apply_reverse && fx.reverse {
        g.push("reverse");
    }
    // gerundet: setpts schneidet sonst ab (29,97 fps: 5,5055 s → ein Bild zu früh)
    g.push(format!("setpts=PTS-STARTPTS+round({:.6}/TB)", o.t_offset));
    // vor allem anderen: Lage im Quellbild (vor Spiegeln, Stabilisieren, Freistellen)
    for cs in &fx.censor {
        censor_graph(&mut g, cs, src);
    }
    if fx.deflicker {
        g.push("deflicker=size=7:mode=pm");
    }
    if fx.denoise > 0.0 {
        let k = fx.denoise.clamp(0.0, 1.0);
        g.push(format!(
            "hqdn3d=luma_spatial={:.2}:chroma_spatial={:.2}:luma_tmp={:.2}:chroma_tmp={:.2}",
            1.0 + 7.0 * k,
            1.0 + 5.0 * k,
            2.0 + 10.0 * k,
            2.0 + 8.0 * k
        ));
    }
    let mut person_done = false;
    if let (Some(m), Some(pf)) = (&o.matte, &fx.person) {
        person_graph(&mut g, m, pf, src);
        person_done = true;
    }
    if person_done {
    } else if let Some(m) = &o.matte {
        // Hintergrund entfernen: Maske als Alphakanal
        g.push("format=rgb24");
        let cur = g.current();
        let l = g.next_label("cm");
        g.lines.push(format!("[{cur}][{m}]alphamerge[{l}]"));
        g.set_current(l);
    } else if fx.stabilize > 0.0 {
        let k = fx.stabilize.clamp(0.0, 1.0);
        let r = 16 * (1.0 + 3.0 * k).round() as u32;
        let z = 1.0 - 0.08 * k;
        let (cw, ch) = (
            ((src.w as f32 * z) as u32).max(2) & !1,
            ((src.h as f32 * z) as u32).max(2) & !1,
        );
        g.push(format!(
            "deshake=rx={r}:ry={r}:edge=mirror,crop={cw}:{ch},scale={}:{}",
            src.w, src.h
        ));
    }
    if fx.flip_h {
        g.push("hflip");
    }
    if fx.flip_v {
        g.push("vflip");
    }
    if let Some(k) = &fx.chroma {
        let [r, gg, b] = k.color;
        g.push(format!(
            "chromakey=color=0x{r:02x}{gg:02x}{b:02x}:similarity={:.3}:blend={:.3}",
            k.similarity.clamp(0.01, 1.0),
            k.blend.clamp(0.0, 1.0)
        ));
        if k.spill > 0.0 {
            let ty = if b > gg { "blue" } else { "green" };
            g.push(format!(
                "format=rgba,despill=type={ty}:mix={:.2}:expand=0.1",
                k.spill.clamp(0.0, 1.0)
            ));
        }
    }
    let src = Src {
        has_alpha: src.has_alpha
            || (o.matte.is_some()
                && (!person_done || matches!(fx.person, Some(cl_core::PersonFx::Outline { transparent: true, .. }))))
            || fx.chroma.is_some(),
        ..src
    };
    let colour_work = !fx.adjust.is_identity()
        || fx.filter.is_some()
        || !fx.effects.is_empty()
        || o.lut.is_some();
    // Alpha (Sticker, Freistellen, Greenscreen) über die Farbfilter retten
    let mut alpha = None;
    if src.has_alpha && colour_work {
        g.push("format=rgba");
        let cur = g.current();
        let (c, a, am) = (g.next_label("k"), g.next_label("al"), g.next_label("am"));
        g.lines.push(format!("[{cur}]split[{c}][{a}]"));
        g.lines.push(format!("[{a}]alphaextract[{am}]"));
        g.set_current(c);
        alpha = Some(am);
    }
    for f in adjust_chain(&fx.adjust) {
        g.push(f);
    }
    if let Some(f) = &fx.filter {
        if let Some(def) = filter(&f.id) {
            g.blended(&(def.build)(1.0, src), f.intensity.clamp(0.0, 1.0));
        } else if let Some(c) = catalog::pack_chain(catalog::Kind::Filter, &f.id, 1.0) {
            g.blended(&c, f.intensity.clamp(0.0, 1.0));
        }
    }
    for e in &fx.effects {
        if let Some(def) = effect(&e.id) {
            let i = e.intensity.clamp(0.0, 1.0);
            if def.native_intensity {
                g.push((def.build)(i, src));
            } else {
                g.blended(&(def.build)(1.0, src), i);
            }
        } else if let Some(c) = catalog::pack_chain(catalog::Kind::Effect, &e.id, e.intensity.clamp(0.0, 1.0)) {
            g.push(c);
        }
    }
    if let Some(l) = &o.lut {
        g.push(format!("lut3d=file={l}"));
    }
    if let Some(am) = alpha {
        g.push("format=rgb24");
        let cur = g.current();
        let l = g.next_label("m");
        g.lines.push(format!("[{cur}][{am}]alphamerge[{l}]"));
        g.set_current(l);
    }
    if let Some(m) = &fx.mask {
        g.push("format=rgba");
        g.push(format!(
            "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{}'",
            mask_expr(m)
        ));
    }
    g.finish(output)
}

/// Zensur-Fläche (Bahn in Clipzeit, siehe `Clip::censor_in_clip_time`): Kopie verpixeln bzw.
/// weichzeichnen und nur innerhalb der Fläche über das Bild legen. Blöcke, Unschärfe und Maske
/// hängen nur von der Bildgröße ab: die kleinere Vorschau sieht aus wie der Export. Die Maske
/// entsteht aus einer Zeile und einer Spalte (`geq` mit Zeitverlauf je Pixel wäre teuer): je
/// Richtung 0 innen bis 255 außen, addiert und bei 128 geschnitten – Rechteck wie Ellipse.
pub fn censor_graph(g: &mut GraphBuilder, cs: &Censor, src: Src) {
    let size = (CENSOR_MIN + cs.strength.clamp(0.0, 1.0) * (CENSOR_MAX - CENSOR_MIN))
        * src.w.min(src.h).max(2) as f32;
    let effect = if cs.blur {
        format!("gblur=sigma={size:.2}")
    } else {
        let n = |v: u32| ((v as f32 / size).round() as u32).max(1);
        format!(
            "scale={}:{}:flags=area,scale={}:{}:flags=neighbor",
            n(src.w),
            n(src.h),
            src.w,
            src.h
        )
    };
    let e = |f: fn(&CensorKey) -> f32| {
        let pts: Vec<(f64, f64)> = cs.path.iter().map(|k| (k.t.as_secs_f64(), f(k) as f64)).collect();
        piecewise_expr(&pts, "T")
    };
    // Abstand zur Mitte relativ zur halben Größe: 1 = Rand
    let side = |pos: &str, full: &str, c: String, size: String| {
        let d = format!("2*abs(({pos}+0.5)/{full}-({c}))/({size})");
        if cs.ellipse {
            format!("min(255,128*pow({d},2))")
        } else {
            format!("255*gt({d},1)")
        }
    };
    let (sx, sy) = (side("X", "W", e(|k| k.cx), e(|k| k.w)), side("Y", "H", e(|k| k.cy), e(|k| k.h)));
    let (w, h) = (src.w, src.h);
    let cur = g.current();
    let (a, b, mx, my) = (g.next_label("za"), g.next_label("zb"), g.next_label("zx"), g.next_label("zy"));
    let (kx, ky) = (g.next_label("zkx"), g.next_label("zky"));
    let (fx, mk, top, out) = (g.next_label("zf"), g.next_label("zk"), g.next_label("zt"), g.next_label("zo"));
    g.lines.push(format!("[{cur}]split=4[{a}][{b}][{mx}][{my}]"));
    g.lines.push(format!("[{b}]{effect}[{fx}]"));
    g.lines.push(format!(
        "[{mx}]scale={w}:1:flags=area,format=gray,geq=lum='{sx}',scale={w}:{h}:flags=neighbor[{kx}]"
    ));
    g.lines.push(format!(
        "[{my}]scale=1:{h}:flags=area,format=gray,geq=lum='{sy}',scale={w}:{h}:flags=neighbor[{ky}]"
    ));
    g.lines.push(format!("[{kx}][{ky}]blend=all_mode=addition,lut=c0='255*lte(val,128)'[{mk}]"));
    g.lines.push(format!("[{fx}][{mk}]alphamerge[{top}]"));
    // shortcut: Bilder mit Alpha werden unter der Fläche deckend; reicht für Video und Fotos
    g.lines.push(format!("[{a}][{top}]overlay=format=auto[{out}]"));
    g.set_current(out);
}

/// Personen-Effekte: Person (Maske `m`) bleibt scharf, der Hintergrund wird verändert.
/// Das Ergebnis ist deckend.
fn person_graph(g: &mut GraphBuilder, m: &str, pf: &cl_core::PersonFx, src: Src) {
    use cl_core::PersonFx;
    let hex = |c: [u8; 3]| format!("0x{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    g.push("format=rgb24");
    let cur = g.current();
    let (fg_src, bg_src, mm) = (g.next_label("pf"), g.next_label("pb"), g.next_label("pm"));
    let fg = g.next_label("pfg");
    let bg = g.next_label("pbg");
    let out = g.next_label("po");
    match pf {
        PersonFx::BlurBackground { amount } => {
            let sigma =
                2.0 + 28.0 * amount.clamp(0.0, 1.0) * (src.w.max(src.h) as f32 / 1920.0).max(0.2);
            g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            g.lines
                .push(format!("[{bg_src}]gblur=sigma={sigma:.2}[{bg}]"));
            g.lines.push(format!("[{fg_src}][{m}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{bg}][{fg}]overlay=format=auto,format=rgb24[{out}]"
            ));
        }
        PersonFx::ColorBackground { color } => {
            g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            g.lines.push(format!(
                "[{bg_src}]drawbox=x=0:y=0:w=iw:h=ih:color={}:t=fill[{bg}]",
                hex(*color)
            ));
            g.lines.push(format!("[{fg_src}][{m}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{bg}][{fg}]overlay=format=auto,format=rgb24[{out}]"
            ));
        }
        PersonFx::Outline { color, width, transparent } => {
            // Maske verbreitern und weich zeichnen → farbiger Saum hinter der Person
            let px =
                (2.0 + 22.0 * width.clamp(0.0, 1.0)) * (src.w.max(src.h) as f32 / 1920.0).max(0.25);
            let (m1, m2, ring, col, base, ringed) = (
                g.next_label("pm1"),
                g.next_label("pm2"),
                g.next_label("pr"),
                g.next_label("pc"),
                g.next_label("pbs"),
                g.next_label("prd"),
            );
            // ohne Hintergrund: der Umriss ist die Grundfläche, das Ergebnis behält Alpha
            if *transparent {
                g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            } else {
                g.lines
                    .push(format!("[{cur}]split=3[{fg_src}][{bg_src}][{base}]"));
            }
            g.lines.push(format!("[{m}]split[{m1}][{m2}]"));
            g.lines.push(format!(
                "[{m2}]format=gray,gblur=sigma={:.2},lut=y='min(255,val*4)'[{mm}]",
                px / 2.0
            ));
            g.lines.push(format!(
                "[{bg_src}]drawbox=x=0:y=0:w=iw:h=ih:color={}:t=fill[{col}]",
                hex(*color)
            ));
            g.lines.push(format!("[{col}][{mm}]alphamerge[{ring}]"));
            let under = if *transparent {
                ring
            } else {
                g.lines
                    .push(format!("[{base}][{ring}]overlay=format=auto[{ringed}]"));
                ringed
            };
            g.lines.push(format!("[{fg_src}][{m1}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{under}][{fg}]overlay=format=auto,format={}[{out}]",
                if *transparent { "rgba" } else { "rgb24" }
            ));
        }
    }
    g.set_current(out);
}

/// Filtergraph für die Vorschau (`[0:v]` → `[out]`, `w`×`h` RGBA) oder `None`, wenn der
/// schnelle Weg ohne Effekte reicht. `t_offset` = Clipzeit des ersten Bildes,
/// `stream` = Wiedergabe (Bildrate und Geschwindigkeit), sonst Einzelbild.
pub fn preview_graph(
    c: &Clip,
    w: u32,
    h: u32,
    t_offset: f64,
    stream: Option<Rational>,
    lut: Option<String>,
) -> Option<String> {
    let speed = c.speed;
    let fx = &c.fx;
    let cutout = fx
        .cutout
        .as_ref()
        .filter(|p| std::path::Path::new(p).is_file());
    let needs = !fx.is_empty()
        && (fx.flip_h
            || fx.flip_v
            || !fx.adjust.is_identity()
            || fx.filter.is_some()
            || !fx.effects.is_empty()
            || fx.mask.is_some()
            || lut.is_some()
            || fx.chroma.is_some()
            || fx.stabilize > 0.0
            || cutout.is_some()
            || fx.denoise > 0.0
            || fx.deflicker
            || !fx.censor.is_empty());
    let speed_stream = stream.is_some() && ((speed - 1.0).abs() > 1e-3 || fx.speed_curve.is_some());
    if !needs && !speed_stream {
        return None;
    }
    let mut pre = vec!["setpts=PTS-STARTPTS".to_string()];
    let mut matte_pre = vec!["setpts=PTS-STARTPTS".to_string()];
    if let Some(fps) = stream {
        if fx.speed_curve.is_none() && (speed - 1.0).abs() > 1e-3 {
            matte_pre.push(format!("setpts=(PTS-STARTPTS)/{speed:.5}"));
        }
        matte_pre.push(format!("fps={}/{}", fps.num, fps.den));
    }
    matte_pre.push(format!("scale={w}:{h}:flags=bilinear,format=gray"));
    if let Some(fps) = stream {
        if fx.speed_curve.is_none() && (speed - 1.0).abs() > 1e-3 {
            pre.push(format!("setpts=(PTS-STARTPTS)/{speed:.5}"));
        }
        if fx.smooth_slowmo && speed < 0.999 {
            // Vorschau: günstiges Überblenden statt voller Bewegungsschätzung
            pre.push(format!(
                "minterpolate=fps={}/{}:mi_mode=blend",
                fps.num, fps.den
            ));
        }
        pre.push(format!("fps={}/{}", fps.num, fps.den));
    }
    pre.push(format!("scale={w}:{h}:flags=bilinear"));
    let mut lines = vec![format!("[0:v]{}[pre]", pre.join(","))];
    let mut matte = None;
    if let Some(m) = cutout {
        let src_t = c
            .source_time(c.start + Time::from_secs_f64(t_offset))
            .as_secs_f64();
        lines.push(format!(
            "movie=filename={}:seek_point={src_t:.4},{}[mt]",
            cl_media::escape_path(m),
            matte_pre.join(",")
        ));
        matte = Some("mt".to_string());
    }
    let has_alpha = true;
    let mut fx2 = fx.clone();
    fx2.reverse = false;
    // Einzelbild: nur die Lage zu dieser Zeit (kurzer Graph auf der Befehlszeile)
    fx2.censor = c.censor_in_clip_time(stream.is_none().then(|| Time::from_secs_f64(t_offset)));
    let o = ChainOpts {
        t_offset,
        apply_reverse: false,
        lut,
        matte,
    };
    lines.extend(fx_graph(
        &fx2,
        Src { w, h, has_alpha },
        &o,
        "pre",
        "fx",
        "p",
    ));
    lines.push("[fx]format=rgba[out]".into());
    Some(lines.join(";"))
}

/// `atempo`-Kette für eine Geschwindigkeit (jede Stufe 0,5…2).
pub fn atempo_chain(speed: f32) -> String {
    let mut s = speed.clamp(0.01, 100.0) as f64;
    let mut v = Vec::new();
    while s > 2.0 {
        v.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        v.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-4 {
        v.push(format!("atempo={s:.5}"));
    }
    v.join(",")
}

/// `setpts` für Geschwindigkeit bzw. Geschwindigkeitskurve. Eingang: Quellzeit ab 0
/// (Länge `src_span` s), Ausgang: Clipzeit (Länge `dur` s).
pub fn speed_setpts(speed: f32, curve: Option<&str>, src_span: f64, dur: f64) -> String {
    match curve {
        Some(id) => {
            // Umkehrfunktion der Kurve abtasten: Quellzeit → Clipzeit
            let pts: Vec<(f64, f64)> = (0..=48)
                .map(|k| {
                    let u = k as f32 / 48.0;
                    (curve_progress(id, u) as f64 * src_span, u as f64 * dur)
                })
                .collect();
            format!("setpts='({})/TB'", piecewise_expr(&pts, "T"))
        }
        None if (speed - 1.0).abs() > 1e-4 => format!("setpts=(PTS-STARTPTS)/{speed:.5}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Jeder Effekt und Filter muss auch als Abspiel-Strom laufen (sonst bleibt beim
    /// Abspielen ein Standbild stehen).
    #[test]
    fn every_effect_and_filter_streams_for_playback() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=3", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(st.success());
        let fps = Rational::new(30, 1);
        let mut failed = Vec::new();
        let ids: Vec<(String, bool)> = EFFECTS
            .iter()
            .map(|e| (e.id.to_string(), false))
            .chain(FILTERS.iter().map(|f| (f.id.to_string(), true)))
            .collect();
        for (id, filter) in ids {
            let mut c = Clip::new(
                ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video },
                Time::ZERO,
                Time::from_secs_f64(3.0),
            );
            if filter {
                c.fx.filter = Some(FxRef::new(&id));
            } else {
                c.fx.effects.push(FxRef::new(&id));
            }
            let Some(g) = preview_graph(&c, 320, 180, 0.5, Some(fps), None) else {
                continue;
            };
            let s = cl_media::FrameStream::open_fx(&src, Time::from_secs_f64(0.5), 320, 180, fps, Some(&g));
            let n = match s {
                Ok(s) => (0..10).take_while(|_| s.next_blocking().is_some()).count(),
                Err(_) => 0,
            };
            if n < 10 {
                failed.push(format!("{id} ({n} Bilder)"));
            }
        }
        assert!(failed.is_empty(), "laufen nicht als Strom: {failed:?}");
    }

    /// VHS auf dem kleinen Vorschaubild sieht aus wie der Full-HD-Export, verkleinert auf dieselbe
    /// Größe: Zeilenmuster und Farbversatz gleich stark (vorher in der Vorschau viel kräftiger).
    #[test]
    fn vhs_preview_size_looks_like_the_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        // Bild nach der Kette auf 480×270: (Zeilenmuster, Farbversatz Rot−Blau am senkrechten Rand)
        let look = |w: u32, h: u32| -> (f64, f64) {
            let vhs = (effect("vhs").unwrap().build)(1.0, Src { w, h, has_alpha: false });
            let out = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-nostdin", "-threads", "2", "-f", "lavfi", "-i"])
                .arg("color=c=0x808080:s=1920x1080:d=0.1,drawbox=x=960:y=0:w=960:h=1080:c=white:t=fill")
                .args(["-frames:v", "1", "-vf"])
                .arg(format!("scale={w}:{h}:flags=area,format=rgb24,{vhs},scale=480:270:flags=area,format=rgb24"))
                .args(["-f", "rawvideo", "-"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let px = |x: usize, y: usize, c: usize| out.stdout[(y * 480 + x) * 3 + c] as f64;
            let rows: Vec<f64> = (0..270).map(|y| (0..200).map(|x| px(x, y, 1)).sum::<f64>() / 200.0).collect();
            let stripes = (1..269).map(|y| (rows[y] - (rows[y - 1] + rows[y + 1]) / 2.0).abs()).sum::<f64>() / 268.0;
            // Rot läuft nach links, Blau nach rechts: Spalten neben der Kante bei x = 240
            let shift = (0..270).map(|y| (230..250).map(|x| (px(x, y, 0) - px(x, y, 2)).abs()).sum::<f64>()).sum::<f64>() / (270.0 * 20.0);
            (stripes, shift)
        };
        // alte Kette: Zeilen 14,6 statt 3,7, Farbversatz 76 statt 21
        let (export, preview) = (look(1920, 1080), look(480, 270));
        assert!((export.0 - preview.0).abs() < 4.0, "Zeilen: Export {export:?}, Vorschau {preview:?}");
        assert!((export.1 - preview.1).abs() < 4.0, "Farbversatz: Export {export:?}, Vorschau {preview:?}");
    }

    fn run_graph(graph: &str, extra_inputs: &[&str]) -> std::result::Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("g.txt"), graph).unwrap();
        let mut cmd = cl_media::ffmpeg_command().unwrap();
        cmd.args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=15:d=0.6",
        ]);
        for i in extra_inputs {
            cmd.args(["-f", "lavfi", "-i", i]);
        }
        let out = cmd
            .args([
                "-/filter_complex",
                "g.txt",
                "-map",
                "[out]",
                "-f",
                "null",
                "-",
            ])
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    const SRC: Src = Src {
        w: 160,
        h: 90,
        has_alpha: false,
    };

    #[test]
    fn every_effect_and_filter_runs_in_ffmpeg() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: None,
        };
        for def in EFFECTS.iter().chain(FILTERS.iter()) {
            for i in [0.4, 1.0] {
                let mut fx = ClipFx::default();
                if FILTERS.iter().any(|f| f.id == def.id) {
                    fx.filter = Some(FxRef {
                        id: def.id.into(),
                        intensity: i,
                        params: Default::default(),
                    });
                } else {
                    fx.effects.push(FxRef {
                        id: def.id.into(),
                        intensity: i,
                        params: Default::default(),
                    });
                }
                let g = fx_graph(&fx, SRC, &o, "0:v", "out", "x").join(";\n");
                run_graph(&g, &[]).unwrap_or_else(|e| panic!("{} ({i}): {e}\n{g}", def.id));
            }
        }
        assert!(EFFECTS.len() >= 30, "{}", EFFECTS.len());
        assert!(FILTERS.len() >= 20);
    }

    #[test]
    fn full_chain_with_adjust_mask_alpha_and_lut() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // Apostroph, Komma und Klammern im Pfad (früher fehlte die LUT dann in der Vorschau)
        let lut = dir.path().join("O'Brien, [x]").join("t.cube");
        std::fs::create_dir_all(lut.parent().unwrap()).unwrap();
        let mut cube = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    cube.push_str(&format!(
                        "{} {} {}\n",
                        r as f32 * 0.9,
                        g as f32,
                        b as f32 * 0.8
                    ));
                }
            }
        }
        std::fs::write(&lut, cube).unwrap();
        let mut fx = ClipFx::default();
        fx.adjust = Adjust {
            brightness: 0.2,
            contrast: 0.3,
            saturation: -0.2,
            temperature: 0.5,
            tint: 0.2,
            highlights: -0.4,
            shadows: 0.3,
            sharpen: 0.5,
            vignette: 0.4,
            grain: 0.3,
            fade: 0.3,
            hue: 20.0,
            hsl: [[0.2, 0.3, -0.1]; 6],
        };
        fx.filter = Some(FxRef {
            id: "cinematic".into(),
            intensity: 0.6,
            params: Default::default(),
        });
        fx.effects.push(FxRef::new("vhs"));
        fx.flip_h = true;
        fx.reverse = true;
        for shape in [
            MaskShape::Linear,
            MaskShape::Mirror,
            MaskShape::Circle,
            MaskShape::Rect,
        ] {
            let mut m = Mask::new(shape);
            m.rotation_deg = 30.0;
            m.invert = shape == MaskShape::Circle;
            fx.mask = Some(m);
            let o = ChainOpts {
                t_offset: 1.5,
                apply_reverse: true,
                lut: Some(cl_media::escape_path(&lut.to_string_lossy())),
                matte: None,
            };
            let src = Src {
                w: 160,
                h: 90,
                has_alpha: true,
            };
            let g = fx_graph(&fx, src, &o, "0:v", "out", "y").join(";\n");
            run_graph(&g, &[]).unwrap_or_else(|e| panic!("{shape:?}: {e}\n{g}"));
        }
    }

    #[test]
    fn censor_never_goes_below_the_minimum() {
        let src = Src { w: 1920, h: 1080, has_alpha: false };
        let graph = |blur: bool, strength: f32| {
            let mut g = GraphBuilder::new("i", "u");
            let path = vec![CensorKey { t: Time::ZERO, cx: 0.5, cy: 0.5, w: 0.2, h: 0.3 }];
            censor_graph(&mut g, &Censor { blur, ellipse: false, strength, track: false, path, ..Default::default() }, src);
            g.finish("o").join(";")
        };
        // Blöcke ≥ 45 px (1080/24), Unschärfe σ ≥ 45 px – auch bei Stärke unter 0
        assert!(graph(false, -1.0).contains("scale=43:24:flags=area"), "{}", graph(false, -1.0));
        assert!(graph(true, -1.0).contains("gblur=sigma=45.00"));
        assert!(graph(true, 1.0).contains("gblur=sigma=135.00"));
        assert!(graph(false, 1.0).contains("scale=14:8:flags=area"));
    }

    #[test]
    fn preview_graph_decodes() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("v.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x180:r=30:d=2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let mut c = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Video,
            },
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        assert!(preview_graph(&c, 160, 90, 0.0, None, None).is_none());
        c.fx.effects.push(FxRef::new("vhs"));
        c.fx.mask = Some(Mask::new(MaskShape::Circle));
        c.speed = 2.0;
        let g = preview_graph(&c, 160, 90, 0.5, None, None).unwrap();
        let f = cl_media::decode_frame_fx(&v, Time::from_secs_f64(0.5), 160, 90, Some(&g)).unwrap();
        assert_eq!(f.data.len(), 160 * 90 * 4);
        assert_eq!(
            f.data[3], 0,
            "Ecke außerhalb der Kreismaske ist transparent"
        );
        let g = preview_graph(&c, 160, 90, 0.0, Some(Rational::new(30, 1)), None).unwrap();
        let s =
            cl_media::FrameStream::open_fx(&v, Time::ZERO, 160, 90, Rational::new(30, 1), Some(&g))
                .unwrap();
        let mut n = 0;
        while s.next_blocking().is_some() {
            n += 1;
        }
        assert!(
            (28..=32).contains(&n),
            "2 s bei Tempo 2 → ~30 Bilder, waren {n}"
        );
    }

    #[test]
    fn chroma_stabilize_and_matte_run() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let mut fx = ClipFx::default();
        fx.chroma = Some(ChromaKey::default());
        fx.stabilize = 0.6;
        fx.adjust.brightness = 0.1;
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: None,
        };
        let g = fx_graph(&fx, SRC, &o, "0:v", "out", "c").join(";\n");
        run_graph(&g, &[]).unwrap_or_else(|e| panic!("{e}\n{g}"));
        // Maske als zweiter Eingang
        let mut lines = vec!["[1:v]format=gray[mt]".to_string()];
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: Some("mt".into()),
        };
        lines.extend(fx_graph(
            &ClipFx {
                flip_h: true,
                ..Default::default()
            },
            SRC,
            &o,
            "0:v",
            "out",
            "m",
        ));
        let g = lines.join(";\n");
        run_graph(&g, &["color=gray:s=160x90:r=15:d=0.6"]).unwrap_or_else(|e| panic!("{e}\n{g}"));
    }

    #[test]
    fn plugin_chains_run() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        use catalog::{Entry, Kind, Pack, PackInfo, Payload};
        let info = PackInfo {
            id: "t.p".into(),
            name: "T".into(),
            version: "1.0.0".into(),
            description: String::new(),
            license: "CC0-1.0".into(),
            author: String::new(),
            source: None,
        };
        catalog::load_packs(vec![Pack {
            info,
            enabled: true,
            entries: vec![Entry {
                id: "t.p/glow".into(),
                kind: Kind::Effect,
                name: "Glow".into(),
                categories: vec![],
                pack: "t.p".into(),
                license: "CC0-1.0".into(),
                author: String::new(),
                source: None,
                tags: Vec::new(),
                payload: Payload::Ffmpeg("gblur=sigma=2,eq=brightness=0.04*{intensity}".into()),
            }],
        }]);
        let mut fx = ClipFx::default();
        fx.effects.push(FxRef {
            id: "t.p/glow".into(),
            intensity: 0.5,
            params: Default::default(),
        });
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: None,
        };
        let g = fx_graph(&fx, SRC, &o, "0:v", "out", "p").join(
            ";
",
        );
        assert!(g.contains("0.04*0.500"), "{g}");
        run_graph(&g, &[]).unwrap();
        assert_eq!(fx_name("t.p/glow"), "Glow");
    }

    #[test]
    fn every_transition_runs_in_ffmpeg() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        for (id, ..) in TRANSITIONS {
            let g = format!("[0:v]format=gbrap[a];[1:v]format=gbrap[b];[a][b]xfade=transition={id}:duration=0.3:offset=0.2[out]");
            run_graph(&g, &["color=red:s=160x90:r=15:d=0.6"])
                .unwrap_or_else(|e| panic!("{id}: {e}"));
        }
        // alle xfade-Varianten des mitgelieferten FFmpeg außer „custom“ und „squeezev“
        // (stürzt in FFmpeg 9.0.2 bei jeder Größe ab)
        assert_eq!(TRANSITIONS.len(), 57);
        assert!(!TRANSITIONS.iter().any(|t| t.0 == "squeezev"));
    }

    #[test]
    fn long_piecewise_stays_shallow_and_runs_in_ffmpeg() {
        // 240 Stützpunkte: Animation am Anfang, danach konstant (wie ein langer Textclip)
        let mut pts: Vec<(f64, f64)> = (0..240)
            .map(|i| (i as f64 * 0.141, if i < 4 { i as f64 * 0.1 } else { 0.4 }))
            .collect();
        // dazu eine echte Kurve mit vielen Knicken
        pts.extend((240..400).map(|i| (i as f64 * 0.141, ((i as f64) * 0.3).sin())));
        let e = piecewise_expr(&pts, "t");
        let mut depth = 0i32;
        let mut max = 0;
        for c in e.chars() {
            match c {
                '(' => {
                    depth += 1;
                    max = max.max(depth);
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        assert!(max < 40, "zu tief verschachtelt: {max}");
        assert!(e.matches("if(").count() < pts.len());
        let _ = piecewise_eval(&pts, 1.0);
        if !cl_media::ffmpeg_available() {
            return;
        }
        let g = format!(
            "[0:v]scale=w='max(2,trunc(320*(0.5+{})/2)*2)':h=180:eval=frame[out]",
            e
        );
        run_graph(&g, &["color=red:s=320x180:r=10:d=1"]).unwrap();
    }

    #[test]
    fn piecewise_and_speed() {
        let e = piecewise_expr(&[(0.0, 0.0), (1.0, 10.0), (2.0, 10.0)], "t");
        assert!(e.starts_with("if(lt(t,"));
        assert_eq!(atempo_chain(1.0), "");
        assert_eq!(atempo_chain(4.0), "atempo=2,atempo=2.00000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.50000");
        // sehr langsame Stücke einer Tempo-Kurve (z. B. 0,5× mit „Kugel“) nicht kappen
        let prod: f64 = atempo_chain(0.05).split(',').map(|a| a[7..].parse::<f64>().unwrap()).product();
        assert!((prod - 0.05).abs() < 1e-4, "{prod}");
        if !cl_media::ffmpeg_available() {
            return;
        }
        let sp = speed_setpts(1.0, Some("montage"), 0.6, 0.6);
        run_graph(&format!("[0:v]{sp}[out]"), &[]).unwrap();
        run_graph(
            &format!("[0:v]{}[out]", speed_setpts(2.0, None, 0.6, 0.3)),
            &[],
        )
        .unwrap();
        run_graph(
            &format!(
                "[0:v]setpts=PTS-STARTPTS,geq=lum='{}':cb=128:cr=128[out]",
                piecewise_expr(&[(0.0, 10.0), (0.3, 200.0)], "T")
            ),
            &[],
        )
        .unwrap();
    }

    /// Paket „Effekte und Filter“ (`plugins/cutline-effekte`): jede Kette läuft in FFmpeg wie in
    /// Vorschau (RGBA) und Export (YUV), behält die Bildgröße, und Effekte tun bei Stärke 0
    /// praktisch nichts.
    #[test]
    fn effekte_pack_chains_run() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/cutline-effekte/plugin.toml"
        ))
        .unwrap();
        // shortcut: Zeilen lesen statt TOML parsen (cl-fx hängt nicht an toml); das Paket schreibt
        // jede Kette in eine Zeile `ffmpeg = "…"` nach ihrer Zeile `kind = "…"`
        let mut assets = Vec::new();
        let mut kind = "";
        for l in src.lines() {
            if let Some(k) = l.strip_prefix("kind = \"") {
                kind = k.trim_end_matches('"');
            }
            if let Some(c) = l.strip_prefix("ffmpeg = \"") {
                assets.push((kind == "filter", c.trim_end_matches('"').to_string()));
            }
        }
        assert!(assets.len() >= 30, "{}", assets.len());
        assert!(assets.iter().all(|(f, c)| *f || c.contains("{intensity}")));
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (w, h, n) = (202usize, 114usize, 12usize);
        // Bilder als RGBA-Bytes; `fx` = (Kette, Stärke, ist Filter)
        let render = |fmt: &str, fx: Option<(&str, f32, bool)>| -> Vec<u8> {
            let mut g = GraphBuilder::new("0:v", "e");
            g.push(format!("format={fmt}"));
            match fx {
                Some((c, i, true)) => g.blended(&c.replace("{intensity}", "1.000"), i),
                Some((c, i, false)) => g.push(c.replace("{intensity}", &format!("{i:.3}"))),
                None => {}
            }
            g.push("format=rgba");
            let graph = g.finish("out").join(";");
            let out = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"])
                .arg(format!("testsrc2=s={w}x{h}:r=24:d=0.5"))
                .args(["-filter_complex", &graph, "-map", "[out]", "-f", "rawvideo", "-"])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success(), "{graph}\n{}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.stdout.len(), w * h * 4 * n, "Bildgröße/Bildzahl: {graph}");
            out.stdout
        };
        let plain = render("yuv420p", None);
        for (filter, c) in &assets {
            for fmt in ["yuv420p", "rgba"] {
                render(fmt, Some((c, 0.6, *filter)));
                render(fmt, Some((c, 1.0, *filter)));
            }
            if !filter {
                let zero = render("yuv420p", Some((c, 0.0, false)));
                let diff = zero.iter().zip(&plain).map(|(a, b)| a.abs_diff(*b) as u64).sum::<u64>()
                    as f64
                    / plain.len() as f64;
                assert!(diff < 2.0, "Stärke 0 verändert das Bild ({diff:.2}): {c}");
            }
        }
    }

    /// Jede eingebaute und jede mitgelieferte Paket-Kette läuft bei Stärke 0, 0,5 und 1 auch bei
    /// ungeraden Maßen und winzigen Bildern und behält die Bildgröße.
    #[test]
    fn every_chain_runs_at_odd_sizes_and_strengths() {
        chains_at_sizes(&[(161, 91, 2, &[0.0, 0.5, 1.0]), (64, 64, 1, &[1.0])]);
    }

    /// Dasselbe in Hochkant-Full-HD (dauert ~1 min): `cargo test -p cl-fx -- --ignored portrait`
    #[test]
    #[ignore]
    fn every_chain_runs_in_portrait_full_hd() {
        chains_at_sizes(&[(1080, 1920, 1, &[0.0, 1.0])]);
    }

    /// (Breite, Höhe, Bilder, Stärken)
    fn chains_at_sizes(sizes: &[(u32, u32, usize, &[f32])]) {
        if !cl_media::ffmpeg_available() {
            return;
        }
        // Paket-Ketten aller mitgelieferten Pakete (Zeilen wie in `effekte_pack_chains_run`)
        let mut packs = Vec::new();
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins");
        for d in std::fs::read_dir(root).unwrap().flatten() {
            let Ok(src) = std::fs::read_to_string(d.path().join("plugin.toml")) else { continue };
            let mut kind = "";
            for l in src.lines() {
                if let Some(k) = l.strip_prefix("kind = \"") {
                    kind = k.trim_end_matches('"');
                }
                if let Some(c) = l.strip_prefix("ffmpeg = \"") {
                    // weitere Parameter (`{radius}` …) mit 1 füllen, wie ein Standardwert
                    let mut c = c.trim_end_matches('"').replace("{intensity}", "\u{1}");
                    while let (Some(a), Some(b)) = (c.find('{'), c.find('}')) {
                        c.replace_range(a..=b.max(a), "1");
                    }
                    packs.push((kind == "filter", c.replace('\u{1}', "{intensity}")));
                }
            }
        }
        assert!(!packs.is_empty());
        let o = ChainOpts { t_offset: 0.0, apply_reverse: false, lut: None, matte: None };
        let mut failed = Vec::new();
        // (Breite, Höhe, Bilder, Stärken)
        for &(w, h, n, strengths) in sizes {
            let src = Src { w, h, has_alpha: false };
            let mut graphs: Vec<(String, String)> = Vec::new();
            for &i in strengths {
                for def in EFFECTS.iter().chain(FILTERS.iter()) {
                    let mut fx = ClipFx::default();
                    let r = FxRef { id: def.id.into(), intensity: i, params: Default::default() };
                    if FILTERS.iter().any(|f| f.id == def.id) {
                        fx.filter = Some(r);
                    } else {
                        fx.effects.push(r);
                    }
                    graphs.push((format!("{} {i}", def.id), fx_graph(&fx, src, &o, "0:v", "f", "x").join(";")));
                }
                for (filter, c) in &packs {
                    let mut g = GraphBuilder::new("0:v", "e");
                    if *filter {
                        g.blended(&c.replace("{intensity}", "1.000"), i);
                    } else {
                        g.push(c.replace("{intensity}", &format!("{i:.3}")));
                    }
                    graphs.push((format!("{c} {i}"), g.finish("f").join(";")));
                }
            }
            for (name, g) in graphs {
                let graph = format!("{g};[f]format=rgba[out]");
                let out = cl_media::ffmpeg_command()
                    .unwrap()
                    .args(["-v", "error", "-nostdin", "-threads", "2", "-filter_complex_threads", "2", "-f", "lavfi", "-i"])
                    // testsrc2 rundet auf gerade Maße: größer erzeugen, in RGBA zuschneiden
                    .arg(format!("testsrc2=s={}x{}:r=10:d={},format=rgba,crop={w}:{h}:0:0", w + 1, h + 1, n as f64 / 10.0))
                    .args(["-filter_complex", &graph, "-map", "[out]", "-frames:v", &n.to_string(), "-f", "rawvideo", "-"])
                    .stdin(Stdio::null())
                    .output()
                    .unwrap();
                if !out.status.success() {
                    failed.push(format!("{w}x{h} {name}: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
                } else if out.stdout.len() != (w * h * 4) as usize * n {
                    failed.push(format!("{w}x{h} {name}: {} statt {} Bytes", out.stdout.len(), (w * h * 4) as usize * n));
                }
            }
        }
        assert!(failed.is_empty(), "{} Ketten scheitern:\n{}", failed.len(), failed.join("\n"));
    }

    /// Zoom-Puls ändert weder Bildzahl noch Dauer – auch nicht bei 60 Bildern/s (mit `zoompan`
    /// wurden aus 1 s zwei).
    #[test]
    fn zoom_pulse_keeps_frames_and_duration_at_60_fps() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let def = effect("zoom-pulse").unwrap();
        let chain = (def.build)(1.0, Src { w: 320, h: 180, has_alpha: false });
        let out = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=60:d=1", "-vf", &chain])
            .args(["-f", "framecrc", "-"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout);
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(rows.len(), 60);
        // Spalte 2 = Zeitstempel des letzten Bildes in 1/60 s
        let last: i64 = rows[59].split(',').nth(2).unwrap().trim().parse().unwrap();
        assert_eq!(last, 59);
        assert!(rows.iter().all(|l| l.contains("86400")), "Bildgröße bleibt 320x180");
    }
}
