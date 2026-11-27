//! Text templates (finished titles with style and animation) and text effects
//! (style presets that only change the look and keep the text).

use cl_core::TextStyle;

pub struct TextTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub text: &'static str,
    /// In animation (id from `cl_core::ANIMATIONS`)
    pub intro: Option<&'static str>,
    pub style: fn() -> TextStyle,
}

fn base(text: &str) -> TextStyle {
    TextStyle {
        text: text.into(),
        ..Default::default()
    }
}

pub const TEMPLATE_CATEGORIES: [&str; 5] = ["Gaming", "Title", "Captions", "Social", "Note"];

pub static TEMPLATES: &[TextTemplate] = &[
    TextTemplate {
        id: "victory",
        name: "Victory",
        category: "Gaming",
        text: "VICTORY!",
        intro: Some("pop"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Lilita One".into()),
            color: [255, 214, 0, 255],
            outline: Some((9e+00, [25, 10, 0, 255])),
            shadow: true,
            uppercase: false,
            ..base("VICTORY!")
        },
    },
    TextTemplate {
        id: "headshot",
        name: "Headshot",
        category: "Gaming",
        text: "HEADSHOT",
        intro: Some("zoom-in"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Black Ops One".into()),
            color: [255, 64, 64, 255],
            outline: Some((6e+00, [20, 0, 0, 255])),
            shadow: true,
            uppercase: false,
            ..base("HEADSHOT")
        },
    },
    TextTemplate {
        id: "arcade",
        name: "Arcade",
        category: "Gaming",
        text: "GAME OVER",
        intro: Some("fade"),
        style: || TextStyle {
            size: 6e+01,
            font_family: Some("Press Start 2P".into()),
            color: [255, 255, 255, 255],
            outline: Some((5e+00, [230, 0, 120, 255])),
            shadow: true,
            uppercase: false,
            ..base("GAME OVER")
        },
    },
    TextTemplate {
        id: "lets-play",
        name: "Let's Play",
        category: "Gaming",
        text: "LET'S PLAY",
        intro: Some("pop"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Bungee".into()),
            color: [0, 235, 255, 255],
            outline: Some((5e+00, [0, 30, 70, 255])),
            shadow: true,
            uppercase: false,
            ..base("LET'S PLAY")
        },
    },
    TextTemplate {
        id: "thumbnail",
        name: "Thumbnail",
        category: "Gaming",
        text: "NO WAY?!",
        intro: Some("zoom-in"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Anton".into()),
            color: [255, 255, 255, 255],
            outline: Some((1e+01, [0, 0, 0, 255])),
            shadow: true,
            uppercase: true,
            ..base("NO WAY?!")
        },
    },
    TextTemplate {
        id: "comic",
        name: "Comic",
        category: "Gaming",
        text: "BOOM!",
        intro: Some("pop"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Bangers".into()),
            color: [255, 220, 40, 255],
            outline: Some((8e+00, [0, 0, 0, 255])),
            shadow: true,
            uppercase: false,
            ..base("BOOM!")
        },
    },
    TextTemplate {
        id: "episode",
        name: "Episode",
        category: "Gaming",
        text: "EPISODE 1",
        intro: Some("rise"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Bebas Neue".into()),
            color: [255, 255, 255, 255],
            shadow: true,
            uppercase: false,
            ..base("EPISODE 1")
        },
    },
    TextTemplate {
        id: "esports",
        name: "Esports",
        category: "Gaming",
        text: "TEAM CUTLINE",
        intro: Some("slide-left"),
        style: || TextStyle {
            size: 7e+01,
            font_family: Some("Russo One".into()),
            color: [255, 255, 255, 255],
            background: Some([0, 150, 255, 235]),
            shadow: false,
            uppercase: false,
            ..base("TEAM CUTLINE")
        },
    },
    TextTemplate {
        id: "nice",
        name: "Nice",
        category: "Gaming",
        text: "NICE!",
        intro: Some("pop"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Luckiest Guy".into()),
            color: [255, 140, 0, 255],
            outline: Some((7e+00, [255, 255, 255, 255])),
            shadow: true,
            uppercase: false,
            ..base("NICE!")
        },
    },
    TextTemplate {
        id: "level-up",
        name: "Level up",
        category: "Gaming",
        text: "LEVEL UP",
        intro: Some("rise"),
        style: || TextStyle {
            size: 1e+02,
            font_family: Some("Titan One".into()),
            color: [90, 255, 130, 255],
            outline: Some((7e+00, [0, 40, 10, 255])),
            shadow: true,
            uppercase: false,
            ..base("LEVEL UP")
        },
    },
    TextTemplate {
        id: "subscribe-box",
        name: "Subscribe callout",
        category: "Gaming",
        text: "SUBSCRIBE",
        intro: Some("pop"),
        style: || TextStyle {
            size: 6e+01,
            font_family: Some("Poppins".into()),
            color: [255, 255, 255, 255],
            background: Some([230, 30, 40, 255]),
            shadow: false,
            uppercase: false,
            ..base("SUBSCRIBE")
        },
    },
    TextTemplate {
        id: "headline",
        name: "Headline",
        category: "Title",
        text: "HEADLINE",
        intro: Some("pop"),
        style: || TextStyle {
            size: 110.0,
            bold: true,
            uppercase: true,
            outline: Some((6.0, [0, 0, 0, 255])),
            ..base("Headline")
        },
    },
    TextTemplate {
        id: "cinema-title",
        name: "Movie title",
        category: "Title",
        text: "T H E  M O V I E",
        intro: Some("fade"),
        style: || TextStyle {
            size: 72.0,
            font_family: Some("Georgia".into()),
            color: [240, 232, 214, 255],
            shadow: false,
            line_spacing: 1.3,
            ..base("T H E  M O V I E")
        },
    },
    TextTemplate {
        id: "impact",
        name: "Impact",
        category: "Title",
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
        category: "Title",
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
        category: "Title",
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
        name: "Chapter",
        category: "Title",
        text: "Chapter 1",
        intro: Some("slide-left"),
        style: || TextStyle {
            size: 80.0,
            italic: true,
            font_family: Some("Georgia".into()),
            background: Some([0, 0, 0, 160]),
            ..base("Chapter 1")
        },
    },
    TextTemplate {
        id: "subtitle-clean",
        name: "Clear",
        category: "Captions",
        text: "This is how captions look",
        intro: None,
        style: || TextStyle {
            size: 54.0,
            outline: Some((3.0, [0, 0, 0, 255])),
            shadow: false,
            ..base("This is how captions look")
        },
    },
    TextTemplate {
        id: "subtitle-box",
        name: "With box",
        category: "Captions",
        text: "Captions with a box",
        intro: None,
        style: || TextStyle {
            size: 50.0,
            background: Some([0, 0, 0, 170]),
            shadow: false,
            ..base("Captions with a box")
        },
    },
    TextTemplate {
        id: "subtitle-yellow",
        name: "Yellow",
        category: "Captions",
        text: "Yellow captions",
        intro: None,
        style: || TextStyle {
            size: 54.0,
            bold: true,
            color: [255, 221, 0, 255],
            outline: Some((3.0, [0, 0, 0, 255])),
            ..base("Yellow captions")
        },
    },
    TextTemplate {
        id: "karaoke",
        name: "Karaoke",
        category: "Captions",
        text: "Sing along",
        intro: Some("pop"),
        style: || TextStyle {
            size: 64.0,
            bold: true,
            color: [80, 220, 255, 255],
            outline: Some((4.0, [255, 255, 255, 255])),
            ..base("Sing along")
        },
    },
    TextTemplate {
        id: "follow",
        name: "Follow",
        category: "Social",
        text: "Follow for more",
        intro: Some("slide-up"),
        style: || TextStyle {
            size: 58.0,
            bold: true,
            color: [20, 20, 20, 255],
            background: Some([255, 255, 255, 235]),
            shadow: false,
            ..base("Follow for more")
        },
    },
    TextTemplate {
        id: "comment",
        name: "Comment",
        category: "Social",
        text: "What do you think?",
        intro: Some("pop"),
        style: || TextStyle {
            size: 56.0,
            color: [255, 255, 255, 255],
            background: Some([255, 59, 92, 230]),
            shadow: false,
            ..base("What do you think?")
        },
    },
    TextTemplate {
        id: "pov",
        name: "POV",
        category: "Social",
        text: "POV: you edit with Cutline",
        intro: Some("fade"),
        style: || TextStyle {
            size: 52.0,
            outline: Some((2.0, [0, 0, 0, 200])),
            ..base("POV: you edit with Cutline")
        },
    },
    TextTemplate {
        id: "price",
        name: "Price",
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
        name: "Note",
        category: "Note",
        text: "Note",
        intro: Some("slide-right"),
        style: || TextStyle {
            size: 46.0,
            color: [20, 20, 20, 255],
            background: Some([255, 214, 0, 240]),
            shadow: false,
            ..base("Note")
        },
    },
    TextTemplate {
        id: "lower-third",
        name: "Lower third",
        category: "Note",
        text: "First Last\nRole",
        intro: Some("slide-left"),
        style: || TextStyle {
            size: 44.0,
            align: -1,
            background: Some([15, 15, 18, 210]),
            shadow: false,
            line_spacing: 1.2,
            ..base("First Last\nRole")
        },
    },
    TextTemplate {
        id: "location",
        name: "Location~place",
        category: "Note",
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
        name: "Attention",
        category: "Note",
        text: "ATTENTION",
        intro: Some("swing"),
        style: || TextStyle {
            size: 90.0,
            bold: true,
            uppercase: true,
            color: [255, 214, 0, 255],
            outline: Some((5.0, [0, 0, 0, 255])),
            ..base("ATTENTION")
        },
    },
    TextTemplate {
        id: "chapter-title",
        name: "Chapter title",
        category: "Title",
        text: "CHAPTER 2",
        intro: Some("wipe-fade"),
        style: || TextStyle {
            size: 96.0,
            font_family: Some("Fjalla One".into()),
            color: [255, 255, 255, 255],
            shadow: true,
            ..base("CHAPTER 2")
        },
    },
    TextTemplate {
        id: "part-two",
        name: "Part 2",
        category: "Title",
        text: "PART 2",
        intro: Some("stamp"),
        style: || TextStyle {
            size: 120.0,
            font_family: Some("Archivo Black".into()),
            color: [255, 214, 0, 255],
            outline: Some((8.0, [0, 0, 0, 255])),
            shadow: true,
            ..base("PART 2")
        },
    },
    TextTemplate {
        id: "quote",
        name: "Quote",
        category: "Title",
        text: "“Good ideas take time.”",
        intro: Some("fade"),
        style: || TextStyle {
            size: 72.0,
            font_family: Some("DM Serif Display".into()),
            color: [250, 245, 235, 255],
            shadow: true,
            line_spacing: 1.2,
            ..base("“Good ideas take time.”")
        },
    },
    TextTemplate {
        id: "handwritten",
        name: "Handwriting",
        category: "Title",
        text: "My day",
        intro: Some("tilt-in"),
        style: || TextStyle {
            size: 90.0,
            font_family: Some("Permanent Marker".into()),
            color: [255, 255, 255, 255],
            shadow: true,
            ..base("My day")
        },
    },
    TextTemplate {
        id: "script",
        name: "Script",
        category: "Title",
        text: "Summer",
        intro: Some("unfold"),
        style: || TextStyle {
            size: 100.0,
            font_family: Some("Pacifico".into()),
            color: [255, 110, 170, 255],
            outline: Some((4.0, [255, 255, 255, 255])),
            shadow: true,
            ..base("Summer")
        },
    },
    TextTemplate {
        id: "neon-sign",
        name: "Neon sign",
        category: "Title",
        text: "AT NIGHT",
        intro: Some("flicker"),
        style: || TextStyle {
            size: 110.0,
            font_family: Some("Righteous".into()),
            color: [90, 255, 240, 255],
            outline: Some((4.0, [0, 40, 70, 255])),
            shadow: true,
            ..base("AT NIGHT")
        },
    },
    TextTemplate {
        id: "credits-line",
        name: "Credits",
        category: "Title",
        text: "Edited by Alex Sample",
        intro: Some("fade"),
        style: || TextStyle {
            size: 54.0,
            font_family: Some("DM Serif Display".into()),
            color: [235, 235, 235, 255],
            shadow: false,
            ..base("Edited by Alex Sample")
        },
    },
    TextTemplate {
        id: "glitch-title",
        name: "Glitch",
        category: "Gaming",
        text: "SYSTEM ERROR",
        intro: Some("glitch-in"),
        style: || TextStyle {
            size: 90.0,
            font_family: Some("Russo One".into()),
            color: [255, 40, 90, 255],
            outline: Some((5.0, [0, 240, 255, 255])),
            shadow: true,
            ..base("SYSTEM ERROR")
        },
    },
    TextTemplate {
        id: "boss-fight",
        name: "Boss fight",
        category: "Gaming",
        text: "BOSSFIGHT",
        intro: Some("punch"),
        style: || TextStyle {
            size: 110.0,
            font_family: Some("Black Ops One".into()),
            color: [255, 60, 40, 255],
            outline: Some((7.0, [0, 0, 0, 255])),
            shadow: true,
            ..base("BOSSFIGHT")
        },
    },
    TextTemplate {
        id: "new-video",
        name: "New video",
        category: "Social",
        text: "NEW VIDEO",
        intro: Some("snap-left"),
        style: || TextStyle {
            size: 80.0,
            font_family: Some("Archivo Black".into()),
            color: [255, 255, 255, 255],
            background: Some([230, 30, 40, 255]),
            shadow: false,
            ..base("NEW VIDEO")
        },
    },
    TextTemplate {
        id: "did-you-know",
        name: "Did you know?",
        category: "Social",
        text: "Did you know?",
        intro: Some("bounce-in"),
        style: || TextStyle {
            size: 64.0,
            font_family: Some("Varela Round".into()),
            color: [20, 20, 24, 255],
            background: Some([255, 214, 0, 245]),
            shadow: false,
            ..base("Did you know?")
        },
    },
    TextTemplate {
        id: "follow-more",
        name: "Follow for more!",
        category: "Social",
        text: "Follow for more!",
        intro: Some("snap-up"),
        style: || TextStyle {
            size: 64.0,
            font_family: Some("Righteous".into()),
            color: [255, 255, 255, 255],
            outline: Some((5.0, [90, 0, 160, 255])),
            shadow: true,
            ..base("Follow for more!")
        },
    },
    TextTemplate {
        id: "big-number",
        name: "Big number",
        category: "Social",
        text: "1,000,000",
        intro: Some("elastic"),
        style: || TextStyle {
            size: 120.0,
            font_family: Some("Bungee".into()),
            color: [255, 255, 255, 255],
            outline: Some((6.0, [0, 0, 0, 255])),
            shadow: true,
            ..base("1,000,000")
        },
    },
    TextTemplate {
        id: "price-tag",
        name: "Price tag",
        category: "Social",
        text: "ONLY $9.99",
        intro: Some("stamp"),
        style: || TextStyle {
            size: 90.0,
            font_family: Some("Archivo Black".into()),
            color: [20, 20, 20, 255],
            background: Some([255, 214, 0, 255]),
            shadow: false,
            ..base("ONLY $9.99")
        },
    },
    TextTemplate {
        id: "name-tag",
        name: "Name tag",
        category: "Note",
        text: "Alex Sample\nCreator",
        intro: Some("snap-left"),
        style: || TextStyle {
            size: 46.0,
            font_family: Some("Varela Round".into()),
            align: -1,
            color: [20, 20, 24, 255],
            background: Some([255, 255, 255, 240]),
            shadow: false,
            line_spacing: 1.2,
            ..base("Alex Sample\nCreator")
        },
    },
    TextTemplate {
        id: "caution",
        name: "Caution",
        category: "Note",
        text: "CAUTION!",
        intro: Some("shake-in"),
        style: || TextStyle {
            size: 80.0,
            font_family: Some("Russo One".into()),
            color: [20, 20, 20, 255],
            background: Some([255, 200, 0, 255]),
            shadow: false,
            ..base("CAUTION!")
        },
    },
    TextTemplate {
        id: "place-time",
        name: "Place & time",
        category: "Note",
        text: "Hamburg · 07:30",
        intro: Some("wipe-fade"),
        style: || TextStyle {
            size: 46.0,
            font_family: Some("Space Mono".into()),
            color: [255, 255, 255, 255],
            background: Some([0, 0, 0, 170]),
            shadow: false,
            ..base("Hamburg · 07:30")
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

/// Text effects: change the look of an existing text.
pub struct TextEffect {
    pub id: &'static str,
    pub name: &'static str,
    pub apply: fn(&mut TextStyle),
}

pub static TEXT_EFFECTS: &[TextEffect] = &[
    TextEffect {
        id: "plain",
        name: "Plain",
        apply: |s| {
            s.outline = None;
            s.background = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "shadow",
        name: "Shadow",
        apply: |s| {
            s.outline = None;
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "outline-black",
        name: "Outline",
        apply: |s| {
            s.outline = Some((4.0, [0, 0, 0, 255]));
            s.background = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "outline-white",
        name: "Light outline",
        apply: |s| {
            s.outline = Some((4.0, [255, 255, 255, 255]));
            s.background = None;
            s.shadow = false;
            s.color = [20, 20, 20, 255];
        },
    },
    TextEffect {
        id: "yellow",
        name: "Yellow",
        apply: |s| {
            s.color = [255, 221, 0, 255];
            s.outline = Some((3.0, [0, 0, 0, 255]));
            s.background = None;
        },
    },
    TextEffect {
        id: "neon-pink",
        name: "Neon pink",
        apply: |s| {
            s.color = [255, 80, 220, 255];
            s.outline = Some((2.0, [255, 210, 250, 255]));
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "neon-blue",
        name: "Neon blue",
        apply: |s| {
            s.color = [70, 200, 255, 255];
            s.outline = Some((2.0, [210, 245, 255, 255]));
            s.background = None;
            s.shadow = true;
        },
    },
    TextEffect {
        id: "box-dark",
        name: "Dark box",
        apply: |s| {
            s.background = Some([0, 0, 0, 180]);
            s.outline = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "box-light",
        name: "Light box",
        apply: |s| {
            s.background = Some([255, 255, 255, 235]);
            s.outline = None;
            s.shadow = false;
            s.color = [20, 20, 20, 255];
        },
    },
    TextEffect {
        id: "box-red",
        name: "Red box",
        apply: |s| {
            s.background = Some([230, 40, 40, 255]);
            s.outline = None;
            s.shadow = false;
            s.color = [255, 255, 255, 255];
        },
    },
    TextEffect {
        id: "box-yellow",
        name: "Markers",
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
        // every effect keeps the text and gives its own look (two equal effects would be a copy mistake)
        let mut looks: Vec<(&str, TextStyle)> = Vec::new();
        for e in TEXT_EFFECTS {
            let mut s = TextStyle::default();
            (e.apply)(&mut s);
            assert_eq!(s.text, "Default text");
            if let Some((other, _)) = looks.iter().find(|(id, l)| *l == s || *id == e.id) {
                panic!("{} repeats {other}", e.id);
            }
            looks.push((e.id, s));
        }
    }
}
