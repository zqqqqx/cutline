//! Eingebaute Sticker als SVG (eigene Zeichnungen, frei verwendbar). Sie werden mit
//! resvg zu PNG gerendert und dann wie ein Bild-Clip behandelt.

use std::path::Path;

pub struct Sticker {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    /// SVG mit viewBox 0 0 100 100
    pub svg: &'static str,
}

pub const STICKER_CATEGORIES: [&str; 5] = ["Formen", "Pfeile", "Sprechblasen", "Deko", "Creator"];

macro_rules! svg {
    ($body:expr) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">"#,
            $body,
            "</svg>"
        )
    };
}

pub static STICKERS: &[Sticker] = &[
    Sticker {
        id: "heart",
        name: "Herz",
        category: "Formen",
        svg: svg!(
            r##"<path d="M50 88 C20 66 6 50 6 32 C6 18 17 8 30 8 C39 8 46 13 50 20 C54 13 61 8 70 8 C83 8 94 18 94 32 C94 50 80 66 50 88Z" fill="#FF3B5C"/>"##
        ),
    },
    Sticker {
        id: "star",
        name: "Stern",
        category: "Formen",
        svg: svg!(
            r##"<path d="M50 6 L62 38 L96 38 L68 58 L79 92 L50 71 L21 92 L32 58 L4 38 L38 38Z" fill="#FFD400" stroke="#E0A800" stroke-width="2" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "circle",
        name: "Kreis",
        category: "Formen",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="42" fill="none" stroke="#FF3B30" stroke-width="7"/>"##
        ),
    },
    Sticker {
        id: "square",
        name: "Rahmen",
        category: "Formen",
        svg: svg!(
            r##"<rect x="8" y="8" width="84" height="84" rx="6" fill="none" stroke="#FFFFFF" stroke-width="6"/>"##
        ),
    },
    Sticker {
        id: "check",
        name: "Haken",
        category: "Formen",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="44" fill="#34C759"/><path d="M28 52 L44 67 L73 35" fill="none" stroke="#fff" stroke-width="10" stroke-linecap="round" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "cross",
        name: "Kreuz",
        category: "Formen",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="44" fill="#FF3B30"/><path d="M33 33 L67 67 M67 33 L33 67" stroke="#fff" stroke-width="10" stroke-linecap="round"/>"##
        ),
    },
    Sticker {
        id: "bolt",
        name: "Blitz",
        category: "Formen",
        svg: svg!(
            r##"<path d="M58 4 L18 56 L46 56 L38 96 L82 40 L54 40Z" fill="#FFD400" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "sun",
        name: "Sonne",
        category: "Formen",
        svg: svg!(
            r##"<g stroke="#FFB800" stroke-width="6" stroke-linecap="round"><path d="M50 4v14M50 82v14M4 50h14M82 50h14M17 17l10 10M73 73l10 10M17 83l10-10M73 27l10-10"/></g><circle cx="50" cy="50" r="22" fill="#FFD400"/>"##
        ),
    },
    Sticker {
        id: "moon",
        name: "Mond",
        category: "Formen",
        svg: svg!(r##"<path d="M64 8 A42 42 0 1 0 92 66 A34 34 0 1 1 64 8Z" fill="#FFE27A"/>"##),
    },
    Sticker {
        id: "cloud",
        name: "Wolke",
        category: "Formen",
        svg: svg!(
            r##"<path d="M26 78 C12 78 4 68 4 57 C4 46 13 38 24 38 C27 24 39 14 53 14 C69 14 81 26 82 41 C92 43 98 51 98 60 C98 70 90 78 80 78Z" fill="#FFFFFF" stroke="#D0D6E0" stroke-width="2"/>"##
        ),
    },
    Sticker {
        id: "arrow-right",
        name: "Pfeil rechts",
        category: "Pfeile",
        svg: svg!(
            r##"<path d="M6 42 H62 V22 L96 50 L62 78 V58 H6Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "arrow-down",
        name: "Pfeil runter",
        category: "Pfeile",
        svg: svg!(r##"<path d="M42 4 V60 H22 L50 96 L78 60 H58 V4Z" fill="#FF3B30"/>"##),
    },
    Sticker {
        id: "arrow-curve",
        name: "Bogenpfeil",
        category: "Pfeile",
        svg: svg!(
            r##"<path d="M12 80 C18 40 48 20 80 26" fill="none" stroke="#FFFFFF" stroke-width="7" stroke-linecap="round"/><path d="M66 12 L88 27 L68 42" fill="none" stroke="#FFFFFF" stroke-width="7" stroke-linecap="round" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "arrow-hand",
        name: "Handpfeil",
        category: "Pfeile",
        svg: svg!(
            r##"<path d="M10 70 Q30 30 60 46 T90 30" fill="none" stroke="#FFD400" stroke-width="6" stroke-linecap="round"/><path d="M76 20 L92 29 L82 45" fill="none" stroke="#FFD400" stroke-width="6" stroke-linecap="round" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "pointer",
        name: "Zeiger",
        category: "Pfeile",
        svg: svg!(
            r##"<path d="M20 8 L80 52 L54 56 L68 88 L56 93 L42 61 L22 78Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="4" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "bubble",
        name: "Sprechblase",
        category: "Sprechblasen",
        svg: svg!(
            r##"<path d="M14 10 H86 Q94 10 94 18 V62 Q94 70 86 70 H40 L20 90 L24 70 H14 Q6 70 6 62 V18 Q6 10 14 10Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/>"##
        ),
    },
    Sticker {
        id: "bubble-round",
        name: "Rund",
        category: "Sprechblasen",
        svg: svg!(
            r##"<ellipse cx="52" cy="44" rx="44" ry="34" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/><path d="M28 70 L16 94 L44 76" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "thought",
        name: "Gedanke",
        category: "Sprechblasen",
        svg: svg!(
            r##"<ellipse cx="54" cy="40" rx="42" ry="30" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/><circle cx="24" cy="78" r="8" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/><circle cx="12" cy="93" r="4" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="2"/>"##
        ),
    },
    Sticker {
        id: "shout",
        name: "Ausruf",
        category: "Sprechblasen",
        svg: svg!(
            r##"<path d="M50 4 L60 22 L80 12 L76 34 L97 38 L80 52 L95 70 L72 70 L74 93 L56 78 L44 96 L38 76 L16 88 L22 66 L3 58 L20 46 L6 28 L28 30 L28 8 L42 22Z" fill="#FFD400" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "sparkle",
        name: "Funkeln",
        category: "Deko",
        svg: svg!(
            r##"<path d="M50 4 C53 36 64 47 96 50 C64 53 53 64 50 96 C47 64 36 53 4 50 C36 47 47 36 50 4Z" fill="#FFFFFF"/><path d="M82 10 C83 18 86 21 94 22 C86 23 83 26 82 34 C81 26 78 23 70 22 C78 21 81 18 82 10Z" fill="#FFE27A"/>"##
        ),
    },
    Sticker {
        id: "crown",
        name: "Krone",
        category: "Deko",
        svg: svg!(
            r##"<path d="M8 30 L30 54 L50 18 L70 54 L92 30 L84 80 H16Z" fill="#FFD400" stroke="#C89400" stroke-width="3" stroke-linejoin="round"/><rect x="16" y="80" width="68" height="10" fill="#E0A800"/>"##
        ),
    },
    Sticker {
        id: "music",
        name: "Musiknote",
        category: "Deko",
        svg: svg!(
            r##"<path d="M38 72 V18 L84 8 V62" fill="none" stroke="#FFFFFF" stroke-width="7" stroke-linejoin="round"/><ellipse cx="28" cy="74" rx="13" ry="10" fill="#FFFFFF"/><ellipse cx="74" cy="64" rx="13" ry="10" fill="#FFFFFF"/>"##
        ),
    },
    Sticker {
        id: "smile",
        name: "Lächeln",
        category: "Deko",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="44" fill="#FFD400" stroke="#E0A800" stroke-width="3"/><circle cx="35" cy="40" r="6" fill="#1C1C1E"/><circle cx="65" cy="40" r="6" fill="#1C1C1E"/><path d="M28 60 Q50 82 72 60" fill="none" stroke="#1C1C1E" stroke-width="6" stroke-linecap="round"/>"##
        ),
    },
    Sticker {
        id: "underline",
        name: "Unterstrich",
        category: "Deko",
        svg: svg!(
            r##"<path d="M6 60 Q30 48 52 56 T94 50" fill="none" stroke="#FF3B5C" stroke-width="8" stroke-linecap="round"/>"##
        ),
    },
    Sticker {
        id: "marker",
        name: "Textmarker",
        category: "Deko",
        svg: svg!(r##"<path d="M6 38 L94 34 L92 66 L8 68Z" fill="#FFD400" fill-opacity="0.7"/>"##),
    },
    Sticker {
        id: "confetti",
        name: "Konfetti",
        category: "Deko",
        svg: svg!(
            r##"<rect x="14" y="18" width="8" height="14" rx="2" fill="#FF3B5C" transform="rotate(-20 18 25)"/><rect x="70" y="12" width="8" height="14" rx="2" fill="#34C759" transform="rotate(25 74 19)"/><rect x="44" y="40" width="8" height="14" rx="2" fill="#FFD400" transform="rotate(10 48 47)"/><rect x="20" y="68" width="8" height="14" rx="2" fill="#5AC8FA" transform="rotate(35 24 75)"/><rect x="76" y="62" width="8" height="14" rx="2" fill="#AF52DE" transform="rotate(-30 80 69)"/><circle cx="58" cy="82" r="4" fill="#FF9500"/><circle cx="34" cy="44" r="3" fill="#FFFFFF"/><circle cx="86" cy="40" r="3" fill="#FFD400"/>"##
        ),
    },
    Sticker {
        id: "sub-button",
        name: "Abo-Knopf",
        category: "Creator",
        svg: svg!(
            r##"<rect x="4" y="30" width="92" height="40" rx="20" fill="#FF2D2D"/><path d="M40 40 L62 50 L40 60Z" fill="#FFFFFF"/>"##
        ),
    },
    Sticker {
        id: "bell",
        name: "Glocke",
        category: "Creator",
        svg: svg!(
            r##"<path d="M50 10 C30 10 24 28 24 44 V62 L14 74 H86 L76 62 V44 C76 28 70 10 50 10Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/><circle cx="50" cy="84" r="8" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/><circle cx="74" cy="22" r="10" fill="#FF2D2D"/>"##
        ),
    },
    Sticker {
        id: "thumbs-up",
        name: "Daumen hoch",
        category: "Creator",
        svg: svg!(
            r##"<path d="M10 46 H26 V90 H10Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3"/><path d="M30 46 L48 12 C56 10 60 16 58 26 L54 40 H82 C90 40 94 48 90 56 L82 84 C80 88 76 90 72 90 H30Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "comment",
        name: "Kommentar",
        category: "Creator",
        svg: svg!(
            r##"<path d="M12 14 H88 Q94 14 94 20 V64 Q94 70 88 70 H44 L24 88 V70 H12 Q6 70 6 64 V20 Q6 14 12 14Z" fill="#3A8DFF"/><circle cx="32" cy="42" r="5" fill="#FFFFFF"/><circle cx="50" cy="42" r="5" fill="#FFFFFF"/><circle cx="68" cy="42" r="5" fill="#FFFFFF"/>"##
        ),
    },
    Sticker {
        id: "share",
        name: "Teilen",
        category: "Creator",
        svg: svg!(
            r##"<path d="M58 14 L94 46 L58 78 V60 C34 60 18 68 6 88 C10 58 26 34 58 32Z" fill="#FFFFFF" stroke="#1C1C1E" stroke-width="3" stroke-linejoin="round"/>"##
        ),
    },
    Sticker {
        id: "live",
        name: "Live",
        category: "Creator",
        svg: svg!(
            r##"<rect x="6" y="30" width="88" height="40" rx="8" fill="#FF2D2D"/><circle cx="24" cy="50" r="7" fill="#FFFFFF"/><rect x="38" y="44" width="46" height="12" rx="6" fill="#FFFFFF"/>"##
        ),
    },
    Sticker {
        id: "webcam-frame",
        name: "Webcam-Rahmen",
        category: "Creator",
        svg: svg!(
            r##"<rect x="6" y="18" width="88" height="64" rx="10" fill="none" stroke="#FFFFFF" stroke-width="5"/><rect x="11" y="23" width="78" height="54" rx="6" fill="none" stroke="#7C5CFF" stroke-width="3"/>"##
        ),
    },
    Sticker {
        id: "progress",
        name: "Fortschrittsbalken",
        category: "Creator",
        svg: svg!(
            r##"<rect x="4" y="44" width="92" height="12" rx="6" fill="#FFFFFF" fill-opacity="0.35"/><rect x="4" y="44" width="58" height="12" rx="6" fill="#FF2D2D"/><circle cx="62" cy="50" r="9" fill="#FF2D2D"/>"##
        ),
    },
];

pub fn sticker(id: &str) -> Option<&'static Sticker> {
    STICKERS.iter().find(|s| s.id == id)
}

/// Beliebiges SVG quadratisch rendern (vormultipliziertes RGBA).
pub fn render_svg(svg: &str, size: u32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(size, size)?;
    let s = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(s, s),
        &mut pm.as_mut(),
    );
    Some(pm.take())
}

/// Rendert einen Sticker quadratisch (`size` px) als vormultipliziertes RGBA.
pub fn render_rgba(st: &Sticker, size: u32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(st.svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(size, size)?;
    let s = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(s, s),
        &mut pm.as_mut(),
    );
    Some(pm.take())
}

/// Rendert einen Sticker als PNG-Datei.
pub fn render_png(st: &Sticker, size: u32, out: &Path) -> Result<(), String> {
    let tree = resvg::usvg::Tree::from_str(st.svg, &resvg::usvg::Options::default())
        .map_err(|e| e.to_string())?;
    let mut pm = resvg::tiny_skia::Pixmap::new(size, size).ok_or("Größe")?;
    let s = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(s, s),
        &mut pm.as_mut(),
    );
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    pm.save_png(out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_stickers_render() {
        for st in STICKERS {
            let px = render_rgba(st, 64).unwrap_or_else(|| panic!("{}", st.id));
            assert!(px.chunks(4).any(|p| p[3] > 0), "{} ist leer", st.id);
            assert!(STICKER_CATEGORIES.contains(&st.category));
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s/heart.png");
        render_png(&STICKERS[0], 256, &p).unwrap();
        assert!(std::fs::metadata(&p).unwrap().len() > 500);
        assert!(STICKERS.len() >= 24);
    }
}
