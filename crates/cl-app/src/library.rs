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
}

/// Zustand der Bibliothek (wird bei jeder Änderung gespeichert).
#[derive(Debug, Default)]
pub struct Library {
    favorites: BTreeSet<String>,
    hidden: BTreeSet<String>,
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
        Self { favorites: s.favorites, hidden: s.hidden, cache: Default::default() }
    }

    fn save(&self) {
        let p = store();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let s = Stored { favorites: self.favorites.clone(), hidden: self.hidden.clone() };
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

/// Filtert Einträge nach Ordner/Kategorie („Alle“, „Favoriten“ oder Name) und Suchtext.
pub fn filter(lib: &Library, items: Vec<Item>, cat: &str, search: &str) -> Vec<Item> {
    let q = search.trim().to_lowercase();
    items
        .into_iter()
        .filter(|i| !lib.is_hidden(&i.key))
        .filter(|i| match cat {
            "Alle" => true,
            "Favoriten" => lib.is_favorite(&i.key),
            c => i.folder == c,
        })
        .filter(|i| q.is_empty() || i.name.to_lowercase().contains(&q) || crate::tr(&i.name).to_lowercase().contains(&q) || i.folder.to_lowercase().contains(&q))
        .collect()
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
        let all = lib.clone_items(Kind::Sound);
        assert_eq!(filter(&lib, all, "Favoriten", "").len(), 1);
        lib.move_to(&it, Kind::Sound, "Memes").unwrap();
        let moved = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(moved.folder, "Memes");
        assert!(lib.is_favorite(&moved.key), "Favorit bleibt beim Verschieben");
        lib.rename(&moved, "Explosion").unwrap();
        let renamed = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(renamed.name, "Explosion");
        let all = lib.clone_items(Kind::Sound);
        assert_eq!(filter(&lib, all, "Alle", "explo").len(), 1);
        lib.remove(&renamed).unwrap();
        assert!(lib.own_items(Kind::Sound).is_empty());
        assert!(home.path().join("Papierkorb/Library/Explosion.wav").is_file());
        // eingebaute Einträge werden nur ausgeblendet
        let b = Item { key: "builtin:sfx:pop".into(), name: "Pop".into(), folder: "Oberfläche".into(), builtin: Some("pop"), path: None };
        lib.remove(&b).unwrap();
        assert!(filter(&lib, vec![b.clone()], "Alle", "").is_empty());
        lib.unhide_all("builtin:sfx:");
        assert_eq!(filter(&lib, vec![b], "Alle", "").len(), 1);
    }

    impl Library {
        fn clone_items(&mut self, k: Kind) -> Vec<Item> {
            self.own_items(k)
        }
    }
}
