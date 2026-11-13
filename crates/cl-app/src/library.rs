//! Eigene Bibliothek für Sticker/Bilder und Soundeffekte: importieren (Knopf oder Ablegen),
//! eigene Ordner, Favoriten, Suche, verschieben, umbenennen und entfernen. Eingebaute Einträge
//! lassen sich ausblenden und wieder einblenden.
//!
//! Ablage: `<Daten>/Library/Stickers/<Ordner>/<Datei>` und `<Daten>/Library/Sounds/<Ordner>/<Datei>`,
//! Favoriten und ausgeblendete Einträge in `<Daten>/Library/library.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

pub const STICKER_EXT: &[&str] = &["png", "webp", "gif", "svg", "jpg", "jpeg"];
pub const SOUND_EXT: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac", "opus"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sticker,
    Sound,
}

impl Kind {
    pub fn dir(self) -> PathBuf {
        paths::data_dir().join("Library").join(match self {
            Kind::Sticker => "Stickers",
            Kind::Sound => "Sounds",
        })
    }
    pub fn exts(self) -> &'static [&'static str] {
        match self {
            Kind::Sticker => STICKER_EXT,
            Kind::Sound => SOUND_EXT,
        }
    }
    /// Standardordner für Importe ohne Ordnerwahl
    pub const DEFAULT_FOLDER: &'static str = "Eigene";
}

/// Ein Eintrag der Bibliothek (eingebaut oder eigene Datei).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Eindeutiger Schlüssel: `builtin:<id>` oder `file:<pfad>`
    pub key: String,
    pub name: String,
    /// Kategorie (eingebaut) bzw. Ordnername (eigen)
    pub folder: String,
    pub builtin: Option<&'static str>,
    pub path: Option<PathBuf>,
}

impl Item {
    pub fn is_own(&self) -> bool {
        self.path.is_some()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    favorites: BTreeSet<String>,
    hidden: BTreeSet<String>,
    areas: std::collections::BTreeMap<String, AreaLayout>,
}

/// Anordnung eines Asset-Bereichs (Sticker, Soundeffekte, Effekte, …).
///
/// Kategorie-Schlüssel: `b:<Name>` eingebaute Kategorie, `p:<Ordner>` eigener Dateiordner,
/// `v:<Name>` eigener Sammelordner (enthält Verweise auf beliebige Einträge).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AreaLayout {
    /// Reihenfolge der Kategorien
    pub order: Vec<String>,
    /// Eigene Anzeigenamen für eingebaute Kategorien
    pub names: std::collections::BTreeMap<String, String>,
    /// Verborgene Kategorien
    pub hidden: BTreeSet<String>,
    /// Sammelordner → Eintragsschlüssel
    pub virt: std::collections::BTreeMap<String, Vec<String>>,
}

/// Eine Kategorie zur Anzeige
#[derive(Debug, Clone, PartialEq)]
pub struct Cat {
    pub key: String,
    pub label: String,
    /// Eingebaut (nur umbenennbar als Anzeigename, nicht löschbar)
    pub builtin: bool,
}

/// Zustand der Bibliothek (wird bei jeder Änderung gespeichert).
#[derive(Debug, Default)]
pub struct Library {
    favorites: BTreeSet<String>,
    hidden: BTreeSet<String>,
    areas: std::collections::BTreeMap<String, AreaLayout>,
    /// Zwischenspeicher der Dateilisten (nach Änderungen neu einlesen)
    cache: std::collections::HashMap<&'static str, (Vec<String>, Vec<Item>)>,
}

fn store() -> PathBuf {
    paths::data_dir().join("Library").join("library.json")
}

fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::Sticker => "st",
        Kind::Sound => "snd",
    }
}

impl Library {
    pub fn load() -> Self {
        let s: Stored = std::fs::read_to_string(store()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { favorites: s.favorites, hidden: s.hidden, areas: s.areas, cache: Default::default() }
    }

    fn save(&self) {
        let p = store();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let s = Stored { favorites: self.favorites.clone(), hidden: self.hidden.clone(), areas: self.areas.clone() };
        let _ = std::fs::write(p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.contains(key)
    }
    pub fn toggle_favorite(&mut self, key: &str) {
        if !self.favorites.remove(key) {
            self.favorites.insert(key.to_string());
        }
        self.save();
    }
    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden.contains(key)
    }
    pub fn hidden_count(&self, prefix: &str) -> usize {
        self.hidden.iter().filter(|k| k.starts_with(prefix)).count()
    }
    /// Eingebauten Eintrag ausblenden
    pub fn hide(&mut self, key: &str) {
        self.hidden.insert(key.to_string());
        self.favorites.remove(key);
        self.save();
    }
    pub fn unhide_all(&mut self, prefix: &str) {
        self.hidden.retain(|k| !k.starts_with(prefix));
        self.save();
    }

    // ---------------------------------------------------------- Kategorien je Bereich

    /// Sichtbare Kategorien in gespeicherter Reihenfolge (neue hinten angehängt).
    pub fn cats(&mut self, area: &str, builtin: &[&str], phys: Option<Kind>) -> Vec<Cat> {
        let all = self.all_cats(area, builtin, phys);
        let l = self.areas.get(area).cloned().unwrap_or_default();
        all.into_iter().filter(|c| !l.hidden.contains(&c.key)).collect()
    }

    fn all_cats(&mut self, area: &str, builtin: &[&str], phys: Option<Kind>) -> Vec<Cat> {
        let l = self.areas.get(area).cloned().unwrap_or_default();
        let mut found: Vec<Cat> = builtin
            .iter()
            .map(|b| {
                let key = format!("b:{b}");
                Cat { label: l.names.get(&key).cloned().unwrap_or_else(|| crate::tr(*b)), key, builtin: true }
            })
            .collect();
        if let Some(k) = phys {
            found.extend(self.folders(k).into_iter().map(|f| Cat { key: format!("p:{f}"), label: f, builtin: false }));
        }
        found.extend(l.virt.keys().map(|v| Cat { key: format!("v:{v}"), label: v.clone(), builtin: false }));
        let mut out: Vec<Cat> = l.order.iter().filter_map(|k| found.iter().find(|c| &c.key == k).cloned()).collect();
        out.extend(found.into_iter().filter(|c| !l.order.contains(&c.key)));
        out
    }

    pub fn hidden_cats(&self, area: &str) -> usize {
        self.areas.get(area).map(|l| l.hidden.len()).unwrap_or(0)
    }

    pub fn unhide_cats(&mut self, area: &str) {
        if let Some(l) = self.areas.get_mut(area) {
            l.hidden.clear();
        }
        self.save();
    }

    pub fn hide_cat(&mut self, area: &str, key: &str) {
        self.areas.entry(area.to_string()).or_default().hidden.insert(key.to_string());
        self.save();
    }

    /// Kategorie um `delta` Plätze verschieben.
    pub fn move_cat(&mut self, area: &str, key: &str, delta: i32, builtin: &[&str], phys: Option<Kind>) {
        let mut order: Vec<String> = self.all_cats(area, builtin, phys).into_iter().map(|c| c.key).collect();
        let Some(i) = order.iter().position(|k| k == key) else { return };
        let j = (i as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        let k = order.remove(i);
        order.insert(j, k);
        self.areas.entry(area.to_string()).or_default().order = order;
        self.save();
    }

    /// Umbenennen: eingebaut = Anzeigename, Dateiordner = auf der Platte, Sammelordner = Name.
    pub fn rename_cat(&mut self, area: &str, key: &str, new: &str, phys: Option<Kind>) -> Result<(), String> {
        let new = sanitize(new);
        if new.is_empty() {
            return Err("Ungültiger Name".into());
        }
        let l = self.areas.entry(area.to_string()).or_default();
        let new_key = match key.split_once(':') {
            Some(("b", _)) => {
                l.names.insert(key.to_string(), new.clone());
                key.to_string()
            }
            Some(("v", old)) => {
                if l.virt.contains_key(&new) {
                    return Err("Ordner gibt es schon".into());
                }
                let items = l.virt.remove(old).unwrap_or_default();
                l.virt.insert(new.clone(), items);
                format!("v:{new}")
            }
            Some(("p", old)) => {
                let k = phys.ok_or("Kein Dateiordner")?;
                let (a, b) = (k.dir().join(old), k.dir().join(&new));
                if b.exists() {
                    return Err("Ordner gibt es schon".into());
                }
                std::fs::rename(&a, &b).map_err(|e| e.to_string())?;
                // Favoriten der Dateien mitnehmen
                let (ap, bp) = (format!("file:{}", a.display()), format!("file:{}", b.display()));
                let moved: Vec<String> = self.favorites.iter().filter(|f| f.starts_with(&ap)).cloned().collect();
                for f in moved {
                    self.favorites.remove(&f);
                    self.favorites.insert(f.replacen(&ap, &bp, 1));
                }
                self.invalidate();
                format!("p:{new}")
            }
            _ => return Err("Unbekannte Kategorie".into()),
        };
        let l = self.areas.entry(area.to_string()).or_default();
        for k in &mut l.order {
            if k == key {
                *k = new_key.clone();
            }
        }
        self.save();
        Ok(())
    }

    /// Eigenen Ordner entfernen (Dateiordner wandert in den Papierkorb).
    pub fn delete_cat(&mut self, area: &str, key: &str, phys: Option<Kind>) -> Result<(), String> {
        match key.split_once(':') {
            Some(("v", name)) => {
                self.areas.entry(area.to_string()).or_default().virt.remove(name);
            }
            Some(("p", name)) => self.delete_folder(phys.ok_or("Kein Dateiordner")?, name)?,
            _ => return Err("Eingebaute Kategorien lassen sich nur verbergen".into()),
        }
        self.areas.entry(area.to_string()).or_default().order.retain(|k| k != key);
        self.save();
        Ok(())
    }

    pub fn create_virtual(&mut self, area: &str, name: &str) -> Result<String, String> {
        let name = sanitize(name);
        if name.is_empty() {
            return Err("Ungültiger Ordnername".into());
        }
        self.areas.entry(area.to_string()).or_default().virt.entry(name.clone()).or_default();
        self.save();
        Ok(format!("v:{name}"))
    }

    pub fn virtual_folders(&self, area: &str) -> Vec<String> {
        self.areas.get(area).map(|l| l.virt.keys().cloned().collect()).unwrap_or_default()
    }

    pub fn add_to_virtual(&mut self, area: &str, folder: &str, item: &str) {
        let v = self.areas.entry(area.to_string()).or_default().virt.entry(folder.to_string()).or_default();
        if !v.iter().any(|x| x == item) {
            v.push(item.to_string());
        }
        self.save();
    }

    pub fn remove_from_virtual(&mut self, area: &str, folder: &str, item: &str) {
        if let Some(v) = self.areas.entry(area.to_string()).or_default().virt.get_mut(folder) {
            v.retain(|x| x != item);
        }
        self.save();
    }

    /// Gehört ein Eintrag (`key`, eingebaute Kategorie bzw. Dateiordner `folder`) zur Kategorie?
    pub fn in_cat(&self, area: &str, cat: &str, key: &str, folder: &str) -> bool {
        match cat {
            "Alle" => {
                // Einträge aus verborgenen Kategorien erscheinen auch unter „Alle“ nicht
                let l = self.areas.get(area);
                !l.is_some_and(|l| l.hidden.contains(&format!("b:{folder}")) || l.hidden.contains(&format!("p:{folder}")))
            }
            "Favoriten" => self.is_favorite(key),
            c => match c.split_once(':') {
                Some(("b", n)) | Some(("p", n)) => n == folder,
                Some(("v", n)) => self.areas.get(area).and_then(|l| l.virt.get(n)).is_some_and(|v| v.iter().any(|x| x == key)),
                _ => false,
            },
        }
    }

    /// Sichtbar unter Kategorie und Suche?
    pub fn visible(&self, area: &str, cat: &str, q: &str, key: &str, name: &str, folder: &str) -> bool {
        let q = q.trim().to_lowercase();
        !self.is_hidden(key)
            && self.in_cat(area, cat, key, folder)
            && (q.is_empty() || name.to_lowercase().contains(&q) || crate::tr(name).to_lowercase().contains(&q) || crate::tr(folder).to_lowercase().contains(&q))
    }

    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    /// Eigene Ordner (Unterordner der Ablage), alphabetisch.
    pub fn folders(&mut self, k: Kind) -> Vec<String> {
        self.scan(k).0.clone()
    }

    /// Eigene Dateien
    pub fn own_items(&mut self, k: Kind) -> Vec<Item> {
        self.scan(k).1.clone()
    }

    fn scan(&mut self, k: Kind) -> &(Vec<String>, Vec<Item>) {
        self.cache.entry(kind_key(k)).or_insert_with(|| {
            let root = k.dir();
            let mut folders = Vec::new();
            let mut items = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&root) {
                let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                dirs.sort();
                for d in dirs {
                    let folder = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    folders.push(folder.clone());
                    let Ok(files) = std::fs::read_dir(&d) else { continue };
                    let mut files: Vec<PathBuf> = files.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| k.exts().contains(&e.to_ascii_lowercase().as_str()))).collect();
                    files.sort();
                    for f in files {
                        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        items.push(Item { key: format!("file:{}", f.display()), name, folder: folder.clone(), builtin: None, path: Some(f) });
                    }
                }
            }
            (folders, items)
        })
    }

    pub fn create_folder(&mut self, k: Kind, name: &str) -> Result<(), String> {
        let name = sanitize(name);
        if name.is_empty() {
            return Err("Ungültiger Ordnername".into());
        }
        std::fs::create_dir_all(k.dir().join(&name)).map_err(|e| e.to_string())?;
        self.invalidate();
        Ok(())
    }

    /// Leeren Ordner löschen; Ordner mit Inhalt wandern in den Papierkorb.
    pub fn delete_folder(&mut self, k: Kind, name: &str) -> Result<(), String> {
        let d = k.dir().join(sanitize(name));
        to_trash(&d)?;
        self.invalidate();
        Ok(())
    }

    /// Dateien in einen Ordner kopieren. Gibt die Anzahl zurück.
    pub fn import(&mut self, k: Kind, folder: &str, files: &[PathBuf]) -> usize {
        let dir = k.dir().join(sanitize(folder));
        let _ = std::fs::create_dir_all(&dir);
        let mut n = 0;
        for f in files {
            let ok_ext = f.extension().and_then(|e| e.to_str()).is_some_and(|e| k.exts().contains(&e.to_ascii_lowercase().as_str()));
            if !ok_ext || !f.is_file() {
                continue;
            }
            let dest = unique(&dir, f.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            if std::fs::copy(f, &dest).is_ok() {
                n += 1;
            }
        }
        self.invalidate();
        n
    }

    /// Eigenen Eintrag in einen anderen Ordner verschieben.
    pub fn move_to(&mut self, item: &Item, k: Kind, folder: &str) -> Result<(), String> {
        let Some(p) = &item.path else { return Err("Eingebaute Einträge lassen sich nicht verschieben".into()) };
        let dir = k.dir().join(sanitize(folder));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = unique(&dir, p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        std::fs::rename(p, &dest).map_err(|e| e.to_string())?;
        self.rekey(&item.key, &format!("file:{}", dest.display()));
        self.invalidate();
        Ok(())
    }

    /// Eigenen Eintrag umbenennen (Dateiname, Endung bleibt).
    pub fn rename(&mut self, item: &Item, new_name: &str) -> Result<(), String> {
        let Some(p) = &item.path else { return Err("Eingebaute Einträge lassen sich nicht umbenennen".into()) };
        let name = sanitize(new_name);
        if name.is_empty() {
            return Err("Ungültiger Name".into());
        }
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dest = unique(p.parent().unwrap_or(Path::new(".")), format!("{name}{ext}"));
        std::fs::rename(p, &dest).map_err(|e| e.to_string())?;
        self.rekey(&item.key, &format!("file:{}", dest.display()));
        self.invalidate();
        Ok(())
    }

    /// Entfernen: eigene Datei in den Papierkorb, eingebauten Eintrag ausblenden.
    pub fn remove(&mut self, item: &Item) -> Result<(), String> {
        match &item.path {
            Some(p) => {
                to_trash(p)?;
                self.favorites.remove(&item.key);
                self.save();
                self.invalidate();
                Ok(())
            }
            None => {
                self.hide(&item.key);
                Ok(())
            }
        }
    }

    fn rekey(&mut self, old: &str, new: &str) {
        if self.favorites.remove(old) {
            self.favorites.insert(new.to_string());
            self.save();
        }
    }
}

fn sanitize(s: &str) -> String {
    s.trim().chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect::<String>().trim_matches('.').to_string()
}

fn unique(dir: &Path, file: String) -> PathBuf {
    let p = dir.join(&file);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file.clone(), String::new()),
    };
    (2..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

/// In den Cutline-Papierkorb verschieben (nichts wird endgültig gelöscht).
fn to_trash(p: &Path) -> Result<(), String> {
    let trash = paths::data_dir().join("Papierkorb").join("Library");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    let dest = unique(&trash, p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    std::fs::rename(p, dest).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_folders_favorites_move_remove() {
        let _g = crate::paths::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let src = home.path().join("in");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("boom.wav"), b"x").unwrap();
        std::fs::write(src.join("note.txt"), b"x").unwrap();
        let mut lib = Library::load();
        assert_eq!(lib.import(Kind::Sound, "Gaming", &[src.join("boom.wav"), src.join("note.txt")]), 1);
        lib.create_folder(Kind::Sound, "Memes").unwrap();
        assert_eq!(lib.folders(Kind::Sound), vec!["Gaming".to_string(), "Memes".to_string()]);
        let it = lib.own_items(Kind::Sound).remove(0);
        lib.toggle_favorite(&it.key);
        assert!(lib.visible("sfx", "Favoriten", "", &it.key, &it.name, &it.folder));
        lib.move_to(&it, Kind::Sound, "Memes").unwrap();
        let moved = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(moved.folder, "Memes");
        assert!(lib.is_favorite(&moved.key), "Favorit bleibt beim Verschieben");
        lib.rename(&moved, "Explosion").unwrap();
        let renamed = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(renamed.name, "Explosion");
        assert!(lib.visible("sfx", "Alle", "explo", &renamed.key, &renamed.name, &renamed.folder));
        lib.remove(&renamed).unwrap();
        assert!(lib.own_items(Kind::Sound).is_empty());
        assert!(home.path().join("Papierkorb/Library/Explosion.wav").is_file());
        // eingebaute Einträge werden nur ausgeblendet
        let b = Item { key: "builtin:sfx:pop".into(), name: "Pop".into(), folder: "Oberfläche".into(), builtin: Some("pop"), path: None };
        lib.remove(&b).unwrap();
        assert!(!lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder));
        lib.unhide_all("builtin:sfx:");
        assert!(lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder));
        // Kategorien: umbenennen (Anzeigename), verschieben, verbergen, Sammelordner
        let cats = lib.cats("sfx", &["Natur", "Oberfläche"], Some(Kind::Sound));
        assert_eq!(cats.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["b:Natur", "b:Oberfläche", "p:Gaming", "p:Memes"]);
        lib.rename_cat("sfx", "b:Natur", "Draußen", Some(Kind::Sound)).unwrap();
        lib.move_cat("sfx", "p:Memes", -3, &["Natur", "Oberfläche"], Some(Kind::Sound));
        lib.hide_cat("sfx", "b:Oberfläche");
        let cats = lib.cats("sfx", &["Natur", "Oberfläche"], Some(Kind::Sound));
        assert_eq!(cats[0].key, "p:Memes");
        assert!(cats.iter().any(|c| c.label == "Draußen"));
        assert!(!cats.iter().any(|c| c.key == "b:Oberfläche"));
        assert!(!lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder), "Eintrag aus verborgener Kategorie");
        let v = lib.create_virtual("sfx", "Intro").unwrap();
        lib.add_to_virtual("sfx", "Intro", "builtin:sfx:pop");
        assert!(lib.in_cat("sfx", &v, "builtin:sfx:pop", "Oberfläche"));
        lib.rename_cat("sfx", &v, "Intros", None).unwrap();
        assert!(lib.in_cat("sfx", "v:Intros", "builtin:sfx:pop", ""));
        lib.rename_cat("sfx", "p:Gaming", "Games", Some(Kind::Sound)).unwrap();
        assert!(Kind::Sound.dir().join("Games").is_dir());
    }
}
