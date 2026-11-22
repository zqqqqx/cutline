//! Brand Kit: colors, fonts and logo per user in `<data>/brand.json`; the logo is copied to
//! `<data>/Brand/` (projects point to the copy, it is never deleted). The kit only takes effect
//! when choosing and inserting, never on existing clips.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_core::*;
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};

pub const MAX_COLORS: usize = 8;
/// Font roles in the order of `BrandKit::fonts`
pub const ROLES: [&str; 3] = ["Titel", "Text", "Untertitel"];
pub const TEXT: usize = 1;
pub const CAPTION: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrandColor {
    #[serde(default)]
    pub name: String,
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrandKit {
    pub colors: Vec<BrandColor>,
    /// Font family per role (`ROLES`), always as many entries as roles
    pub fonts: Vec<Option<String>>,
    pub logo: Option<PathBuf>,
}

impl Default for BrandKit {
    fn default() -> Self {
        Self { colors: Vec::new(), fonts: vec![None; ROLES.len()], logo: None }
    }
}

impl BrandKit {
    /// Content of `brand.json`; broken or missing → empty kit, limits enforced,
    /// vanished logo → no logo.
    pub fn parse(text: &str) -> Self {
        let mut k: Self = serde_json::from_str(text).unwrap_or_default();
        k.colors.truncate(MAX_COLORS);
        k.fonts.resize(ROLES.len(), None);
        k.logo = k.logo.filter(|p| p.is_file());
        k
    }
    fn path() -> PathBuf {
        crate::paths::data_dir().join("brand.json")
    }
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(Self::path()).unwrap_or_default())
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::paths::data_dir());
        let _ = crate::paths::write_atomic(&Self::path(), serde_json::to_string_pretty(self).unwrap_or_default());
    }
    /// Font of a role (`TEXT`, `CAPTION` …)
    pub fn font(&self, role: usize) -> Option<&String> {
        self.fonts.get(role).and_then(Option::as_ref)
    }
    /// Set fonts without duplicates, in role order
    pub fn font_list(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for f in self.fonts.iter().flatten() {
            if !v.contains(f) {
                v.push(f.clone());
            }
        }
        v
    }
}

/// Copy the logo into the data folder (same file types as custom stickers).
pub fn copy_logo(src: &Path) -> std::result::Result<PathBuf, String> {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !crate::library::STICKER_EXT.contains(&ext.as_str()) {
        return Err(crate::tr("Dateityp wird nicht unterstützt"));
    }
    let dir = crate::paths::data_dir().join("Brand");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dest = crate::library::unique(&dir, name);
    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Position of the logo: top right, 12 % of the project width wide (corner like picture-in-picture).
pub fn logo_transform(pw: f32, ph: f32, iw: f32, ih: f32) -> Transform {
    crate::arrange::in_corner(pw, ph, (iw, ih), 0.12, 1)
}

fn colors_id() -> egui::Id {
    egui::Id::new("brand-colors")
}

/// Provide the kit's colors to the shared color picker (`widgets::color_button`).
pub fn publish(ctx: &egui::Context, kit: &BrandKit) {
    ctx.data_mut(|d| d.insert_temp(colors_id(), Arc::new(kit.colors.clone())));
}

/// Colors the color picker shows as a row (empty: no row).
pub fn colors(ctx: &egui::Context) -> Arc<Vec<BrandColor>> {
    ctx.data(|d| d.get_temp(colors_id())).unwrap_or_default()
}

impl App {
    /// „Brand Kit“ section in the settings window
    pub fn brand_settings(&mut self, ui: &mut egui::Ui) {
        let mut kit = self.brand.clone();
        theme::section(ui, "Farben");
        let mut remove = None;
        for (i, c) in kit.colors.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut c.rgb);
                ui.add(egui::TextEdit::singleline(&mut c.name).hint_text(crate::tr("Name")).desired_width(220.0));
                if theme::icon_button(ui, icon::X, "Entfernen", true, false).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            kit.colors.remove(i);
        }
        if kit.colors.len() < MAX_COLORS && ui.add(theme::secondary(format!("{}  Farbe hinzufügen", icon::PLUS))).clicked() {
            kit.colors.push(BrandColor { name: String::new(), rgb: [255, 255, 255] });
        }
        theme::section(ui, "Schriften");
        for (i, role) in ROLES.iter().enumerate() {
            theme::row(ui, role, |ui| {
                if let Some(f) = self.font_picker(ui, ("brand-font", i), kit.fonts[i].as_deref(), &crate::tr("Keine"), &[], 260.0) {
                    kit.fonts[i] = f;
                }
            });
        }
        theme::section(ui, "Logo");
        ui.horizontal(|ui| {
            if let Some(p) = &kit.logo {
                ui.label(RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).small());
            }
            if ui.add(theme::secondary(format!("{}  Logo wählen …", icon::IMAGE))).clicked() {
                if let Some(f) = crate::dialog().add_filter(crate::tr("Bilder"), crate::library::STICKER_EXT).pick_file() {
                    match copy_logo(&f) {
                        Ok(p) => kit.logo = Some(p),
                        Err(e) => self.toast_err(e),
                    }
                }
            }
            if kit.logo.is_some() && theme::icon_button(ui, icon::X, "Logo entfernen", true, false).clicked() {
                kit.logo = None;
            }
        });
        let settled = !ui.ctx().is_using_pointer() && !ui.ctx().wants_keyboard_input();
        self.brand_edited(ui.ctx(), kit, settled);
    }

    /// Take over the edited kit: the color picker follows at once, it is only saved when the
    /// change is finished (`settled`: nothing is being dragged, no text field active) – not
    /// every frame while dragging. Closing the settings and quitting write the rest.
    fn brand_edited(&mut self, ctx: &egui::Context, kit: BrandKit, settled: bool) {
        if kit != self.brand {
            self.brand = kit;
            publish(ctx, &self.brand);
            self.brand_unsaved = true;
        }
        if settled {
            self.brand_flush();
        }
    }

    /// Write a not yet saved change to the kit.
    pub fn brand_flush(&mut self) {
        if std::mem::take(&mut self.brand_unsaved) {
            self.brand.save();
        }
    }

    /// Take over the kit, save it and give it to the color picker.
    pub fn set_brand(&mut self, ctx: &egui::Context, kit: BrandKit) {
        self.brand = kit;
        self.brand.save();
        self.brand_unsaved = false;
        publish(ctx, &self.brand);
    }

    /// Logo as an image clip above the video, over the whole project length, top right (one
    /// history step). Without a logo nothing.
    pub fn insert_logo(&mut self) -> Option<ClipId> {
        let path = self.brand.logo.clone().filter(|p| p.is_file())?;
        let file = crate::library_ui::own_sticker_file(&path)?;
        let len = self.project.duration();
        let (pw, ph) = (self.project.settings.width as f32, self.project.settings.height as f32);
        self.one_step(|s| {
            s.add_image_overlay_step(file, Time::ZERO, None, |s, c| {
                if len > Time::ZERO {
                    c.duration = len;
                }
                if let ClipContent::Media { media, .. } = c.content {
                    if let Some(m) = s.project.media(media) {
                        c.transform = logo_transform(pw, ph, m.width as f32, m.height as f32);
                    }
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dragging in the color picker: the color row follows at once, `brand.json` is only written when
    /// the change is finished – or on closing or quitting.
    #[test]
    fn brand_saves_when_the_change_is_done() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let mut kit = BrandKit::default();
        for v in [10, 20, 30] {
            kit.colors = vec![BrandColor { name: String::new(), rgb: [v, 0, 0] }];
            app.brand_edited(&ctx, kit.clone(), false);
            assert_eq!(colors(&ctx)[0].rgb, [v, 0, 0]);
            assert_eq!(BrandKit::load(), BrandKit::default(), "saved while dragging");
        }
        app.brand_edited(&ctx, kit.clone(), true);
        assert_eq!(BrandKit::load(), kit);
        // still in the field, then settings closed or program quit
        kit.colors[0].name = "Rot".into();
        app.brand_edited(&ctx, kit.clone(), false);
        assert_ne!(BrandKit::load(), kit);
        app.brand_flush();
        assert_eq!(BrandKit::load(), kit);
    }

    #[test]
    fn brand_json_round_trip_limits_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let logo = dir.path().join("logo.png");
        std::fs::write(&logo, b"x").unwrap();
        let kit = BrandKit {
            colors: vec![BrandColor { name: "Rot".into(), rgb: [200, 0, 0] }, BrandColor { name: String::new(), rgb: [1, 2, 3] }],
            fonts: vec![None, Some("Poppins".into()), Some("Anton".into())],
            logo: Some(logo.clone()),
        };
        assert_eq!(BrandKit::parse(&serde_json::to_string(&kit).unwrap()), kit);
        // broken or empty: empty kit with three empty font roles
        for bad in ["", "{", "[1]", r#"{"colors": "rot"}"#] {
            let k = BrandKit::parse(bad);
            assert!(k.colors.is_empty() && k.logo.is_none(), "{bad}");
            assert_eq!(k.fonts, vec![None; 3], "{bad}");
        }
        // Limits: at most 8 colors and 3 fonts
        let many = serde_json::json!({
            "colors": (0..12).map(|i| serde_json::json!({"rgb": [i, i, i]})).collect::<Vec<_>>(),
            "fonts": ["A", "B", "C", "D", "E"],
        });
        let k = BrandKit::parse(&many.to_string());
        assert_eq!(k.colors.len(), MAX_COLORS);
        assert_eq!(k.fonts.len(), 3);
        assert_eq!(k.font(TEXT).map(String::as_str), Some("B"));
        // Logo file gone: no logo, the rest stays
        std::fs::remove_file(&logo).unwrap();
        let k = BrandKit::parse(&serde_json::to_string(&kit).unwrap());
        assert!(k.logo.is_none());
        assert_eq!(k.colors, kit.colors);
    }

    #[test]
    fn font_list_skips_empty_roles_and_duplicates() {
        let k = BrandKit { fonts: vec![Some("A".into()), None, Some("A".into())], ..Default::default() };
        assert_eq!(k.font_list(), ["A"]);
        assert!(BrandKit::parse("").font_list().is_empty());
    }

    /// Width 12 % of the project width, margin 4 % of the shorter side at the top and right
    #[test]
    fn logo_sits_top_right_in_every_format() {
        for (pw, ph) in [(1920.0, 1080.0), (1080.0, 1920.0), (1080.0, 1080.0)] {
            for (iw, ih) in [(512.0, 512.0), (800.0, 200.0), (300.0, 900.0)] {
                let t = logo_transform(pw, ph, iw, ih);
                let fit = f32::min(pw / iw, ph / ih);
                let (w, h) = (iw * fit * t.scale, ih * fit * t.scale);
                let m = 0.04 * f32::min(pw, ph);
                let right = pw / 2.0 + t.x + w / 2.0;
                let top = ph / 2.0 + t.y - h / 2.0;
                assert!((w - 0.12 * pw).abs() < 0.01, "{pw}x{ph} {iw}x{ih}: width {w}");
                assert!((right - (pw - m)).abs() < 0.01 && (top - m).abs() < 0.01, "{pw}x{ph} {iw}x{ih}");
                assert_eq!(t.rotation_deg, 0.0);
            }
        }
    }

    /// Width of the row with the shared color picker (without a window, own egui context)
    fn picker_width(ctx: &egui::Context) -> f32 {
        let mut w = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                w = ui
                    .horizontal(|ui| {
                        crate::widgets::color_button(ui, &mut [0, 0, 0, 255]);
                        ui.min_rect().width()
                    })
                    .inner;
            });
        });
        w
    }

    /// The row of brand colors only shows when the kit has colors
    #[test]
    fn color_picker_shows_brand_row_only_with_colors() {
        let ctx = egui::Context::default();
        let plain = picker_width(&ctx);
        publish(&ctx, &BrandKit::default());
        assert_eq!(picker_width(&ctx), plain, "empty kit: as before");
        let mut kit = BrandKit::default();
        kit.colors = vec![BrandColor { name: "A".into(), rgb: [1, 2, 3] }; 3];
        publish(&ctx, &kit);
        assert_eq!(colors(&ctx).len(), 3);
        assert!(picker_width(&ctx) > plain + 3.0 * 14.0, "three fields next to it");
    }

    fn png(path: &Path, w: u32, h: u32) {
        image::save_buffer(path, &vec![200u8; (w * h * 4) as usize], w, h, image::ExtendedColorType::Rgba8).unwrap();
    }

    /// „Logo einfügen“: new track above the video, whole project length, top right; one
    /// undo step, the rest of the project stays untouched. Without a logo nothing happens.
    #[test]
    fn insert_logo_is_one_undo_step_and_touches_nothing_else() {
        if !cl_media::test_tools() {
            return;
        }
        let mut app = crate::saving::tests::test_app();
        let mut p = Project::new("Logo");
        let video = MediaRef {
            id: MediaId::new(),
            path: app.dir().join("a.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
            has_alpha: false,
        };
        let mut main = Track::new(TrackKind::Video);
        main.main = true;
        main.clips.push(Clip::new(ClipContent::Media { media: video.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0)));
        p.timeline.tracks.push(main);
        p.media.push(video);
        app.set_doc(p);
        let before = app.project.clone();
        assert!(!app.action_enabled("brand_logo"));
        assert!(app.insert_logo().is_none());
        assert_eq!(app.project, before, "unchanged without a logo");

        let src = app.dir().join("marke.png");
        png(&src, 64, 32);
        let logo = copy_logo(&src).unwrap();
        assert!(logo.starts_with(crate::paths::data_dir().join("Brand")) && logo.is_file());
        assert!(copy_logo(&app.dir().join("x.txt")).is_err());
        app.brand.logo = Some(logo.clone());
        assert!(app.action_enabled("brand_logo"));
        let id = app.insert_logo().expect("logo clip");
        let (t, c) = app.project.find_clip(id).unwrap();
        assert!(t.kind == TrackKind::Video && !t.main);
        assert_eq!((c.start, c.duration), (Time::ZERO, Time::from_secs_f64(10.0)));
        assert_eq!(c.transform, logo_transform(1920.0, 1080.0, 64.0, 32.0));
        assert_eq!(app.project.timeline.tracks[0], before.timeline.tracks[0], "main track untouched");
        assert_eq!(app.project.timeline.tracks.len(), 2);
        assert!(app.project.media.iter().any(|m| m.path == logo));

        app.undo();
        app.sync_project();
        assert_eq!(app.project.timeline.tracks, before.timeline.tracks);
        assert_eq!(app.project.media, before.media);
    }

    /// Default text and „Klassisch“ captions get the brand font; templates and other
    /// caption styles keep their own, without a kit everything stays as before.
    #[test]
    fn new_text_and_classic_captions_use_brand_fonts() {
        let mut app = crate::saving::tests::test_app();
        app.set_doc(Project::new("Schrift"));
        let font = |app: &App, id: ClipId| match &app.project.find_clip(id).unwrap().1.content {
            ClipContent::Text(s) => s.font_family.clone(),
            _ => panic!("no text"),
        };
        let cap = |app: &App| match app.caption_clip("Hallo", Time::ZERO, Time::from_secs_f64(1.0), Vec::new()).content {
            ClipContent::Text(s) => s.font_family,
            _ => panic!("no text"),
        };
        let tpl = cl_fx::text::TEMPLATES.iter().find(|t| t.make().font_family.is_some()).unwrap();
        let other = crate::captions::PRESETS.iter().find(|p| p.id != "classic" && (p.style)().font_family.is_some()).unwrap();
        for brand in [false, true] {
            if brand {
                app.brand.fonts = vec![Some("Titelschrift".into()), Some("Textschrift".into()), Some("Untertitelschrift".into())];
            }
            let want = |s: &str| brand.then(|| s.to_string());
            let id = app.add_text_at(Time::ZERO, None, None).unwrap();
            assert_eq!(font(&app, id), want("Textschrift"), "default text, kit {brand}");
            let id = app.add_text_at(Time::ZERO, Some(tpl.id), None).unwrap();
            assert_eq!(font(&app, id), tpl.make().font_family, "template, kit {brand}");
            app.settings.caption_preset = "classic".into();
            assert_eq!(cap(&app), want("Untertitelschrift"), "Klassisch, kit {brand}");
            app.settings.caption_preset = other.id.into();
            assert_eq!(cap(&app), (other.style)().font_family, "{}, kit {brand}", other.id);
        }
    }
}
