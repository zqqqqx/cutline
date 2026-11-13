//! Textvorlagen (fertige Titel mit Stil und Animation) und Texteffekte
//! (Stil-Voreinstellungen, die nur das Aussehen ändern und den Text behalten).

use cl_core::TextStyle;

pub struct TextTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub text: &'static str,
    /// Einblend-Animation (id aus `cl_core::ANIMATIONS`)
    pub intro: Option<&'static str>,
    pub style: fn() -> TextStyle,
}

fn base(text: &str) -> TextStyle {
    TextStyle {
        text: text.into(),
        ..Default::default()
    }
}

pub const TEMPLATE_CATEGORIES: [&str; 4] = ["Titel", "Untertitel", "Social", "Hinweis"];

pub static TEMPLATES: &[TextTemplate] = &[
    TextTemplate {
        id: "headline",
        name: "Schlagzeile",
        category: "Titel",
        text: "SCHLAGZEILE",
        intro: Some("pop"),
        style: || TextStyle {
            size: 110.0,
            bold: true,
            uppercase: true,
            outline: Some((6.0, [0, 0, 0, 255])),
            ..base("Schlagzeile")
        },
    },
    TextTemplate {
        id: "cinema-title",
        name: "Kinotitel",
        category: "Titel",
        text: "D E R  F I L M",
        intro: Some("fade"),
        style: || TextStyle {
            size: 72.0,
            font_family: Some("Georgia".into()),
            color: [240, 232, 214, 255],
            shadow: false,
            line_spacing: 1.3,
            ..base("D E R  F I L M")
        },
    },
    TextTemplate {
        id: "impact",
        name: "Impact",
        category: "Titel",
        text: "WOW!",
        intro: Some("zoom-in"),
        style: || TextStyle {
            size: 150.0,
            font_family: Some("Impact".into()),
            color: [255, 214, 0, 255],
            outline: Some((8.0, [20, 20, 20, 255])),
            ..base("WOW!")
        },
    },
    TextTemplate {
        id: "minimal",
        name: "Minimal",
        category: "Titel",
        text: "minimal",
        intro: Some("rise"),
        style: || TextStyle {
            size: 64.0,
            color: [255, 255, 255, 230],
            shadow: false,
            ..base("minimal")
        },
    },
    TextTemplate {
        id: "neon",
        name: "Neon",
        category: "Titel",
        text: "NEON",
        intro: Some("fade"),
        style: || TextStyle {
            size: 120.0,
            bold: true,
            color: [255, 80, 220, 255],
            outline: Some((3.0, [255, 210, 250, 255])),
            ..base("NEON")
        },
    },
    TextTemplate {
        id: "chapter",
        name: "Kapitel",
        category: "Titel",
        text: "Kapitel 1",
        intro: Some("slide-left"),
        style: || TextStyle {
            size: 80.0,
            italic: true,
            font_family: Some("Georgia".into()),
            background: Some([0, 0, 0, 160]),
            ..base("Kapitel 1")
        },
    },
    TextTemplate {
        id: "subtitle-clean",
        name: "Klar",
        category: "Untertitel",
        text: "So sehen Untertitel aus",
        intro: None,
        style: || TextStyle {
            size: 54.0,
            outline: Some((3.0, [0, 0, 0, 255])),
            shadow: false,
            ..base("So sehen Untertitel aus")
        },
    },
    TextTemplate {
        id: "subtitle-box",
        name: "Mit Fläche",
        category: "Untertitel",
        text: "Untertitel mit Fläche",
        intro: None,
        style: || TextStyle {
            size: 50.0,
            background: Some([0, 0, 0, 170]),
            shadow: false,
            ..base("Untertitel mit Fläche")
        },
    },
    TextTemplate {
        id: "subtitle-yellow",
        name: "Gelb",
        category: "Untertitel",
        text: "Gelbe Untertitel",
        intro: None,
        style: || TextStyle {
            size: 54.0,
            bold: true,
            color: [255, 221, 0, 255],
            outline: Some((3.0, [0, 0, 0, 255])),
            ..base("Gelbe Untertitel")
        },
    },
    TextTemplate {
        id: "karaoke",
        name: "Karaoke",
        category: "Untertitel",
        text: "Sing mit",
        intro: Some("pop"),
        style: || TextStyle {
            size: 64.0,
            bold: true,
            color: [80, 220, 255, 255],
            outline: Some((4.0, [255, 255, 255, 255])),
            ..base("Sing mit")
        },
    },
    TextTemplate {
        id: "follow",
        name: "Folgen",
        category: "Social",
        text: "Folgen für mehr",
        intro: Some("slide-up"),
        style: || TextStyle {
            size: 58.0,
            bold: true,
            color: [20, 20, 20, 255],
            background: Some([255, 255, 255, 235]),
            shadow: false,
            ..base("Folgen für mehr")
        },
    },
    TextTemplate {
        id: "comment",
        name: "Kommentar",
        category: "Social",
        text: "Was meint ihr?",
        intro: Some("pop"),
        style: || TextStyle {
            size: 56.0,
            color: [255, 255, 255, 255],
            background: Some([255, 59, 92, 230]),
            shadow: false,
            ..base("Was meint ihr?")
        },
    },
    TextTemplate {
        id: "pov",
        name: "POV",
        category: "Social",
        text: "POV: du schneidest mit Cutline",
        intro: Some("fade"),
        style: || TextStyle {
            size: 52.0,
            outline: Some((2.0, [0, 0, 0, 200])),
            ..base("POV: du schneidest mit Cutline")
        },
    },
    TextTemplate {
        id: "price",
        name: "Preis",
        category: "Social",
        text: "−30 %",
        intro: Some("zoom-in"),
        style: || TextStyle {
            size: 130.0,
            bold: true,
            color: [255, 255, 255, 255],
            background: Some([230, 40, 40, 255]),
            shadow: false,
            ..base("−30 %")
        },
    },
    TextTemplate {
        id: "note",
        name: "Hinweis",
        category: "Hinweis",
        text: "Hinweis",
        intro: Some("slide-right"),
        style: || TextStyle {
            size: 46.0,
            color: [20, 20, 20, 255],
            background: Some([255, 214, 0, 240]),
            shadow: false,
            ..base("Hinweis")
        },
    },
    TextTemplate {
        id: "lower-third",
        name: "Bauchbinde",
        category: "Hinweis",
        text: "Name Nachname\nFunktion",
        intro: Some("slide-left"),
        style: || TextStyle {
            size: 44.0,
            align: -1,
            background: Some([15, 15, 18, 210]),
            shadow: false,
            line_spacing: 1.2,
            ..base("Name Nachname\nFunktion")
        },
    },
    TextTemplate {
        id: "location",
        name: "Ort",
        category: "Hinweis",
        text: "Berlin, 2026",
        intro: Some("rise"),
        style: || TextStyle {
            size: 48.0,
            italic: true,
            shadow: true,
            ..base("Berlin, 2026")
        },
    },
    TextTemplate {
        id: "warning",
        name: "Achtung",
        category: "Hinweis",
        text: "ACHTUNG",
        intro: Some("swing"),
        style: || TextStyle {
            size: 90.0,
            bold: true,
            uppercase: true,
            color: [255, 214, 0, 255],
            outline: Some((5.0, [0, 0, 0, 255])),
            ..base("ACHTUNG")
        },
    },
];

impl TextTemplate {
    pub fn make(&self) -> TextStyle {
        let mut s = (self.style)();
        s.text = self.text.into();
        s
    }
}

/// Texteffekte: ändern das Aussehen eines bestehenden Textes.
pub struct TextEffect {
    pub id: &'static str,
    pub name: &'static str,
    pub apply: fn(&mut TextStyle),
}

pub static TEXT_EFFECTS: &[TextEffect] = &[
    TextEffect {
        id: "plain",
        name: "Schlicht",
        apply: |s| {
            s.outline = None;
            s.background = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "shadow",
        name: "Schatten",
        apply: |s| {
            s.outline = None;
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "outline-black",
        name: "Kontur",
        apply: |s| {
            s.outline = Some((4.0, [0, 0, 0, 255]));
            s.background = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "outline-white",
        name: "Kontur hell",
        apply: |s| {
            s.outline = Some((4.0, [255, 255, 255, 255]));
            s.background = None;
            s.shadow = false;
            s.color = [20, 20, 20, 255];
        },
    },
    TextEffect {
        id: "yellow",
        name: "Gelb",
        apply: |s| {
            s.color = [255, 221, 0, 255];
            s.outline = Some((3.0, [0, 0, 0, 255]));
            s.background = None;
        },
    },
    TextEffect {
        id: "neon-pink",
        name: "Neon Pink",
        apply: |s| {
            s.color = [255, 80, 220, 255];
            s.outline = Some((2.0, [255, 210, 250, 255]));
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "neon-blue",
        name: "Neon Blau",
        apply: |s| {
            s.color = [70, 200, 255, 255];
            s.outline = Some((2.0, [210, 245, 255, 255]));
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "box-dark",
        name: "Fläche dunkel",
        apply: |s| {
            s.background = Some([0, 0, 0, 180]);
            s.outline = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "box-light",
        name: "Fläche hell",
        apply: |s| {
            s.background = Some([255, 255, 255, 235]);
            s.outline = None;
            s.shadow = false;
            s.color = [20, 20, 20, 255];
        },
    },
    TextEffect {
        id: "box-red",
        name: "Fläche rot",
        apply: |s| {
            s.background = Some([230, 40, 40, 255]);
            s.outline = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "box-yellow",
        name: "Marker",
        apply: |s| {
            s.background = Some([255, 214, 0, 240]);
            s.outline = None;
            s.shadow = false;
            s.color = [20, 20, 20, 255];
        },
    },
    TextEffect {
        id: "gold",
        name: "Gold",
        apply: |s| {
            s.color = [240, 200, 90, 255];
            s.outline = Some((2.0, [90, 60, 10, 255]));
            s.background = None;
            s.shadow = true;
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_effects_are_consistent() {
        let ids: std::collections::HashSet<_> = TEMPLATES.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), TEMPLATES.len());
        for t in TEMPLATES {
            assert!(TEMPLATE_CATEGORIES.contains(&t.category), "{}", t.id);
            assert_eq!(t.make().text, t.text);
            if let Some(a) = t.intro {
                assert!(cl_core::ANIMATIONS.iter().any(|x| x.0 == a), "{a}");
            }
        }
        let mut s = TextStyle::default();
        for e in TEXT_EFFECTS {
            (e.apply)(&mut s);
            assert_eq!(s.text, "Standardtext");
        }
    }
}
