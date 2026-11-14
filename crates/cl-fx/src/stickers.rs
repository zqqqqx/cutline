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

pub const STICKER_CATEGORIES: [&str; 7] =
    ["Formen", "Pfeile", "Sprechblasen", "Deko", "Creator", "Gaming", "Reaktionen"];

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
    Sticker {
        id: "play-button",
        name: "Play-Knopf",
        category: "Creator",
        svg: svg!(
            r##"<rect x="6" y="22" width="88" height="56" rx="16" fill="#FF1E1E"/><path d="M42 36 L64 50 L42 64Z" fill="#FFFFFF"/>"##
        ),
    },
    Sticker {
        id: "new-badge",
        name: "Neu",
        category: "Creator",
        svg: svg!(
            r##"<path d="M50 4 L58 16 L72 10 L74 25 L89 26 L84 40 L96 50 L84 60 L89 74 L74 75 L72 90 L58 84 L50 96 L42 84 L28 90 L26 75 L11 74 L16 60 L4 50 L16 40 L11 26 L26 25 L28 10 L42 16Z" fill="#FFD400" stroke="#1A1A1A" stroke-width="3" stroke-linejoin="round"/><text x="50" y="58" font-family="Anton" font-size="24" text-anchor="middle" fill="#1A1A1A">NEW</text>"##
        ),
    },
    Sticker {
        id: "circle-highlight",
        name: "Einkreisen",
        category: "Creator",
        svg: svg!(
            r##"<path d="M20 30 C34 8 82 10 90 40 C98 70 60 92 32 84 C10 78 4 52 18 34 C24 26 32 22 40 20" fill="none" stroke="#FF2D2D" stroke-width="5" stroke-linecap="round"/>"##
        ),
    },
    Sticker {
        id: "crosshair",
        name: "Fadenkreuz",
        category: "Gaming",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="30" fill="none" stroke="#FF3B3B" stroke-width="4"/><circle cx="50" cy="50" r="4" fill="#FF3B3B"/><path d="M50 8 V30 M50 70 V92 M8 50 H30 M70 50 H92" stroke="#FF3B3B" stroke-width="4" stroke-linecap="round"/>"##
        ),
    },
    Sticker {
        id: "healthbar",
        name: "Lebensleiste",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M10 40 C10 32 18 30 22 36 C26 30 34 32 34 40 C34 48 22 56 22 56 C22 56 10 48 10 40Z" fill="#FF3B5C"/><rect x="38" y="38" width="56" height="16" rx="4" fill="#1A1A1A" stroke="#FFFFFF" stroke-width="2"/><rect x="41" y="41" width="34" height="10" rx="2" fill="#3DDC6B"/>"##
        ),
    },
    Sticker {
        id: "trophy",
        name: "Pokal",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M28 12 H72 V38 C72 52 62 62 50 62 C38 62 28 52 28 38Z" fill="#FFC81E"/><path d="M28 18 H14 C14 34 20 42 30 44 M72 18 H86 C86 34 80 42 70 44" fill="none" stroke="#FFC81E" stroke-width="6"/><rect x="44" y="62" width="12" height="14" fill="#E0A800"/><rect x="30" y="76" width="40" height="12" rx="3" fill="#8B5A2B"/><path d="M50 22 L54 32 L64 32 L56 38 L59 48 L50 42 L41 48 L44 38 L36 32 L46 32Z" fill="#FFF4C2"/>"##
        ),
    },
    Sticker {
        id: "controller",
        name: "Controller",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M24 30 H76 C88 30 96 50 96 66 C96 78 84 82 76 72 L68 62 H32 L24 72 C16 82 4 78 4 66 C4 50 12 30 24 30Z" fill="#2B2D42"/><path d="M26 42 V58 M18 50 H34" stroke="#FFFFFF" stroke-width="5" stroke-linecap="round"/><circle cx="70" cy="44" r="4.5" fill="#3DDC6B"/><circle cx="80" cy="52" r="4.5" fill="#FF3B3B"/><circle cx="60" cy="52" r="4.5" fill="#2D9CFF"/><circle cx="70" cy="60" r="4.5" fill="#FFD400"/>"##
        ),
    },
    Sticker {
        id: "flame",
        name: "Flamme",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M50 4 C58 24 78 34 78 60 C78 80 64 94 50 94 C36 94 22 80 22 60 C22 44 32 36 36 24 C40 34 44 38 48 40 C48 26 46 16 50 4Z" fill="#FF5A1F"/><path d="M50 44 C56 56 66 62 66 74 C66 84 58 92 50 92 C42 92 34 84 34 74 C34 64 42 60 44 52 C46 58 48 60 50 60Z" fill="#FFC81E"/>"##
        ),
    },
    Sticker {
        id: "skull",
        name: "Totenkopf",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M50 8 C28 8 14 24 14 44 C14 58 22 66 28 70 V84 H72 V70 C78 66 86 58 86 44 C86 24 72 8 50 8Z" fill="#F2F2F2" stroke="#1A1A1A" stroke-width="3"/><circle cx="36" cy="46" r="10" fill="#1A1A1A"/><circle cx="64" cy="46" r="10" fill="#1A1A1A"/><path d="M50 58 L44 68 H56Z" fill="#1A1A1A"/><path d="M40 84 V74 M50 84 V74 M60 84 V74" stroke="#1A1A1A" stroke-width="3"/>"##
        ),
    },
    Sticker {
        id: "gg",
        name: "GG",
        category: "Gaming",
        svg: svg!(
            r##"<rect x="8" y="22" width="84" height="56" rx="12" fill="#7B2FF7"/><text x="50" y="65" font-family="Anton" font-size="38" text-anchor="middle" fill="#FFFFFF">GG</text>"##
        ),
    },
    Sticker {
        id: "boom",
        name: "Boom",
        category: "Gaming",
        svg: svg!(
            r##"<path d="M50 2 L60 26 L84 12 L74 38 L98 44 L76 58 L90 80 L64 72 L58 98 L46 76 L26 92 L30 66 L4 64 L24 48 L8 28 L34 32 L36 8Z" fill="#FFD400" stroke="#FF3B1F" stroke-width="4" stroke-linejoin="round"/><text x="51" y="60" font-family="Anton" font-size="22" text-anchor="middle" fill="#FF3B1F">BOOM</text>"##
        ),
    },
    Sticker {
        id: "hundred",
        name: "100",
        category: "Reaktionen",
        svg: svg!(
            r##"<text x="50" y="62" font-family="Anton" font-size="46" text-anchor="middle" fill="#FF2D2D">100</text><path d="M14 72 C40 66 64 66 88 70 M18 80 C42 75 62 75 84 78" stroke="#FF2D2D" stroke-width="5" stroke-linecap="round" fill="none"/>"##
        ),
    },
    Sticker {
        id: "laugh",
        name: "Lachen",
        category: "Reaktionen",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="44" fill="#FFCC33"/><path d="M26 40 Q34 32 42 40 M58 40 Q66 32 74 40" stroke="#5A3A12" stroke-width="5" fill="none" stroke-linecap="round"/><path d="M24 54 H76 C76 72 64 84 50 84 C36 84 24 72 24 54Z" fill="#5A3A12"/><path d="M32 62 H68 C66 70 58 76 50 76 C42 76 34 70 32 62Z" fill="#FF6B81"/><path d="M18 44 C12 52 12 60 18 62 C22 58 22 50 18 44Z M82 44 C88 52 88 60 82 62 C78 58 78 50 82 44Z" fill="#4FC3F7"/>"##
        ),
    },
    Sticker {
        id: "shock",
        name: "Schock",
        category: "Reaktionen",
        svg: svg!(
            r##"<circle cx="50" cy="50" r="44" fill="#FFCC33"/><circle cx="35" cy="40" r="8" fill="#FFFFFF" stroke="#5A3A12" stroke-width="3"/><circle cx="65" cy="40" r="8" fill="#FFFFFF" stroke="#5A3A12" stroke-width="3"/><circle cx="35" cy="40" r="3" fill="#5A3A12"/><circle cx="65" cy="40" r="3" fill="#5A3A12"/><ellipse cx="50" cy="70" rx="10" ry="14" fill="#5A3A12"/>"##
        ),
    },
    Sticker {
        id: "question",
        name: "Fragezeichen",
        category: "Reaktionen",
        svg: svg!(
            r##"<path d="M30 32 C30 16 42 8 52 8 C66 8 74 18 74 30 C74 46 54 48 54 64 V68" fill="none" stroke="#FF2D2D" stroke-width="12" stroke-linecap="round"/><circle cx="54" cy="86" r="7" fill="#FF2D2D"/>"##
        ),
    },
    Sticker {
        id: "exclamation",
        name: "Ausrufezeichen",
        category: "Reaktionen",
        svg: svg!(
            r##"<path d="M50 10 V66" stroke="#FF2D2D" stroke-width="14" stroke-linecap="round"/><circle cx="50" cy="86" r="8" fill="#FF2D2D"/>"##
        ),
    },
];

pub fn sticker(id: &str) -> Option<&'static Sticker> {
    STICKERS.iter().find(|s| s.id == id)
}

/// SVG-Optionen mit der mitgelieferten Schrift „Anton“ (OFL) für Sticker mit Text
fn options() -> &'static resvg::usvg::Options<'static> {
    static OPT: std::sync::OnceLock<resvg::usvg::Options<'static>> = std::sync::OnceLock::new();
    OPT.get_or_init(|| {
        let mut o = resvg::usvg::Options::default();
        o.fontdb_mut()
            .load_font_data(include_bytes!("../../cl-app/assets/fonts/Anton-Regular.ttf").to_vec());
        o
    })
}

/// Beliebiges SVG quadratisch rendern (vormultipliziertes RGBA).
pub fn render_svg(svg: &str, size: u32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(svg, options()).ok()?;
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
    let tree = resvg::usvg::Tree::from_str(st.svg, options()).ok()?;
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
    let tree = resvg::usvg::Tree::from_str(st.svg, options())
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
        let mut ids: Vec<&str> = STICKERS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), STICKERS.len());
        assert!(STICKERS.len() >= 50);
        // Text in Stickern wird mit der eingebetteten Schrift gezeichnet (sonst bliebe er leer)
        let with_text = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text x="50" y="70" font-family="Anton" font-size="60" text-anchor="middle" fill="#FF0000">GG</text></svg>"##;
        let px = render_svg(with_text, 64).unwrap();
        assert!(px.chunks(4).any(|p| p[3] > 0), "Text nicht gezeichnet");
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s/heart.png");
        render_png(&STICKERS[0], 256, &p).unwrap();
        assert!(std::fs::metadata(&p).unwrap().len() > 500);
        assert!(STICKERS.len() >= 24);
    }
}
