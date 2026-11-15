//! Einstellungen → Inhalte: alle Pakete an einer Stelle (Grundausstattung und Asset-Pakete),
//! ein-/ausschalten, hinzufügen (ZIP), entfernen, Quellen und Lizenzen.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cl_fx::catalog::{self, Entry, Kind, Pack, PackInfo, Payload};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};
use crate::timeline::menu_item;
use crate::widgets;

/// Rückfrage bzw. Fenster der Seite „Inhalte“.
#[derive(Debug, Clone)]
pub enum ContentsDialog {
    /// Paket gleicher ID ist schon installiert: ersetzen?
    Replace { zip: PathBuf, id: String },
    /// Eigenes Paket löschen?
    Remove { dir: PathBuf, name: String },
    /// Danksagung (Markdown)
    Credits(String),
}

/// Ordner für eigene Pakete
pub fn user_root() -> PathBuf {
    paths::data_dir().join("Plugins")
}

fn localized_or_empty(t: &cl_plugin::LocalizedText, lang: &str) -> String {
    if t.is_empty() {
        String::new()
    } else {
        cl_plugin::localized(t, lang).to_string()
    }
}

/// Katalog-Pakete aus den geladenen Asset-Paketen (auch ausgeschaltete, mit Schalterstand).
/// IDs der Einträge: `<paket-id>/<asset-id>` (wie bisher bei Plugin-Effekten).
pub fn catalog_packs(reg: &cl_plugin::Registry, lang: &str) -> Vec<Pack> {
    use cl_plugin::AssetKind as A;
    reg.plugins
        .iter()
        .filter(|p| p.manifest.plugin.kind == cl_plugin::PluginKind::AssetPack)
        .map(|p| {
            let m = &p.manifest.plugin;
            let info = PackInfo {
                id: m.id.clone(),
                name: cl_plugin::localized(&m.name, lang).to_string(),
                version: m.version.clone(),
                description: localized_or_empty(&m.description, lang),
                license: m.license.clone(),
                author: m.authors.join(", "),
                source: m.source.clone(),
            };
            let entries = p
                .manifest
                .assets
                .iter()
                .filter_map(|a| {
                    let file = || a.file.as_ref().map(|f| Payload::File(p.dir.join(f)));
                    let chain = || {
                        a.ffmpeg_filled()
                            .filter(|c| cl_plugin::ffmpeg_chain_allowed(c))
                            .map(Payload::Ffmpeg)
                    };
                    let (kind, payload) = match a.kind {
                        A::VideoEffect => (Kind::Effect, chain()?),
                        A::Filter => (Kind::Filter, chain()?),
                        A::Sound => (Kind::Sound, file()?),
                        A::Sticker => (Kind::Sticker, file()?),
                        // shortcut: Shader-Assets und weitere Arten erscheinen erst mit ihrer Render-Stufe
                        _ => return None,
                    };
                    Some(Entry {
                        id: format!("{}/{}", m.id, a.id),
                        kind,
                        name: cl_plugin::localized(&a.name, lang).to_string(),
                        // ohne Kategorie: eigener Ordner mit dem Paketnamen
                        categories: if a.category.is_empty() {
                            vec![info.name.clone()]
                        } else {
                            a.category.clone()
                        },
                        pack: m.id.clone(),
                        license: a.license.clone().unwrap_or_else(|| info.license.clone()),
                        author: a.author.clone().unwrap_or_else(|| info.author.clone()),
                        source: a.source.clone().or_else(|| info.source.clone()),
                        payload,
                    })
                })
                .collect();
            Pack {
                info,
                enabled: p.enabled,
                entries,
            }
        })
        .collect()
}

/// IDs der Inhaltspakete (`packs`) bzw. aller übrigen Plugins (Integrationen …).
pub fn plugin_ids(reg: &cl_plugin::Registry, packs: bool) -> Vec<String> {
    reg.plugins
        .iter()
        .filter(|p| (p.manifest.plugin.kind == cl_plugin::PluginKind::AssetPack) == packs)
        .map(|p| p.manifest.plugin.id.clone())
        .collect()
}

/// „12 Effekte · 3 Sounds“
pub fn counts_text(counts: &BTreeMap<Kind, usize>) -> String {
    counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| {
            crate::tr(match k {
                Kind::Effect => format!("{n} Effekte"),
                Kind::Filter => format!("{n} Filter"),
                Kind::Transition => format!("{n} Übergänge"),
                Kind::Sound => format!("{n} Sounds"),
                Kind::Sticker => format!("{n} Sticker"),
            })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

impl App {
    /// Katalog neu anmelden (nach Ein-/Ausschalten, Sprachwechsel, neuen Paketen).
    pub fn refresh_catalog(&mut self) {
        catalog::load_packs(catalog_packs(&self.plugins, self.i18n.lang().code()));
        self.invalidate_preview();
    }

    /// Plugin-Ordner neu einlesen (nach Hinzufügen oder Entfernen).
    fn reload_plugins(&mut self) {
        let dirs = paths::plugin_dirs();
        self.plugins = cl_plugin::Registry::scan(dirs.iter().map(|p| p.as_path()));
        self.plugins.apply_states(&self.settings.plugin_states);
        self.refresh_catalog();
    }

    fn add_pack(&mut self, zip: &Path, replace: bool) {
        let root = user_root();
        match cl_plugin::install_zip(zip, &root, replace) {
            Ok(cl_plugin::Installed::Exists(id)) => {
                self.contents_dialog = Some(ContentsDialog::Replace {
                    zip: zip.to_path_buf(),
                    id,
                });
            }
            Ok(cl_plugin::Installed::Done(id)) => {
                // ein mitgeliefertes Paket gleicher ID gewinnt beim Laden: dann nicht behalten
                let bundled = self
                    .plugins
                    .plugins
                    .iter()
                    .any(|p| p.manifest.plugin.id == id && !p.dir.starts_with(&root));
                if bundled {
                    let _ = cl_plugin::remove_pack(&root, &root.join(&id));
                    self.settings_error = Some(crate::tr(format!("„{id}“ ist schon mitgeliefert")));
                    return;
                }
                self.reload_plugins();
                self.toast(format!("{}  Paket „{id}“ hinzugefügt", icon::CHECK));
            }
            Err(e) => self.settings_error = Some(e),
        }
    }

    pub fn contents_settings(&mut self, ui: &mut egui::Ui) {
        let root = user_root();
        let mut toggled = None;
        let mut remove = None;
        for s in catalog::packs() {
            let standard = s.info.id == catalog::STANDARD;
            let own = self
                .plugins
                .plugins
                .iter()
                .find(|p| p.manifest.plugin.id == s.info.id)
                .filter(|p| p.dir.starts_with(&root))
                .map(|p| p.dir.clone());
            let mut on = s.enabled;
            egui::Frame::default()
                .fill(theme::BG_RAISED())
                .corner_radius(theme::R_CARD)
                .inner_margin(theme::PAD_CARD_L)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if !on {
                        ui.set_opacity(0.45);
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::tr(&s.info.name)).strong());
                        ui.label(
                            RichText::new(format!("v{} · {}", s.info.version, s.info.license))
                                .small()
                                .color(theme::TEXT_DIM()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.set_opacity(1.0);
                            if !standard && widgets::toggle(ui, &mut on) {
                                toggled = Some((s.info.id.clone(), on));
                            }
                            if let Some(dir) = &own {
                                ui.menu_button(icon::DOTS_THREE, |ui| {
                                    if menu_item(ui, icon::TRASH, "Entfernen", "") {
                                        remove = Some((dir.clone(), s.info.name.clone()));
                                        ui.close_menu();
                                    }
                                });
                            }
                        });
                    });
                    if !s.info.description.is_empty() {
                        ui.label(RichText::new(crate::tr(&s.info.description)).small().color(theme::TEXT_DIM()));
                    }
                    ui.label(RichText::new(counts_text(&s.counts)).small().color(theme::TEXT_DIM()));
                });
            ui.add_space(6.0);
        }
        if let Some((id, on)) = toggled {
            self.set_plugin_enabled(&id, on);
        }
        if let Some((dir, name)) = remove {
            self.contents_dialog = Some(ContentsDialog::Remove { dir, name });
        }
        for e in &self.plugins.errors {
            ui.label(RichText::new(format!("{} {e}", icon::WARNING)).small().color(theme::WARN_TEXT()));
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::secondary(format!("{}  Paket hinzufügen …", icon::PLUS))).clicked() {
                if let Some(f) = crate::dialog().add_filter("ZIP", &["zip"]).pick_file() {
                    self.add_pack(&f, false);
                }
            }
            if ui.add(theme::ghost(format!("{}  Paket-Ordner öffnen", icon::FOLDER_OPEN))).clicked() {
                let _ = std::fs::create_dir_all(&root);
                crate::home::reveal(&root);
            }
            if ui.add(theme::ghost(format!("{}  Quellen und Lizenzen", icon::INFO))).clicked() {
                let packs = catalog_packs(&self.plugins, self.i18n.lang().code());
                self.contents_dialog = Some(ContentsDialog::Credits(catalog::credits(&packs, &|s| crate::tr(s))));
            }
        });
    }

    /// Rückfragen und Danksagung über den Einstellungen.
    pub fn contents_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.contents_dialog.clone() else {
            return;
        };
        let (mut ok, mut close) = (false, false);
        let modal = crate::dialog::modal(ctx, "contents-dialog").show(ctx, |ui| match &d {
            ContentsDialog::Replace { id, .. } => {
                ui.set_width(340.0);
                close |= crate::dialog::title(ui, "Paket ersetzen?");
                ui.label(crate::tr(format!("„{id}“ ist schon installiert. Durch das neue Paket ersetzen?")));
                let (m, c) = crate::dialog::buttons(ui, theme::primary("Ersetzen"), true);
                ok |= m;
                close |= c;
            }
            ContentsDialog::Remove { name, .. } => {
                ui.set_width(340.0);
                close |= crate::dialog::title(ui, "Paket entfernen?");
                ui.label(crate::tr(format!(
                    "„{name}“ wird gelöscht. Projekte, die seine Inhalte nutzen, melden sie danach als fehlend."
                )));
                let (m, c) = crate::dialog::buttons(ui, theme::danger("Entfernen"), true);
                ok |= m;
                close |= c;
            }
            ContentsDialog::Credits(text) => {
                ui.set_width(520.0);
                close |= crate::dialog::title(ui, "Quellen und Lizenzen");
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(RichText::new(text).monospace().size(theme::FS_HINT));
                });
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary(format!("{}  Kopieren", icon::COPY))).clicked() {
                        ui.ctx().copy_text(text.clone());
                        ok = true;
                    }
                });
            }
        });
        if modal.should_close() {
            close = true;
        }
        if ok || close {
            self.contents_dialog = None;
        }
        if ok {
            match d {
                ContentsDialog::Replace { zip, .. } => self.add_pack(&zip, true),
                ContentsDialog::Remove { dir, .. } => match cl_plugin::remove_pack(&user_root(), &dir) {
                    Ok(()) => {
                        self.reload_plugins();
                        self.toast(format!("{}  Paket entfernt", icon::CHECK));
                    }
                    Err(e) => self.settings_error = Some(e),
                },
                ContentsDialog::Credits(_) => self.toast(format!("{}  In die Zwischenablage kopiert", icon::CHECK)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
    }

    #[test]
    fn shipped_pack_effects_land_in_builtin_folders() {
        let reg = cl_plugin::Registry::scan([bundled().as_path()]);
        let packs = catalog_packs(&reg, "de");
        assert_eq!(plugin_ids(&reg, true), vec!["cutline.basis".to_string()]);
        assert!(plugin_ids(&reg, false).contains(&"cutline.epidemic-sound".to_string()));
        let basis = packs.iter().find(|p| p.info.id == "cutline.basis").unwrap();
        assert!(!basis.entries.is_empty());
        for e in &basis.entries {
            let builtin: &[&str] = match e.kind {
                Kind::Effect => &cl_fx::EFFECT_CATEGORIES,
                Kind::Filter => &cl_fx::FILTER_CATEGORIES,
                _ => &[],
            };
            assert!(builtin.contains(&e.category()), "{} in {}", e.id, e.category());
        }
    }

    /// Danksagung im Repo ist aktuell; jedes mitgelieferte Paket nennt eine Lizenz.
    /// Neu erzeugen: `CUTLINE_WRITE_THIRD_PARTY=1 cargo test -p cl-app third_party`
    #[test]
    fn third_party_file_is_current() {
        let reg = cl_plugin::Registry::scan([bundled().as_path()]);
        let packs = catalog_packs(&reg, "de");
        for p in &packs {
            assert!(!p.info.license.trim().is_empty(), "{} ohne Lizenz", p.info.id);
            assert!(p.entries.iter().all(|e| !e.license.trim().is_empty()), "{}", p.info.id);
        }
        let want = catalog::credits(&packs, &|s| s.to_string());
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../THIRD_PARTY.md");
        if std::env::var_os("CUTLINE_WRITE_THIRD_PARTY").is_some() {
            std::fs::write(&file, &want).unwrap();
        }
        let have = std::fs::read_to_string(&file).unwrap_or_default().replace("\r\n", "\n");
        assert_eq!(have, want, "THIRD_PARTY.md veraltet (siehe Kommentar am Test)");
    }

    /// Kleines Testpaket mit Sound (WAV) und Sticker (SVG): gelistet, ausgeschaltet weg,
    /// Pfad-Ausbruch abgelehnt.
    #[test]
    fn pack_sounds_and_stickers_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(pack.join("ding.wav"), spec).unwrap();
        for i in 0..800 {
            w.write_sample(((i as f32 * 0.3).sin() * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(
            pack.join("star.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>"#,
        )
        .unwrap();
        let manifest = r#"
[plugin]
id = "test.pack"
version = "1.0.0"
api = 1
kind = "asset-pack"
name = { de = "Testpaket", en = "Test pack" }
license = "CC0-1.0"
authors = ["Jemand"]

[[asset]]
id = "ding"
kind = "sound"
name = { de = "Ding" }
category = ["Oberfläche"]
file = "ding.wav"

[[asset]]
id = "star"
kind = "sticker"
name = { de = "Stern" }
file = "star.svg"
license = "CC-BY-4.0"
"#;
        std::fs::write(pack.join("plugin.toml"), manifest).unwrap();
        let mut reg = cl_plugin::Registry::scan([dir.path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let packs = catalog_packs(&reg, "en");
        let snd = catalog::list_in(&packs, Kind::Sound);
        let e = snd.last().unwrap();
        assert_eq!(e.id, "test.pack/ding");
        assert_eq!(e.category(), "Oberfläche");
        assert_eq!(e.payload, Payload::File(pack.join("ding.wav")));
        let st = catalog::get_in(&packs, Kind::Sticker, "test.pack/star").unwrap();
        assert_eq!((st.license.as_str(), st.author.as_str()), ("CC-BY-4.0", "Jemand"));
        assert_eq!(st.category(), "Test pack", "ohne Kategorie: Ordner mit dem Paketnamen");
        assert_eq!(counts_text(&catalog::packs_in(&packs)[1].counts), "1 Sounds · 1 Sticker");
        // ausgeschaltet: verschwunden
        reg.apply_states(&[("test.pack".to_string(), false)].into_iter().collect());
        let packs = catalog_packs(&reg, "en");
        assert_eq!(catalog::list_in(&packs, Kind::Sound).len(), cl_fx::sfx::SFX.len());
        assert!(catalog::get_in(&packs, Kind::Sticker, "test.pack/star").is_none());
        // Pfad-Ausbruch: Paket wird nicht geladen
        std::fs::write(dir.path().join("x.wav"), b"RIFF").unwrap();
        std::fs::write(pack.join("plugin.toml"), manifest.replace("file = \"ding.wav\"", "file = \"../x.wav\"")).unwrap();
        let reg = cl_plugin::Registry::scan([dir.path()]);
        assert!(reg.plugins.is_empty());
        assert!(reg.errors[0].contains("innerhalb"), "{:?}", reg.errors);
    }

    /// Die Endungen der Pakete sind eine Teilmenge dessen, was die eigene Bibliothek kann.
    #[test]
    fn pack_file_types_match_library() {
        for e in cl_plugin::SOUND_EXT {
            assert!(crate::library::SOUND_EXT.contains(e), "{e}");
        }
        for e in cl_plugin::STICKER_EXT {
            assert!(crate::library::STICKER_EXT.contains(e), "{e}");
        }
    }
}
