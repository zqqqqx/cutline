//! Startseite: alle Projekte als Kacheln oder Liste, neu anlegen, öffnen, umbenennen,
//! duplizieren, im Explorer zeigen und löschen (in den Cutline-Papierkorb).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cl_core::{Project, Time};
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};

pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub duration: Time,
    pub clips: usize,
    pub size: (u32, u32),
    pub modified: SystemTime,
}

#[derive(Default)]
pub struct HomeState {
    pub entries: Vec<Entry>,
    scanned: bool,
    pub search: String,
    pub list_view: bool,
    rename: Option<(PathBuf, String)>,
    confirm_delete: Option<PathBuf>,
    thumbs: HashMap<PathBuf, Option<egui::TextureHandle>>,
    dirty: HashSet<PathBuf>,
}

pub const THUMB_W: u32 = 384;
pub const THUMB_H: u32 = 216;

/// Titelbild eines Projekts (rohe RGBA-Daten 384×216 im Datenordner).
pub fn thumb_path(project: &Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    project.hash(&mut h);
    paths::data_dir()
        .join("thumbs")
        .join(format!("{:016x}.rgba", h.finish()))
}

/// Datei/Ordner im Explorer zeigen (Datei wird markiert).
///
/// Der Explorer versteht `/select,"C:\pfad mit leerzeichen\datei"` nur in genau dieser Form:
/// mit Backslashes und Anführungszeichen nur um den Pfad. Die übliche Argument-Quotierung
/// (`"/select,C:\…"`) führt dazu, dass er stattdessen „Dokumente“ öffnet.
pub fn reveal(p: &Path) {
    let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // fehlende Datei: nächsten vorhandenen Ordner zeigen
    let target = if p.exists() {
        p.clone()
    } else {
        p.ancestors()
            .find(|a| a.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or(p.clone())
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let win = target.display().to_string().replace('/', "\\");
        let arg = if target.is_file() {
            format!("/select,\"{win}\"")
        } else {
            format!("\"{win}\"")
        };
        let _ = std::process::Command::new("explorer").raw_arg(arg).spawn();
    }
    #[cfg(not(windows))]
    {
        let dir = if target.is_file() {
            target.parent().unwrap_or(&target).to_path_buf()
        } else {
            target
        };
        let _ = std::process::Command::new(if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        })
        .arg(dir)
        .spawn();
    }
}

/// Freier Name „Projekt N“.
pub fn next_project_name() -> String {
    let dir = paths::projects_dir();
    (1..)
        .map(|n| format!("Projekt {n}"))
        .find(|n| !dir.join(format!("{n}.cutline")).exists())
        .unwrap_or_else(|| "Projekt".into())
}

pub fn ago(t: SystemTime) -> String {
    let s = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match s {
        0..=59 => crate::tr("gerade eben"),
        60..=3599 => crate::tr(format!("vor {} Min.", s / 60)),
        3600..=86399 => crate::tr(format!("vor {} Std.", s / 3600)),
        86400..=172799 => crate::tr("gestern"),
        _ => crate::tr(format!("vor {} Tagen", s / 86400)),
    }
}

impl HomeState {
    pub fn refresh(&mut self) {
        self.scanned = false;
        self.dirty.extend(self.thumbs.keys().cloned());
    }

    pub fn thumb_ready(&mut self, out: &Path) {
        if let Some(p) = self.thumbs.keys().find(|p| thumb_path(p) == out).cloned() {
            self.dirty.insert(p);
        }
    }

    fn scan(&mut self, recent: &[PathBuf]) {
        let mut paths_: Vec<PathBuf> = std::fs::read_dir(paths::projects_dir())
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
                    .collect()
            })
            .unwrap_or_default();
        for r in recent {
            if r.is_file() && !paths_.contains(r) {
                paths_.push(r.clone());
            }
        }
        self.entries = paths_
            .into_iter()
            .filter_map(|p| {
                let modified = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let proj = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| Project::from_json(&s).ok())?;
                Some(Entry {
                    name: proj.name.clone(),
                    duration: proj.duration(),
                    clips: proj.timeline.tracks.iter().map(|t| t.clips.len()).sum(),
                    size: (proj.settings.width, proj.settings.height),
                    modified,
                    path: p,
                })
            })
            .collect();
        self.entries.sort_by(|a, b| b.modified.cmp(&a.modified));
        self.scanned = true;
    }

    fn thumb(&mut self, ctx: &egui::Context, p: &Path) -> Option<egui::TextureId> {
        let need = !self.thumbs.contains_key(p) || self.dirty.remove(p);
        if need {
            let tex = std::fs::read(thumb_path(p))
                .ok()
                .filter(|d| d.len() == (THUMB_W * THUMB_H * 4) as usize)
                .map(|d| {
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [THUMB_W as usize, THUMB_H as usize],
                        &d,
                    );
                    ctx.load_texture(
                        format!("home-{}", p.display()),
                        img,
                        egui::TextureOptions::LINEAR,
                    )
                });
            self.thumbs.insert(p.to_path_buf(), tex);
        }
        self.thumbs.get(p).and_then(|t| t.as_ref().map(|t| t.id()))
    }
}

impl App {
    pub fn home_page(&mut self, ui: &mut egui::Ui) {
        if !self.home.scanned {
            let recent = self.settings.recent.clone();
            self.home.scan(&recent);
        }
        let full = ui.max_rect();
        let side = Rect::from_min_max(
            full.min + vec2(4.0, 4.0),
            pos2(full.left() + 220.0, full.bottom() - 4.0),
        );
        let main = Rect::from_min_max(
            pos2(side.right() + 4.0, full.top() + 4.0),
            full.max - vec2(4.0, 4.0),
        );
        ui.painter()
            .rect_filled(side, theme::RADIUS, theme::BG_CARD());
        ui.painter()
            .rect_filled(main, theme::RADIUS, theme::BG_CARD());

        // Seitenleiste
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(side.shrink(16.0)), |ui| {
            ui.horizontal(|ui| theme::wordmark(ui, 30.0));
            ui.add_space(22.0);
            if ui
                .add_sized(
                    [ui.available_width(), 34.0],
                    theme::primary(format!("{}  Neues Projekt", icon::PLUS)),
                )
                .clicked()
            {
                self.new_project();
            }
            ui.add_space(6.0);
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    theme::secondary(format!("{}  Projekt öffnen …", icon::FOLDER_OPEN)),
                )
                .clicked()
            {
                self.open_dialog();
            }
            self.home_templates(ui);
            if self.import_enabled() {
                ui.add_space(6.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        theme::ghost(format!("{}  Projekt importieren …", icon::DOWNLOAD_SIMPLE)),
                    )
                    .on_hover_text(crate::tr("Aus CapCut, Final Cut Pro oder DaVinci Resolve"))
                    .clicked()
                {
                    self.import.open = true;
                }
            }
            ui.add_space(18.0);
            let nav = |ui: &mut egui::Ui, g: &str, t: &str, active: bool| {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                if active {
                    ui.painter().rect_filled(r, theme::R_CARD, theme::BG_RAISED());
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(r, theme::R_CARD, theme::overlay(10));
                }
                let col = if active {
                    theme::ACCENT()
                } else {
                    theme::TEXT_DIM()
                };
                ui.painter().text(
                    r.left_center() + vec2(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    crate::tr(g),
                    // Icon-Glyphe
                    egui::FontId::proportional(14.0),
                    col,
                );
                ui.painter().text(
                    r.left_center() + vec2(34.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    crate::tr(t),
                    egui::FontId::proportional(theme::FS_TEXT),
                    col,
                );
                resp.clicked()
            };
            nav(ui, icon::SQUARES_FOUR, "Projekte", true);
            if nav(ui, icon::FOLDER_SIMPLE, "Projektordner", false) {
                let d = paths::projects_dir();
                let _ = std::fs::create_dir_all(&d);
                reveal(&d);
            }
            if nav(ui, icon::GEAR_SIX, "Einstellungen", false) {
                self.settings_open = true;
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "Cutline {} · Open Source · MPL-2.0",
                        env!("CARGO_PKG_VERSION")
                    )))
                    .small()
                    .color(theme::TEXT_FAINT()),
                );
            });
        });

        // Hauptbereich
        ui.allocate_new_ui(
            egui::UiBuilder::new().max_rect(main.shrink2(vec2(24.0, 18.0))),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::tr("Projekte")).size(theme::FS_PAGE).strong());
                    ui.label(
                        RichText::new(crate::tr(format!("{}", self.home.entries.len())))
                            .color(theme::TEXT_FAINT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icon::LIST, "Liste", true, self.home.list_view)
                            .clicked()
                        {
                            self.home.list_view = true;
                        }
                        if theme::icon_button(
                            ui,
                            icon::SQUARES_FOUR,
                            "Kacheln",
                            true,
                            !self.home.list_view,
                        )
                        .clicked()
                        {
                            self.home.list_view = false;
                        }
                        ui.add_space(8.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.home.search)
                                .hint_text(crate::tr(format!(
                                    "{}  Projekte suchen",
                                    icon::MAGNIFYING_GLASS
                                )))
                                .desired_width(220.0),
                        );
                    });
                });
                ui.add_space(14.0);
                let q = self.home.search.to_lowercase();
                let idx: Vec<usize> = (0..self.home.entries.len())
                    .filter(|i| {
                        q.is_empty() || self.home.entries[*i].name.to_lowercase().contains(&q)
                    })
                    .collect();
                let mut open: Option<PathBuf> = None;
                let mut action: Option<(&'static str, PathBuf)> = None;
                egui::ScrollArea::vertical()
                    .id_salt("home")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.home.list_view {
                            for i in idx {
                                let (path, name, dur, clips, size, modified) = {
                                    let e = &self.home.entries[i];
                                    (
                                        e.path.clone(),
                                        e.name.clone(),
                                        e.duration,
                                        e.clips,
                                        e.size,
                                        e.modified,
                                    )
                                };
                                let (r, resp) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 52.0),
                                    Sense::click(),
                                );
                                if resp.hovered() {
                                    ui.painter().rect_filled(r, theme::R_CARD, theme::BG_RAISED());
                                }
                                let tr =
                                    Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(71.0, 40.0));
                                let tex = self.home.thumb(ui.ctx(), &path);
                                self.paint_thumb(ui.painter(), tr, tex);
                                ui.painter().text(
                                    pos2(tr.right() + 12.0, r.center().y - 8.0),
                                    egui::Align2::LEFT_CENTER,
                                    crate::tr(&name),
                                    egui::FontId::proportional(theme::FS_TEXT),
                                    theme::TEXT(),
                                );
                                ui.painter().text(
                                    pos2(tr.right() + 12.0, r.center().y + 9.0),
                                    egui::Align2::LEFT_CENTER,
                                    crate::tr(format!(
                                        "{} · {clips} Clips · {}×{}",
                                        fmt_dur(dur),
                                        size.0,
                                        size.1
                                    )),
                                    egui::FontId::proportional(theme::FS_HINT),
                                    theme::TEXT_FAINT(),
                                );
                                ui.painter().text(
                                    r.right_center() - vec2(14.0, 0.0),
                                    egui::Align2::RIGHT_CENTER,
                                    crate::tr(ago(modified)),
                                    egui::FontId::proportional(theme::FS_HINT),
                                    theme::TEXT_DIM(),
                                );
                                if resp.clicked() {
                                    open = Some(path.clone());
                                }
                                resp.context_menu(|ui| project_menu(ui, &path, &mut action));
                            }
                        } else {
                            // Kachel „Neues Projekt“ + Projekte
                            let n = idx.len() + 1;
                            let gap = 16.0;
                            let avail = ui.available_width();
                            let cols = ((avail + gap) / (210.0 + gap)).floor().max(1.0) as usize;
                            let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
                            let th = (w * 9.0 / 16.0).round();
                            let h = th + 46.0;
                            for row in 0..n.div_ceil(cols) {
                                let (rr, _) =
                                    ui.allocate_exact_size(vec2(avail, h), Sense::hover());
                                for col in 0..cols {
                                    let k = row * cols + col;
                                    if k >= n {
                                        break;
                                    }
                                    let r = Rect::from_min_size(
                                        pos2(rr.left() + col as f32 * (w + gap), rr.top()),
                                        vec2(w, h),
                                    );
                                    let thumb_r = Rect::from_min_size(r.min, vec2(w, th));
                                    if k == 0 {
                                        let resp = ui.interact(
                                            thumb_r,
                                            ui.id().with("new-proj"),
                                            Sense::click(),
                                        );
                                        let hov = resp.hovered();
                                        ui.painter().rect_filled(
                                            thumb_r,
                                            theme::R_CARD,
                                            if hov {
                                                theme::BG_HOVER()
                                            } else {
                                                theme::BG_RAISED()
                                            },
                                        );
                                        ui.painter().rect_stroke(
                                            thumb_r,
                                            theme::R_CARD,
                                            Stroke::new(
                                                1.0_f32,
                                                // kein Ruhe-Rahmen (Fläche hebt sich schon ab)
                                                if hov {
                                                    Color32::from_white_alpha(120)
                                                } else {
                                                    Color32::TRANSPARENT
                                                },
                                            ),
                                            egui::StrokeKind::Inside,
                                        );
                                        ui.painter().circle_filled(
                                            thumb_r.center() - vec2(0.0, 10.0),
                                            18.0,
                                            theme::ACCENT(),
                                        );
                                        ui.painter().text(
                                            thumb_r.center() - vec2(0.0, 10.0),
                                            egui::Align2::CENTER_CENTER,
                                            icon::PLUS,
                                            // Icon-Glyphe
                                            egui::FontId::proportional(18.0),
                                            theme::ACCENT_INK(),
                                        );
                                        ui.painter().text(
                                            thumb_r.center() + vec2(0.0, 24.0),
                                            egui::Align2::CENTER_CENTER,
                                            crate::tr("Neues Projekt"),
                                            egui::FontId::proportional(theme::FS_TEXT),
                                            theme::TEXT(),
                                        );
                                        ui.painter().text(
                                            pos2(r.left() + 2.0, thumb_r.bottom() + 14.0),
                                            egui::Align2::LEFT_CENTER,
                                            crate::tr("Oder Dateien hierher ziehen"),
                                            egui::FontId::proportional(theme::FS_HINT),
                                            theme::TEXT_FAINT(),
                                        );
                                        if resp.clicked() {
                                            self.new_project();
                                        }
                                        continue;
                                    }
                                    let i = idx[k - 1];
                                    let (path, name, dur, modified) = {
                                        let e = &self.home.entries[i];
                                        (e.path.clone(), e.name.clone(), e.duration, e.modified)
                                    };
                                    let resp = ui.interact(
                                        r,
                                        ui.id().with(("proj", &path)),
                                        Sense::click(),
                                    );
                                    let tex = self.home.thumb(ui.ctx(), &path);
                                    self.paint_thumb(ui.painter(), thumb_r, tex);
                                    if resp.hovered() {
                                        ui.painter().rect_stroke(
                                            thumb_r,
                                            theme::R_CARD,
                                            Stroke::new(1.5_f32, theme::ACCENT()),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    let badge = ui.painter().layout_no_wrap(
                                        fmt_dur(dur),
                                        egui::FontId::proportional(theme::FS_HINT),
                                        Color32::WHITE,
                                    );
                                    let br = Rect::from_min_size(
                                        thumb_r.right_bottom() - badge.size() - vec2(12.0, 9.0),
                                        badge.size() + vec2(6.0, 2.0),
                                    );
                                    ui.painter().rect_filled(
                                        br,
                                        theme::R_CTRL,
                                        Color32::from_black_alpha(170),
                                    );
                                    ui.painter().galley(
                                        br.min + vec2(3.0, 1.0),
                                        badge,
                                        Color32::WHITE,
                                    );
                                    let np = ui.painter().with_clip_rect(Rect::from_min_max(
                                        pos2(r.left(), thumb_r.bottom()),
                                        pos2(r.right() - 26.0, r.bottom()),
                                    ));
                                    np.text(
                                        pos2(r.left() + 2.0, thumb_r.bottom() + 14.0),
                                        egui::Align2::LEFT_CENTER,
                                        crate::tr(&name),
                                        egui::FontId::proportional(theme::FS_TEXT),
                                        theme::TEXT(),
                                    );
                                    np.text(
                                        pos2(r.left() + 2.0, thumb_r.bottom() + 31.0),
                                        egui::Align2::LEFT_CENTER,
                                        crate::tr(ago(modified)),
                                        egui::FontId::proportional(theme::FS_HINT),
                                        theme::TEXT_FAINT(),
                                    );
                                    // ⋯-Menü
                                    let mr = Rect::from_center_size(
                                        pos2(r.right() - 12.0, thumb_r.bottom() + 16.0),
                                        vec2(22.0, 22.0),
                                    );
                                    let mresp = ui.interact(
                                        mr,
                                        ui.id().with(("pmenu", &path)),
                                        Sense::click(),
                                    );
                                    if resp.hovered() || mresp.hovered() {
                                        ui.painter().text(
                                            mr.center(),
                                            egui::Align2::CENTER_CENTER,
                                            icon::DOTS_THREE,
                                            // Icon-Glyphe
                                            egui::FontId::proportional(16.0),
                                            theme::TEXT(),
                                        );
                                    }
                                    let pid = ui.make_persistent_id(("pmenu-pop", &path));
                                    if mresp.clicked() {
                                        ui.memory_mut(|m| m.toggle_popup(pid));
                                    } else if resp.clicked() {
                                        open = Some(path.clone());
                                    }
                                    egui::popup_below_widget(
                                        ui,
                                        pid,
                                        &mresp,
                                        egui::PopupCloseBehavior::CloseOnClick,
                                        |ui| {
                                            ui.set_min_width(180.0);
                                            project_menu(ui, &path, &mut action);
                                        },
                                    );
                                    resp.context_menu(|ui| project_menu(ui, &path, &mut action));
                                }
                                ui.add_space(gap);
                            }
                            if idx.is_empty() && !q.is_empty() {
                                theme::empty_state(ui, icon::MAGNIFYING_GLASS, "Keine Projekte gefunden", "");
                            }
                        }
                    });
                if let Some(p) = open {
                    self.open_project(&p);
                }
                if let Some((a, p)) = action {
                    match a {
                        "open" => self.open_project(&p),
                        "rename" => {
                            let name = self
                                .home
                                .entries
                                .iter()
                                .find(|e| e.path == p)
                                .map(|e| e.name.clone())
                                .unwrap_or_default();
                            self.home.rename = Some((p, name));
                        }
                        "duplicate" => self.duplicate_project(&p),
                        "reveal" => reveal(&p),
                        "props" => self.props = Some(crate::props::Props::Project(p)),
                        "exports" => {
                            self.exports_for = self.home.entries.iter().find(|e| e.path == p).map(|e| crate::exports::ExportsList::new(&e.name, Some(&e.path)));
                        }
                        "delete" => self.home.confirm_delete = Some(p),
                        _ => {}
                    }
                }
            },
        );
        self.home_dialogs(ui.ctx());
    }

    fn paint_thumb(&self, p: &egui::Painter, r: Rect, tex: Option<egui::TextureId>) {
        p.rect_filled(r, theme::R_CARD, theme::BG_RAISED());
        match tex {
            Some(t) => {
                p.add(
                    egui::epaint::RectShape::filled(r, theme::R_CARD, Color32::WHITE)
                        .with_texture(t, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                );
            }
            None => {
                p.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::FILM_SLATE,
                    egui::FontId::proportional(r.height() * 0.22),
                    theme::TEXT_FAINT(),
                );
            }
        }
    }

    fn duplicate_project(&mut self, p: &Path) {
        let Ok(s) = std::fs::read_to_string(p) else {
            return;
        };
        let Ok(mut proj) = Project::from_json(&s) else {
            return;
        };
        proj.name = format!("{} (Kopie)", proj.name);
        let dir = paths::projects_dir();
        let mut out = dir.join(format!("{}.cutline", proj.name));
        let mut n = 2;
        while out.exists() {
            out = dir.join(format!(
                "{} (Kopie {n}).cutline",
                proj.name.trim_end_matches(" (Kopie)")
            ));
            n += 1;
        }
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(&out, proj.to_json()).is_ok() {
            let _ = std::fs::copy(thumb_path(p), thumb_path(&out));
            self.toast("Projekt dupliziert");
        }
        self.home.refresh();
    }

    fn home_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((path, mut name)) = self.home.rename.take() {
            let mut keep = true;
            let mut save = false;
            let m = egui::Modal::new(egui::Id::new("rename")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    RichText::new(crate::tr("Projekt umbenennen"))
                        .strong()
                        .size(theme::FS_TITLE),
                );
                ui.add_space(10.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .id_salt("rename-field")
                        .desired_width(f32::INFINITY),
                );
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = true;
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::primary("Umbenennen")).clicked() {
                            save = true;
                        }
                        if ui.add(theme::secondary("Abbrechen")).clicked() {
                            keep = false;
                        }
                    });
                });
            });
            if m.should_close() {
                keep = false;
            }
            if save && !name.trim().is_empty() {
                if let Some(proj) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| Project::from_json(&s).ok())
                {
                    let mut proj = proj;
                    let old = std::mem::replace(&mut proj.name, name.trim().to_string());
                    if let Err(e) = paths::write_atomic(&path, proj.to_json()) {
                        self.toast_err(e.to_string());
                    } else {
                        let mut now = path.clone();
                        // Datei im Projektordner mit umbenennen
                        if path.starts_with(paths::projects_dir()) {
                            let np = path.with_file_name(format!("{}.cutline", paths::safe_name(&proj.name)));
                            if !np.exists() && std::fs::rename(&path, &np).is_ok() {
                                let _ = std::fs::rename(thumb_path(&path), thumb_path(&np));
                                self.settings.recent.retain(|r| r != &path);
                                now = np;
                            }
                        }
                        crate::exports::rename_project(&path, &now, &old, &proj.name);
                    }
                }
                self.home.refresh();
            } else if keep && !save {
                self.home.rename = Some((path, name));
            }
        }
        if let Some(path) = self.home.confirm_delete.clone() {
            let mut close = false;
            let m = egui::Modal::new(egui::Id::new("delete")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(crate::tr("Projekt löschen?")).strong().size(theme::FS_TITLE));
                ui.add_space(8.0);
                let name = self.home.entries.iter().find(|e| e.path == path).map(|e| e.name.clone()).unwrap_or_default();
                ui.label(crate::tr(format!("„{name}“ wird in den Cutline-Papierkorb verschoben. Deine Medien-Dateien bleiben unverändert.")));
                ui.label(RichText::new(crate::tr(paths::data_dir().join("Papierkorb").display().to_string())).small().color(theme::TEXT_FAINT()));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::danger("Löschen")).clicked() {
                            let trash = paths::data_dir().join("Papierkorb");
                            let _ = std::fs::create_dir_all(&trash);
                            let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                            let fname = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                            let target = trash.join(format!("{stamp}-{fname}"));
                            if std::fs::rename(&path, &target).or_else(|_| std::fs::copy(&path, &target).and_then(|_| std::fs::remove_file(&path))).is_ok() {
                                let _ = std::fs::remove_file(thumb_path(&path));
                                self.settings.recent.retain(|r| r != &path);
                                self.settings.save();
                                self.toast("Projekt in den Papierkorb verschoben");
                            }
                            close = true;
                        }
                        if ui.add(theme::secondary("Abbrechen")).clicked() {
                            close = true;
                        }
                    });
                });
            });
            if close || m.should_close() {
                self.home.confirm_delete = None;
                self.home.refresh();
            }
        }
    }
}

fn project_menu(ui: &mut egui::Ui, path: &Path, action: &mut Option<(&'static str, PathBuf)>) {
    for (a, g, t) in [
        ("open", icon::FOLDER_OPEN, "Öffnen"),
        ("rename", icon::PENCIL_SIMPLE, "Umbenennen"),
        ("duplicate", icon::COPY_SIMPLE, "Duplizieren"),
        ("reveal", icon::FOLDER_SIMPLE, "Im Explorer zeigen"),
        ("exports", icon::EXPORT, "Exporte"),
        ("props", icon::INFO, "Eigenschaften"),
        ("delete", icon::TRASH, "Löschen"),
    ] {
        if ui
            .add(theme::menu_row(crate::tr(format!("{g}   {t}"))).min_width(170.0))
            .clicked()
        {
            *action = Some((a, path.to_path_buf()));
            ui.close_menu();
        }
    }
}

pub fn fmt_dur(t: Time) -> String {
    let s = t.as_secs_f64().round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}
