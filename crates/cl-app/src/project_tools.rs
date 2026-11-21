//! Project tools: automatic backups with restore, relinking missing media
//! (search a folder) and archiving a project (project + media + fonts).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cl_core::{EditOp, Project};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};

/// Interval of the automatic backups
/// This many backups are kept per project
#[cfg(test)]
const BACKUP_KEEP: usize = 30;

/// Old folder `Backups/<file name>`: shared by same-named projects. Only read now.
fn legacy_backup_dir(project: &Path) -> PathBuf {
    let stem = project
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Projekt".into());
    paths::data_dir().join("Backups").join(stem)
}

/// `Backups/<file name>-<hash of the full path>`: same-named projects stay separate.
pub fn backup_dir(project: &Path) -> PathBuf {
    // FNV-1a: stable across program versions (unlike `DefaultHasher`)
    let key = project.to_string_lossy().replace('\\', "/").to_lowercase();
    let h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let mut d = legacy_backup_dir(project).into_os_string();
    d.push(format!("-{:08x}", h as u32));
    d.into()
}

fn backups_in(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut v: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
        .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok().map(|t| (p, t)))
        .collect();
    v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    v
}

/// Backups of a project (also from the old folder), newest first.
pub fn list_backups(project: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut v = backups_in(&backup_dir(project));
    v.extend(backups_in(&legacy_backup_dir(project)));
    v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    v
}

/// Project file renamed or moved: take the backups along.
pub fn move_backups(old: &Path, new: &Path) {
    let (a, b) = (backup_dir(old), backup_dir(new));
    if a.is_dir() && !b.exists() {
        let _ = std::fs::rename(a, b);
    }
}

/// Write a backup and remove old ones.
pub fn write_backup(project: &Path, json: &str, keep: usize) -> std::io::Result<PathBuf> {
    let dir = backup_dir(project);
    std::fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let p = dir.join(format!("{stamp}.cutline"));
    paths::write_atomic(&p, json)?;
    // only rotate in our own folder: the old one can contain other projects' backups
    for (old, _) in backups_in(&dir).into_iter().skip(keep.max(1)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(p)
}

/// Search files recursively by name (for missing media).
pub fn find_by_name(root: &Path, names: &[String], depth: usize) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return found;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                found.extend(find_by_name(&p, names, depth - 1));
            }
        } else if let Some(n) = p.file_name().map(|s| s.to_string_lossy().to_lowercase()) {
            if let Some(want) = names.iter().find(|w| w.to_lowercase() == n) {
                found.push((want.clone(), p));
            }
        }
    }
    found
}

/// Relink only unambiguous hits: the file name occurs once among the missing files and once in
/// the searched folder. Everything else stays missing rather than pointing at the wrong footage.
pub fn unique_relinks(missing: &[(cl_core::MediaId, PathBuf)], found: &[(String, PathBuf)]) -> Vec<(cl_core::MediaId, PathBuf)> {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    missing
        .iter()
        .filter_map(|(id, p)| {
            let n = name(p)?;
            let mut hits = found.iter().filter(|(_, f)| name(f).as_ref() == Some(&n));
            let hit = hits.next()?;
            // several media on the same path are one file and may share the hit
            let twin = missing.iter().any(|(_, q)| q != p && name(q).as_ref() == Some(&n));
            (hits.next().is_none() && !twin).then(|| (*id, hit.1.clone()))
        })
        .collect()
}

/// Archive a project: `<target>/<name>/` with `Medien/`, `Schriften/` and the adjusted project.
pub fn archive(p: &Project, fonts: &[PathBuf], dest_root: &Path) -> Result<PathBuf, String> {
    let safe = paths::safe_name(&p.name);
    let dir = (1..)
        .map(|i| {
            if i == 1 {
                dest_root.join(&safe)
            } else {
                dest_root.join(format!("{safe} ({i})"))
            }
        })
        .find(|d| !d.exists())
        .unwrap();
    let media_dir = dir.join("Medien");
    std::fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;
    let mut out = p.clone();
    let mut used = std::collections::HashSet::new();
    for m in &mut out.media {
        if !m.path.is_file() {
            continue;
        }
        let name = m
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // do not overwrite identical file names from different folders
        let mut target = media_dir.join(&name);
        let mut i = 2;
        while used.contains(&target) {
            target = media_dir.join(format!("{i}_{name}"));
            i += 1;
        }
        used.insert(target.clone());
        std::fs::copy(&m.path, &target).map_err(|e| format!("{}: {e}", m.path.display()))?;
        m.path = target;
    }
    if !fonts.is_empty() {
        let fd = dir.join("Schriften");
        std::fs::create_dir_all(&fd).map_err(|e| e.to_string())?;
        out.project_fonts.clear();
        for f in fonts.iter().filter(|f| f.is_file()) {
            let t = fd.join(f.file_name().unwrap_or_default());
            std::fs::copy(f, &t).map_err(|e| e.to_string())?;
            out.project_fonts.push(t);
        }
    }
    paths::write_atomic(&dir.join(format!("{safe}.cutline")), out.to_json())
        .map_err(|e| e.to_string())?;
    Ok(dir)
}

impl App {
    /// Every frame: backup due? Draw dialogs.
    pub fn project_tools(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("Projekt-Werkzeuge");
        // Automatic backup (only for saved projects with changes since the last one)
        if let Some(path) = self.project_path.clone() {
            let every = Duration::from_secs(self.settings.backup_minutes.max(1) as u64 * 60);
            let due = self.last_backup.is_none_or(|t| t.elapsed() >= every);
            if due && self.revision != self.backup_revision {
                self.backup_now(&path);
            }
        }
        if let Some(rx) = &self.archive {
            if let Ok(r) = rx.try_recv() {
                self.archive = None;
                match r {
                    Ok(d) => {
                        self.toast(format!("Archiv erstellt: {}", d.display()));
                        crate::home::reveal(&d);
                    }
                    Err(e) => self.toast_err(format!("Archivieren fehlgeschlagen: {e}")),
                }
            }
        }
        self.backups_window(ctx);
        self.missing_window(ctx);
        self.props_window(ctx);
        self.exports_window(ctx);
        self.shorts_window(ctx);
        self.thumb_window(ctx);
        self.aspect_fit_dialog(ctx);
    }

    pub fn start_archive(&mut self, ctx: &egui::Context) {
        let Some(dest) = crate::dialog()
            .set_title(crate::tr("Zielordner für das Archiv"))
            .pick_folder()
        else {
            return;
        };
        self.commit_live();
        let p = self.project.clone();
        let fonts: Vec<PathBuf> = p.project_fonts.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(archive(&p, &fonts, &dest));
            c.request_repaint();
        });
        self.archive = Some(rx);
        self.toast("Projekt wird archiviert …");
    }

    fn backups_window(&mut self, ctx: &egui::Context) {
        if !self.backups_open {
            return;
        }
        let mut open = true;
        let mut restore: Option<PathBuf> = None;
        crate::dialog::window(ctx, crate::tr("Sicherungen"), "backups").default_width(380.0).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            if crate::dialog::title(ui, "Sicherungen") {
                open = false;
            }
            ui.add_space(6.0);
            let Some(path) = self.project_path.clone() else {
                ui.label(crate::tr("Das Projekt ist noch nicht gespeichert."));
                return;
            };
            if ui.add(theme::secondary(format!("{}  Jetzt sichern", icon::FLOPPY_DISK))).clicked() {
                self.backup_now(&path);
            }
            ui.add_space(4.0);
            let list = list_backups(&path);
            if list.is_empty() {
                theme::empty_state(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Noch keine Sicherungen", "");
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (p, t) in list {
                    ui.horizontal(|ui| {
                        let local = chrono_like(t);
                        ui.label(RichText::new(local).monospace());
                        ui.label(RichText::new(crate::tr(crate::home::ago(t))).small().color(theme::TEXT_DIM()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(theme::ghost("Wiederherstellen")).clicked() {
                                restore = Some(p.clone());
                            }
                        });
                    });
                }
            });
        });
        if !open {
            self.backups_open = false;
        }
        // Read in the background; taken over (with a backup of the current state) in `poll_load`
        if let Some(p) = restore {
            self.start_load(&p, crate::loading::LoadMode::Restore, ctx);
        }
    }

    fn missing_window(&mut self, ctx: &egui::Context) {
        if !self.missing_open {
            return;
        }
        let missing: Vec<(cl_core::MediaId, PathBuf)> = self
            .project
            .media
            .iter()
            .filter(|m| !self.presence.get(&m.path))
            .map(|m| (m.id, m.path.clone()))
            .collect();
        let mut open = true;
        let mut search = false;
        let mut pick: Option<cl_core::MediaId> = None;
        crate::dialog::window(ctx, crate::tr("Medien neu verknüpfen"), "relink").default_width(460.0).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            if crate::dialog::title(ui, "Medien neu verknüpfen") {
                open = false;
            }
            if missing.is_empty() {
                ui.label(RichText::new(crate::tr(format!("{}  Alle Medien sind vorhanden", icon::CHECK_CIRCLE))).color(theme::TEXT_DIM()));
                return;
            }
            ui.label(crate::tr(theme::count_text(missing.len(), "{} Mediendatei wurde nicht gefunden (verschoben oder umbenannt).", "{} Mediendateien wurden nicht gefunden (verschoben oder umbenannt).")));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (id, p) in &missing {
                    // Button first (right), fixed row height; file name in full, folder shortened in the middle
                    let row = egui::vec2(ui.available_width(), 24.0);
                    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::ghost("Datei wählen …")).clicked() {
                            pick = Some(*id);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(icon::WARNING).color(theme::DANGER_TEXT()));
                            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            ui.add(egui::Label::new(RichText::new(&name).strong()).truncate()).on_hover_text(crate::paths::shown(p));
                            let dir = p.parent().map(crate::paths::shown).unwrap_or_default();
                            let font = egui::TextStyle::Small.resolve(ui.style());
                            let ctx = ui.ctx().clone();
                            let dir = theme::elide_middle(&dir, ui.available_width() - 4.0, |s| theme::text_width(&ctx, s, font.clone()));
                            ui.label(RichText::new(dir).small().color(theme::TEXT_DIM())).on_hover_text(crate::paths::shown(p));
                        });
                    });
                }
            });
            crate::dialog::footer(ui, |ui| {
                if ui.add(theme::primary(format!("{}  Ordner durchsuchen …", icon::MAGNIFYING_GLASS))).on_hover_text(crate::tr("Sucht alle fehlenden Dateien nach Namen in einem Ordner und seinen Unterordnern")).clicked() {
                    search = true;
                }
                if ui.add(theme::secondary("Abbrechen")).clicked() {
                    open = false;
                }
            });
        });
        if !open {
            self.missing_open = false;
        }
        if let Some(id) = pick {
            if let Some(f) = crate::dialog().pick_file() {
                self.apply(EditOp::RelinkMedia { media: id, path: f });
                self.invalidate_preview();
            }
        }
        if search {
            if let Some(dir) = crate::dialog().pick_folder() {
                let names: Vec<String> = missing
                    .iter()
                    .filter_map(|(_, p)| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .collect();
                let found = find_by_name(&dir, &names, 6);
                let ops: Vec<EditOp> = unique_relinks(&missing, &found)
                    .into_iter()
                    .map(|(media, path)| EditOp::RelinkMedia { media, path })
                    .collect();
                let n = ops.len();
                if n > 0 {
                    self.apply_batch(ops);
                    self.invalidate_preview();
                }
                self.toast(format!("{n} von {} Dateien gefunden", missing.len()));
            }
        }
    }
}

/// Date/time in local time without an extra dependency (Windows: not exact via the system
/// time zone; we show the system time free of UTC offset via `ago` next to it).
fn chrono_like(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let off = local_offset_secs();
    let s = secs + off;
    let days = s.div_euclid(86400);
    let rem = s.rem_euclid(86400);
    // Days → date (proleptic Gregorian calendar)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{d:02}.{m:02}.{y} {:02}:{:02}", rem / 3600, rem / 60 % 60)
}

/// Offset of local time from UTC in seconds (from the environment; default Central Europe).
pub(crate) fn local_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        // PowerShell/`date` would be expensive; we read the offset once via the Windows-API-free
        // variant: comparing file timestamps is not possible, hence the TZ variable or +1 h.
        if let Ok(tz) = std::env::var("CUTLINE_TZ_OFFSET") {
            return tz.parse::<i64>().unwrap_or(0) * 60;
        }
        if cfg!(windows) {
            local_offset_windows()
        } else {
            0
        }
    })
}

#[cfg(windows)]
fn local_offset_windows() -> i64 {
    #[repr(C)]
    struct SystemTimeW([u16; 8]);
    #[repr(C)]
    struct TzInfo {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTimeW,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTimeW,
        daylight_bias: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(tz: *mut TzInfo) -> u32;
    }
    let mut tz: TzInfo = unsafe { std::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    // 2 = daylight saving time active
    let bias = tz.bias
        + if r == 2 {
            tz.daylight_bias
        } else {
            tz.standard_bias
        };
    -(bias as i64) * 60
}

#[cfg(not(windows))]
fn local_offset_windows() -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_named_projects_keep_separate_backups() {
        let home = crate::paths::TestHome::new();
        let a = home.path().join("x/Film.cutline");
        let b = home.path().join("y/Film.cutline");
        // An old-style backup stays visible
        let legacy = legacy_backup_dir(&a);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("1.cutline"), "{}").unwrap();
        write_backup(&a, "{}", 1).unwrap();
        write_backup(&b, "{}", 1).unwrap();
        assert!(legacy.join("1.cutline").is_file(), "rotation leaves other backups in place");
        assert_eq!(list_backups(&a).len(), 2);
        assert_ne!(backup_dir(&a), backup_dir(&b));
        // Renaming takes the backups along
        let c = home.path().join("x/Neu.cutline");
        move_backups(&a, &c);
        assert_eq!(backups_in(&backup_dir(&c)).len(), 1);
    }

    #[test]
    fn backups_rotate_and_archive_copies() {
        let home = crate::paths::TestHome::new();
        let proj = home.path().join("Film.cutline");
        for i in 0..(BACKUP_KEEP + 3) {
            let p = write_backup(&proj, &format!("{{\"n\":{i}}}"), BACKUP_KEEP).unwrap();
            // force unique timestamps
            let _ = std::fs::rename(&p, p.with_file_name(format!("{}.cutline", 1000 + i)));
        }
        let _ = write_backup(&proj, "{}", BACKUP_KEEP);
        assert!(list_backups(&proj).len() <= BACKUP_KEEP + 1);
        // Archive
        let media = home.path().join("src");
        std::fs::create_dir_all(media.join("a")).unwrap();
        std::fs::create_dir_all(media.join("b")).unwrap();
        std::fs::write(media.join("a/clip.mp4"), b"1").unwrap();
        std::fs::write(media.join("b/clip.mp4"), b"2").unwrap();
        let mut p = Project::new("Film");
        for (i, sub) in ["a", "b"].iter().enumerate() {
            let mut m: cl_core::MediaRef = serde_json::from_value(serde_json::json!({
                "id": cl_core::MediaId::new(), "path": media.join(sub).join("clip.mp4"), "kind": "Video", "duration": 1000000,
                "width": 2, "height": 2, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
            })).unwrap();
            m.width = 2 + i as u32;
            p.media.push(m);
        }
        let out = archive(&p, &[], &home.path().join("out")).unwrap();
        let back = Project::from_json(&std::fs::read_to_string(out.join("Film.cutline")).unwrap())
            .unwrap();
        assert!(back
            .media
            .iter()
            .all(|m| m.path.starts_with(out.join("Medien")) && m.path.is_file()));
        assert_ne!(
            back.media[0].path, back.media[1].path,
            "same names stay separate"
        );
        // Search by name
        let f = find_by_name(&media, &["CLIP.mp4".into()], 3);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn relink_by_folder_skips_ambiguous_names() {
        let id = |_| cl_core::MediaId::new();
        let (a, b, c, d) = (id(0), id(1), id(2), id(3));
        let missing = vec![
            (a, PathBuf::from("/alt/1/GX010001.MP4")),
            (b, PathBuf::from("/alt/1/solo.mp4")),
            (c, PathBuf::from("/alt/1/twin.mp4")),
            (d, PathBuf::from("/alt/2/twin.mp4")),
        ];
        let found: Vec<(String, PathBuf)> = vec![
            ("GX010001.MP4".into(), "/neu/a/GX010001.MP4".into()),
            ("GX010001.MP4".into(), "/neu/b/GX010001.MP4".into()),
            ("solo.mp4".into(), "/neu/SOLO.mp4".into()),
            ("twin.mp4".into(), "/neu/twin.mp4".into()),
        ];
        // two files with one name in the folder, or two missing media with one name: stays missing
        assert_eq!(unique_relinks(&missing, &found), vec![(b, PathBuf::from("/neu/SOLO.mp4"))]);
        // the same path used by two media is one file
        let same = vec![(a, PathBuf::from("/alt/x.mp4")), (b, PathBuf::from("/alt/x.mp4"))];
        let hit = vec![("x.mp4".to_string(), PathBuf::from("/neu/x.mp4"))];
        assert_eq!(unique_relinks(&same, &hit).len(), 2);
    }
}
