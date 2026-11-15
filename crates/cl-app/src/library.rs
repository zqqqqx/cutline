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
    /// Videos, Bilder und Ton, die man in vielen Projekten braucht
    Media,
}

pub const MEDIA_EXT: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "m4v", "avi", "png", "jpg", "jpeg", "webp", "gif", "wav", "mp3", "ogg",
    "flac", "m4a", "aac", "opus",
];

/// Wohin sich eine Mediendatei speichern lässt: Bilder/GIFs als Element, Ton als Sound.
/// Videos nicht (Elemente können keine Videos, Ton daraus bräuchte eine Umwandlung).
pub fn save_kind(p: &Path) -> Option<Kind> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    [Kind::Sticker, Kind::Sound].into_iter().find(|k| k.exts().contains(&ext.as_str()))
}

/// Vorsatz für verknüpfte Ordner (eigene Ordner auf der Platte, werden nicht kopiert)
pub fn link_label(name: &str) -> String {
    format!("{} {name}", crate::theme::icon::LINK_SIMPLE)
}

impl Kind {
    pub const ALL: [(Kind, &'static str); 3] =
        [(Kind::Media, "Medien"), (Kind::Sticker, "Sticker"), (Kind::Sound, "Sounds")];
    pub fn dir(self) -> PathBuf {
        paths::data_dir().join("Library").join(match self {
            Kind::Sticker => "Stickers",
            Kind::Sound => "Sounds",
            Kind::Media => "Media",
        })
    }
    pub fn exts(self) -> &'static [&'static str] {
        match self {
            Kind::Sticker => STICKER_EXT,
            Kind::Sound => SOUND_EXT,
            Kind::Media => MEDIA_EXT,
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
    /// Liegt in einem verknüpften Ordner: nicht umbenennen, verschieben oder löschen
    pub linked: bool,
    /// Schlagworte aus dem Paket (Suche)
    pub tags: Vec<String>,
}

impl Item {
    pub fn is_own(&self) -> bool {
        self.path.is_some() && !self.linked
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    favorites: BTreeSet<String>,
    /// Zuletzt verwendet (neueste zuerst)
    recent: Vec<String>,
    hidden: BTreeSet<String>,
    areas: std::collections::BTreeMap<String, AreaLayout>,
    /// Verknüpfte Ordner: (Art, Pfad)
    linked: Vec<(String, PathBuf)>,
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
    /// Ordner → Eintragsschlüssel, die dort zuerst stehen: eigene Reihenfolge („Favoriten“ und
    /// eigene Ordner) bzw. oben angepinnte Einträge (übrige Ordner und „Alle“), siehe [`order_kind`]
    pub seq: std::collections::BTreeMap<String, Vec<String>>,
}

/// Wie ein Ordner seine Reihenfolge bestimmt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Eigene Reihenfolge, alle Einträge verschiebbar („Favoriten“, eigene Ordner)
    Own,
    /// Normale Reihenfolge, angepinnte Einträge zuerst (eingebaute/verknüpfte Ordner, „Alle“)
    Pins,
    /// Feste Reihenfolge („Zuletzt“)
    Fixed,
}

pub fn order_kind(cat: &str) -> Order {
    match cat {
        "Favoriten" => Order::Own,
        "Zuletzt" => Order::Fixed,
        c if c.starts_with("v:") => Order::Own,
        c if c.strip_prefix("p:").is_some_and(|n| !n.starts_with(crate::theme::icon::LINK_SIMPLE)) => Order::Own,
        _ => Order::Pins,
    }
}

/// Stellt die Einträge aus `order` in dieser Reihenfolge nach vorn; der Rest folgt unverändert. O(n).
pub fn arrange_by<T>(order: &[String], items: &mut Vec<T>, key: impl Fn(&T) -> String) {
    if order.is_empty() {
        return;
    }
    let rank: std::collections::HashMap<&str, usize> =
        order.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let (mut front, mut rest) = (Vec::new(), Vec::with_capacity(items.len()));
    for it in items.drain(..) {
        match rank.get(key(&it).as_str()) {
            Some(&r) => front.push((r, it)),
            None => rest.push(it),
        }
    }
    front.sort_by_key(|(r, _)| *r);
    items.extend(front.into_iter().map(|(_, t)| t));
    items.append(&mut rest);
}

/// `key` in `seq` vor `before` setzen (fehlt `before` in `seq`: ans Ende). `whole`: `seq` zuerst
/// aus der angezeigten Reihenfolge `shown` bilden (eigene Reihenfolge; nicht Angezeigtes bleibt
/// hinten erhalten), sonst nur Einträge verschieben, die schon in `seq` stehen (Pins).
pub fn move_in(seq: &mut Vec<String>, shown: &[String], key: &str, before: Option<&str>, whole: bool) {
    if before == Some(key) {
        return;
    }
    if whole {
        let set: std::collections::HashSet<&String> = shown.iter().collect();
        let mut v = shown.to_vec();
        v.extend(seq.iter().filter(|k| !set.contains(k)).cloned());
        *seq = v;
    }
    let Some(i) = seq.iter().position(|k| k == key) else { return };
    let k = seq.remove(i);
    let j = before.and_then(|b| seq.iter().position(|k| k == b)).unwrap_or(seq.len());
    seq.insert(j, k);
}

/// Liegt der Dateischlüssel `key` (`file:<pfad>`) im Ordner `dir`?
fn file_under(key: &str, dir: &Path) -> bool {
    key.strip_prefix("file:")
        .and_then(|p| p.strip_prefix(&dir.display().to_string()))
        .is_some_and(|r| r.starts_with(['/', '\\']))
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
    recent: Vec<String>,
    hidden: BTreeSet<String>,
    areas: std::collections::BTreeMap<String, AreaLayout>,
    linked: Vec<(String, PathBuf)>,
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
        Kind::Media => "med",
    }
}

/// Dateien eines verknüpften Ordners (bis drei Ebenen tief, höchstens 3000)
fn walk(dir: &Path, exts: &[&str], depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= 3000 {
            return;
        }
        if p.is_dir() {
            if depth > 0 {
                walk(&p, exts, depth - 1, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
        {
            out.push(p);
        }
    }
}

impl Library {
    pub fn load() -> Self {
        let s: Stored = paths::read_json_or_quarantine(&store());
        Self {
            favorites: s.favorites,
            recent: s.recent,
            hidden: s.hidden,
            areas: s.areas,
            linked: s.linked,
            cache: Default::default(),
        }
    }

    fn save(&self) {
        let p = store();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let s = Stored {
            favorites: self.favorites.clone(),
            recent: self.recent.clone(),
            hidden: self.hidden.clone(),
            areas: self.areas.clone(),
            linked: self.linked.clone(),
        };
        let _ = crate::paths::write_atomic(&p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }

    /// Eintrag als „zuletzt verwendet“ merken (höchstens 40, neueste zuerst).
    pub fn mark_used(&mut self, key: &str) {
        self.recent.retain(|k| k != key);
        self.recent.insert(0, key.to_string());
        self.recent.truncate(40);
        self.save();
    }
    /// Rang in „Zuletzt verwendet“ (0 = zuletzt), falls vorhanden.
    pub fn recent_rank(&self, key: &str) -> Option<usize> {
        self.recent.iter().position(|k| k == key)
    }
    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.contains(key)
    }
    pub fn toggle_favorite(&mut self, key: &str) {
        if !self.unfavorite(key) {
            self.favorites.insert(key.to_string());
        }
        self.save();
    }
    /// Aus den Favoriten nehmen, samt Platz in deren Reihenfolge (neu hinzugefügt kommt ans Ende).
    fn unfavorite(&mut self, key: &str) -> bool {
        for l in self.areas.values_mut() {
            if let Some(s) = l.seq.get_mut("Favoriten") {
                s.retain(|k| k != key);
            }
        }
        self.favorites.remove(key)
    }

    // ---------------------------------------------------------- Reihenfolge und Pins

    /// Gespeicherte Reihenfolge bzw. Pins eines Ordners (leer, wenn keine).
    pub fn order(&self, area: &str, cat: &str) -> &[String] {
        self.areas.get(area).and_then(|l| l.seq.get(cat)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Die fertig gefilterte Liste eines Tabs ordnen: eigene Reihenfolge bzw. Pins zuerst;
    /// bei Suchtext unverändert.
    pub fn arrange<T>(&self, area: &str, cat: &str, q: &str, items: &mut Vec<T>, key: impl Fn(&T) -> String) {
        if q.trim().is_empty() && order_kind(cat) != Order::Fixed {
            arrange_by(self.order(area, cat), items, key);
        }
    }

    /// Eintrag vor `before` bzw. ans Ende (`None`) setzen; `shown` = angezeigte Schlüssel in Reihenfolge.
    pub fn move_entry(&mut self, area: &str, cat: &str, shown: &[String], key: &str, before: Option<&str>) {
        let whole = match order_kind(cat) {
            Order::Own => true,
            Order::Pins => false,
            Order::Fixed => return,
        };
        let seq = self.areas.entry(area.to_string()).or_default().seq.entry(cat.to_string()).or_default();
        move_in(seq, shown, key, before, whole);
        self.save();
    }

    pub fn is_pinned(&self, area: &str, cat: &str, key: &str) -> bool {
        order_kind(cat) == Order::Pins && self.order(area, cat).iter().any(|k| k == key)
    }

    /// Oben anpinnen (hinten an die Angepinnten) bzw. lösen; nur in Ordnern ohne eigene Reihenfolge.
    pub fn toggle_pin(&mut self, area: &str, cat: &str, key: &str) {
        if order_kind(cat) != Order::Pins {
            return;
        }
        let l = self.areas.entry(area.to_string()).or_default();
        let seq = l.seq.entry(cat.to_string()).or_default();
        if seq.iter().any(|k| k == key) {
            seq.retain(|k| k != key);
            if seq.is_empty() {
                l.seq.remove(cat);
            }
        } else {
            seq.push(key.to_string());
        }
        self.save();
    }

    /// Eintragsschlüssel überall umschreiben (Favoriten, Zuletzt, Sammelordner, Reihenfolgen):
    /// `f` gibt `None` (unberührt), `Some(None)` (entfernen) oder `Some(Some(neu))` zurück.
    fn remap(&mut self, f: impl Fn(&str) -> Option<Option<String>>) {
        let fix = |v: &mut Vec<String>| {
            *v = std::mem::take(v)
                .into_iter()
                .filter_map(|k| match f(&k) {
                    None => Some(k),
                    Some(n) => n,
                })
                .collect();
        };
        let mut fav: Vec<String> = self.favorites.iter().cloned().collect();
        fix(&mut fav);
        self.favorites = fav.into_iter().collect();
        fix(&mut self.recent);
        for l in self.areas.values_mut() {
            l.virt.values_mut().chain(l.seq.values_mut()).for_each(fix);
        }
    }
    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden.contains(key)
    }
    pub fn hidden_count(&self, which: impl Fn(&str) -> bool) -> usize {
        self.hidden.iter().filter(|k| which(k)).count()
    }
    /// Eingebauten Eintrag ausblenden
    pub fn hide(&mut self, key: &str) {
        self.hidden.insert(key.to_string());
        self.unfavorite(key);
        self.save();
    }
    pub fn unhide_all(&mut self, which: impl Fn(&str) -> bool) {
        self.hidden.retain(|k| !which(k));
        self.save();
    }

    // ---------------------------------------------------------- Kategorien je Bereich

    /// Sichtbare Kategorien in gespeicherter Reihenfolge (neue hinten angehängt).
    pub fn cats(&mut self, area: &str, builtin: &[&str], phys: Option<Kind>) -> Vec<Cat> {
        let all = self.all_cats(area, builtin, phys);
        let l = self.areas.get(area).cloned().unwrap_or_default();
        all.into_iter()
            .filter(|c| !l.hidden.contains(&c.key))
            .collect()
    }

    fn all_cats(&mut self, area: &str, builtin: &[&str], phys: Option<Kind>) -> Vec<Cat> {
        let l = self.areas.get(area).cloned().unwrap_or_default();
        let mut found: Vec<Cat> = builtin
            .iter()
            .map(|b| {
                let key = format!("b:{b}");
                Cat {
                    label: l.names.get(&key).cloned().unwrap_or_else(|| crate::tr(*b)),
                    key,
                    builtin: true,
                }
            })
            .collect();
        if let Some(k) = phys {
            found.extend(self.folders(k).into_iter().map(|f| Cat {
                key: format!("p:{f}"),
                label: f,
                builtin: false,
            }));
        }
        found.extend(l.virt.keys().map(|v| Cat {
            key: format!("v:{v}"),
            label: v.clone(),
            builtin: false,
        }));
        let mut out: Vec<Cat> = l
            .order
            .iter()
            .filter_map(|k| found.iter().find(|c| &c.key == k).cloned())
            .collect();
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
        self.areas
            .entry(area.to_string())
            .or_default()
            .hidden
            .insert(key.to_string());
        self.save();
    }

    /// Kategorie um `delta` Plätze verschieben.
    pub fn move_cat(
        &mut self,
        area: &str,
        key: &str,
        delta: i32,
        builtin: &[&str],
        phys: Option<Kind>,
    ) {
        let mut order: Vec<String> = self
            .all_cats(area, builtin, phys)
            .into_iter()
            .map(|c| c.key)
            .collect();
        let Some(i) = order.iter().position(|k| k == key) else {
            return;
        };
        let j = (i as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        let k = order.remove(i);
        order.insert(j, k);
        self.areas.entry(area.to_string()).or_default().order = order;
        self.save();
    }

    /// Umbenennen: eingebaut = Anzeigename, Dateiordner = auf der Platte, Sammelordner = Name.
    pub fn rename_cat(
        &mut self,
        area: &str,
        key: &str,
        new: &str,
        phys: Option<Kind>,
    ) -> Result<(), String> {
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
                // Favoriten, Ordner und Reihenfolgen der Dateien mitnehmen
                let (ap, bp) = (
                    format!("file:{}", a.display()),
                    format!("file:{}", b.display()),
                );
                self.remap(|k| file_under(k, &a).then(|| Some(k.replacen(&ap, &bp, 1))));
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
        // Reihenfolge bzw. Pins wandern mit
        if let Some(s) = l.seq.remove(key) {
            l.seq.insert(new_key, s);
        }
        self.save();
        Ok(())
    }

    /// Eigenen Ordner entfernen (Dateiordner wandert in den Papierkorb). `used`: Dateien des offenen Projekts.
    pub fn delete_cat(&mut self, area: &str, key: &str, phys: Option<Kind>, used: &[PathBuf]) -> Result<(), String> {
        match key.split_once(':') {
            Some(("v", name)) => {
                self.areas
                    .entry(area.to_string())
                    .or_default()
                    .virt
                    .remove(name);
            }
            // verknüpfter Ordner: nur die Verknüpfung lösen, Dateien bleiben
            Some(("p", name)) if name.starts_with(crate::theme::icon::LINK_SIMPLE) => {
                let k = phys.ok_or("Kein Dateiordner")?;
                if let Some(d) = self.linked(k).into_iter().find(|d| {
                    link_label(&d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()) == name
                }) {
                    self.unlink_folder(k, &d);
                }
            }
            Some(("p", name)) => self.delete_folder(phys.ok_or("Kein Dateiordner")?, name, used)?,
            _ => return Err("Eingebaute Kategorien lassen sich nur verbergen".into()),
        }
        let l = self.areas.entry(area.to_string()).or_default();
        l.order.retain(|k| k != key);
        l.seq.remove(key);
        self.save();
        Ok(())
    }

    pub fn create_virtual(&mut self, area: &str, name: &str) -> Result<String, String> {
        let name = sanitize(name);
        if name.is_empty() {
            return Err("Ungültiger Ordnername".into());
        }
        self.areas
            .entry(area.to_string())
            .or_default()
            .virt
            .entry(name.clone())
            .or_default();
        self.save();
        Ok(format!("v:{name}"))
    }

    pub fn virtual_folders(&self, area: &str) -> Vec<String> {
        self.areas
            .get(area)
            .map(|l| l.virt.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn add_to_virtual(&mut self, area: &str, folder: &str, item: &str) {
        let v = self
            .areas
            .entry(area.to_string())
            .or_default()
            .virt
            .entry(folder.to_string())
            .or_default();
        if !v.iter().any(|x| x == item) {
            v.push(item.to_string());
        }
        self.save();
    }

    pub fn remove_from_virtual(&mut self, area: &str, folder: &str, item: &str) {
        if let Some(v) = self
            .areas
            .entry(area.to_string())
            .or_default()
            .virt
            .get_mut(folder)
        {
            v.retain(|x| x != item);
        }
        if let Some(s) = self.areas.get_mut(area).and_then(|l| l.seq.get_mut(&format!("v:{folder}"))) {
            s.retain(|x| x != item);
        }
        self.save();
    }

    /// Gehört ein Eintrag (`key`, eingebaute Kategorie bzw. Dateiordner `folder`) zur Kategorie?
    pub fn in_cat(&self, area: &str, cat: &str, key: &str, folder: &str) -> bool {
        match cat {
            "Alle" => {
                // Einträge aus verborgenen Kategorien erscheinen auch unter „Alle“ nicht
                let l = self.areas.get(area);
                !l.is_some_and(|l| {
                    l.hidden.contains(&format!("b:{folder}"))
                        || l.hidden.contains(&format!("p:{folder}"))
                })
            }
            "Favoriten" => self.is_favorite(key),
            "Zuletzt" => self.recent_rank(key).is_some(),
            c => match c.split_once(':') {
                Some(("b", n)) | Some(("p", n)) => n == folder,
                Some(("v", n)) => self
                    .areas
                    .get(area)
                    .and_then(|l| l.virt.get(n))
                    .is_some_and(|v| v.iter().any(|x| x == key)),
                _ => false,
            },
        }
    }

    /// Sichtbar unter Kategorie und Suche?
    pub fn visible(
        &self,
        area: &str,
        cat: &str,
        q: &str,
        key: &str,
        name: &str,
        folder: &str,
    ) -> bool {
        self.visible_tagged(area, cat, q, key, name, folder, &[])
    }

    /// Wie [`visible`](Self::visible); die Suche findet auch die Schlagworte eines Paket-Inhalts.
    #[allow(clippy::too_many_arguments)]
    pub fn visible_tagged(
        &self,
        area: &str,
        cat: &str,
        q: &str,
        key: &str,
        name: &str,
        folder: &str,
        tags: &[String],
    ) -> bool {
        let q = q.trim().to_lowercase();
        // Suchtext sucht in allen Ordnern (verborgene Ordner bleiben verborgen)
        let cat = if q.is_empty() { cat } else { "Alle" };
        !self.is_hidden(key)
            && self.in_cat(area, cat, key, folder)
            && (q.is_empty()
                || name.to_lowercase().contains(&q)
                || crate::tr(name).to_lowercase().contains(&q)
                || crate::tr(folder).to_lowercase().contains(&q)
                || tags.iter().any(|t| t.to_lowercase().contains(&q)))
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
                let mut dirs: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                dirs.sort();
                for d in dirs {
                    let folder = d
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    folders.push(folder.clone());
                    let Ok(files) = std::fs::read_dir(&d) else {
                        continue;
                    };
                    let mut files: Vec<PathBuf> = files
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                                k.exts().contains(&e.to_ascii_lowercase().as_str())
                            })
                        })
                        .collect();
                    files.sort();
                    for f in files {
                        let name = f
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        items.push(Item {
                            key: format!("file:{}", f.display()),
                            name,
                            folder: folder.clone(),
                            builtin: None,
                            path: Some(f),
                            linked: false,
                            tags: Vec::new(),
                        });
                    }
                }
            }
            // Verknüpfte Ordner: Dateien bleiben, wo sie sind
            for (kk, dir) in self.linked.iter().filter(|(kk, _)| kk == kind_key(k)) {
                let _ = kk;
                let folder = link_label(&dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                folders.push(folder.clone());
                let mut files = Vec::new();
                walk(dir, k.exts(), 3, &mut files);
                items.extend(files.into_iter().map(|f| Item {
                    key: format!("file:{}", f.display()),
                    name: f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    folder: folder.clone(),
                    builtin: None,
                    path: Some(f),
                    linked: true,
                    tags: Vec::new(),
                }));
            }
            (folders, items)
        })
    }

    pub fn linked(&self, k: Kind) -> Vec<PathBuf> {
        self.linked.iter().filter(|(kk, _)| kk == kind_key(k)).map(|(_, p)| p.clone()).collect()
    }

    pub fn link_folder(&mut self, k: Kind, dir: PathBuf) {
        if !self.linked.iter().any(|(kk, p)| kk == kind_key(k) && *p == dir) {
            self.linked.push((kind_key(k).to_string(), dir));
            self.save();
        }
        self.invalidate();
    }

    pub fn unlink_folder(&mut self, k: Kind, dir: &Path) {
        self.linked.retain(|(kk, p)| !(kk == kind_key(k) && p == dir));
        self.save();
        self.invalidate();
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
    pub fn delete_folder(&mut self, k: Kind, name: &str, used: &[PathBuf]) -> Result<(), String> {
        let d = k.dir().join(sanitize(name));
        // wie bei einzelnen Einträgen: was das offene Projekt nutzt, bleibt
        if used.iter().any(|u| u.starts_with(&d)) {
            return Err(format!("„{}“ enthält Dateien, die im offenen Projekt verwendet werden, und bleibt deshalb erhalten", name.trim()));
        }
        to_trash(&d)?;
        // die Dateien sind weg: ihre Favoriten, Pins und Reihenfolgen auch
        self.remap(|k| file_under(k, &d).then_some(None));
        self.save();
        self.invalidate();
        Ok(())
    }

    /// Dateien in einen Ordner kopieren. Gibt die Anzahl zurück.
    pub fn import(&mut self, k: Kind, folder: &str, files: &[PathBuf]) -> usize {
        let dir = k.dir().join(sanitize(folder));
        let _ = std::fs::create_dir_all(&dir);
        let mut n = 0;
        for f in files {
            let ok_ext = f
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| k.exts().contains(&e.to_ascii_lowercase().as_str()));
            if !ok_ext || !f.is_file() {
                continue;
            }
            let dest = unique(
                &dir,
                f.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            if std::fs::copy(f, &dest).is_ok() {
                n += 1;
            }
        }
        self.invalidate();
        n
    }

    /// Eigenen Eintrag in einen anderen Ordner verschieben.
    pub fn move_to(&mut self, item: &Item, k: Kind, folder: &str) -> Result<(), String> {
        let Some(p) = &item.path else {
            return Err("Eingebaute Einträge lassen sich nicht verschieben".into());
        };
        let dir = k.dir().join(sanitize(folder));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = unique(
            &dir,
            p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        std::fs::rename(p, &dest).map_err(|e| e.to_string())?;
        // die Reihenfolge des alten Ordners braucht den Eintrag nicht mehr
        for l in self.areas.values_mut() {
            if let Some(s) = l.seq.get_mut(&format!("p:{}", item.folder)) {
                s.retain(|k| *k != item.key);
            }
        }
        self.rekey(&item.key, &format!("file:{}", dest.display()));
        self.invalidate();
        Ok(())
    }

    /// Eigenen Eintrag umbenennen (Dateiname, Endung bleibt).
    pub fn rename(&mut self, item: &Item, new_name: &str) -> Result<(), String> {
        let Some(p) = &item.path else {
            return Err("Eingebaute Einträge lassen sich nicht umbenennen".into());
        };
        let name = sanitize(new_name);
        if name.is_empty() {
            return Err("Ungültiger Name".into());
        }
        let ext = p
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
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
                self.remap(|k| (k == item.key).then_some(None));
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
        self.remap(|k| (k == old).then(|| Some(new.to_string())));
        self.save();
    }
}

fn sanitize(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

pub fn unique(dir: &Path, file: String) -> PathBuf {
    let p = dir.join(&file);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file.clone(), String::new()),
    };
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// In den Cutline-Papierkorb verschieben (nichts wird endgültig gelöscht).
fn to_trash(p: &Path) -> Result<(), String> {
    let trash = paths::data_dir().join("Papierkorb").join("Library");
    std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    let dest = unique(
        &trash,
        p.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    std::fs::rename(p, dest).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_spans_all_folders_but_not_hidden_ones() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let mut lib = Library::default();
        let v = |lib: &Library, q: &str| lib.visible("effect", "b:Intro", q, "fx:shake", "Handkamera", "Bewegung");
        assert!(!v(&lib, ""), "ohne Suche nur der aktive Ordner");
        assert!(v(&lib, "hand"), "Suche über alle Ordner");
        lib.hide_cat("effect", "b:Bewegung");
        assert!(!v(&lib, "hand"), "verborgen bleibt verborgen");
    }


    #[test]
    fn media_saves_as_element_or_sound_into_new_folder() {
        assert_eq!(save_kind(Path::new("a.GIF")), Some(Kind::Sticker));
        assert_eq!(save_kind(Path::new("a.png")), Some(Kind::Sticker));
        assert_eq!(save_kind(Path::new("a.mp3")), Some(Kind::Sound));
        assert_eq!(save_kind(Path::new("a.mp4")), None);
        assert_eq!(save_kind(Path::new("a")), None);
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let src = home.path().join("clip.gif");
        std::fs::write(&src, b"x").unwrap();
        let mut lib = Library::default();
        // neuer Ordner und Kopie in einem Schritt, gleicher Name zweimal: zweite Kopie umbenannt
        assert_eq!(lib.import(Kind::Sticker, "Memes", std::slice::from_ref(&src)), 1);
        assert_eq!(lib.import(Kind::Sticker, "Memes", std::slice::from_ref(&src)), 1);
        assert_eq!(lib.folders(Kind::Sticker), vec!["Memes".to_string()]);
        assert_eq!(lib.own_items(Kind::Sticker).len(), 2);
        assert!(src.is_file(), "Original bleibt");
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arrange_puts_ordered_first_and_keeps_the_rest() {
        let mut items = keys(&["a", "b", "c", "d", "e"]);
        // unbekannte Schlüssel stören nicht, neue (nicht gelistete) folgen in alter Reihenfolge
        arrange_by(&keys(&["d", "gone", "b"]), &mut items, |s| s.clone());
        assert_eq!(items, keys(&["d", "b", "a", "c", "e"]));
        let mut same = keys(&["x", "y"]);
        arrange_by(&[], &mut same, |s| s.clone());
        assert_eq!(same, keys(&["x", "y"]));
    }

    #[test]
    fn move_before_to_end_and_onto_itself() {
        let shown = keys(&["a", "b", "c"]);
        // eigene Reihenfolge: aus der Anzeige gebildet, Verborgenes bleibt hinten
        let mut seq = keys(&["hidden"]);
        move_in(&mut seq, &shown, "c", Some("a"), true);
        assert_eq!(seq, keys(&["c", "a", "b", "hidden"]));
        move_in(&mut seq, &keys(&["c", "a", "b"]), "c", None, true);
        assert_eq!(seq, keys(&["a", "b", "hidden", "c"]));
        move_in(&mut seq, &shown, "b", Some("b"), true);
        assert_eq!(seq, keys(&["a", "b", "hidden", "c"]), "auf sich selbst: nichts");
        // Pins: nur Angepinnte bewegen sich; Ziel ohne Pin = Ende der Angepinnten
        let mut pins = keys(&["x", "y"]);
        move_in(&mut pins, &shown, "x", Some("a"), false);
        assert_eq!(pins, keys(&["y", "x"]));
        move_in(&mut pins, &shown, "a", Some("y"), false);
        assert_eq!(pins, keys(&["y", "x"]));
    }

    #[test]
    fn order_kinds() {
        assert_eq!(order_kind("Favoriten"), Order::Own);
        assert_eq!(order_kind("v:Intro"), Order::Own);
        assert_eq!(order_kind("p:Eigene"), Order::Own);
        assert_eq!(order_kind(&format!("p:{}", link_label("Clips"))), Order::Pins);
        assert_eq!(order_kind("Alle"), Order::Pins);
        assert_eq!(order_kind("b:Bewegung"), Order::Pins);
        assert_eq!(order_kind("Zuletzt"), Order::Fixed);
    }

    #[test]
    fn favorites_order_pins_and_search() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let mut lib = Library::default();
        let all = keys(&["fx:a", "fx:b", "fx:c"]);
        let id = |s: &String| s.clone();
        for k in &all {
            lib.toggle_favorite(k);
        }
        let mut v = all.clone();
        lib.arrange("effect", "Favoriten", "", &mut v, id);
        assert_eq!(v, all, "ohne eigene Reihenfolge wie bisher");
        lib.move_entry("effect", "Favoriten", &all, "fx:c", Some("fx:a"));
        let mut v = all.clone();
        lib.arrange("effect", "Favoriten", "", &mut v, id);
        assert_eq!(v, keys(&["fx:c", "fx:a", "fx:b"]));
        let mut v = all.clone();
        lib.arrange("effect", "Favoriten", "x", &mut v, id);
        assert_eq!(v, all, "Suche: normale Reihenfolge");
        // entfernter und neu gesetzter Favorit kommt ans Ende
        lib.toggle_favorite("fx:c");
        lib.toggle_favorite("fx:c");
        let mut v = all.clone();
        lib.arrange("effect", "Favoriten", "", &mut v, id);
        assert_eq!(v, keys(&["fx:a", "fx:b", "fx:c"]));
        // Pins je Ordner getrennt
        lib.toggle_pin("effect", "Alle", "fx:c");
        lib.toggle_pin("effect", "Alle", "fx:b");
        lib.toggle_pin("effect", "Favoriten", "fx:a");
        assert!(lib.is_pinned("effect", "Alle", "fx:c"));
        assert!(!lib.is_pinned("effect", "b:Bewegung", "fx:c"));
        assert!(!lib.is_pinned("effect", "Favoriten", "fx:a"), "kein Anpinnen in Favoriten");
        let mut v = all.clone();
        lib.arrange("effect", "Alle", "", &mut v, id);
        assert_eq!(v, keys(&["fx:c", "fx:b", "fx:a"]));
        lib.move_entry("effect", "Alle", &v, "fx:b", Some("fx:c"));
        let mut v = all.clone();
        lib.arrange("effect", "Alle", "", &mut v, id);
        assert_eq!(v, keys(&["fx:b", "fx:c", "fx:a"]));
        let mut v = all.clone();
        lib.arrange("effect", "b:Bewegung", "", &mut v, id);
        assert_eq!(v, all);
        lib.toggle_pin("effect", "Alle", "fx:b");
        assert!(!lib.is_pinned("effect", "Alle", "fx:b"));
        // ausgeblendet: Pin bleibt (kommt mit „wieder anzeigen“ zurück), Favorit geht
        lib.hide("fx:c");
        assert!(lib.is_pinned("effect", "Alle", "fx:c"));
        assert!(!lib.order("effect", "Favoriten").contains(&"fx:c".to_string()));
    }

    #[test]
    fn old_library_json_loads_and_new_fields_round_trip() {
        let old = r#"{"favorites":["fx:a"],"recent":[],"hidden":[],"areas":{"effect":{"order":["b:Licht"],"names":{},"hidden":[],"virt":{"Intro":["fx:a"]}}},"linked":[]}"#;
        let s: Stored = serde_json::from_str(old).unwrap();
        assert!(s.favorites.contains("fx:a"));
        assert!(s.areas["effect"].seq.is_empty());
        let mut s = s;
        s.areas.get_mut("effect").unwrap().seq.insert("Alle".into(), keys(&["fx:b"]));
        let back: Stored = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.areas["effect"].seq["Alle"], keys(&["fx:b"]));
    }

    #[test]
    fn own_folder_order_survives_rename_and_goes_with_delete() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let mut lib = Library::default();
        // Sammelordner
        let v = lib.create_virtual("effect", "Intro").unwrap();
        for k in ["fx:a", "fx:b"] {
            lib.add_to_virtual("effect", "Intro", k);
        }
        lib.move_entry("effect", &v, &keys(&["fx:a", "fx:b"]), "fx:b", Some("fx:a"));
        lib.rename_cat("effect", &v, "Intros", None).unwrap();
        assert_eq!(lib.order("effect", "v:Intros"), keys(&["fx:b", "fx:a"]));
        lib.remove_from_virtual("effect", "Intros", "fx:b");
        assert_eq!(lib.order("effect", "v:Intros"), keys(&["fx:a"]));
        lib.delete_cat("effect", "v:Intros", None, &[]).unwrap();
        assert!(lib.order("effect", "v:Intros").is_empty());
        // Dateiordner: Schlüssel der Dateien wandern mit, Nachbarordner mit gleichem Anfang nicht
        let src = home.path().join("in");
        std::fs::create_dir_all(&src).unwrap();
        for f in ["a.wav", "b.wav"] {
            std::fs::write(src.join(f), b"x").unwrap();
        }
        lib.import(Kind::Sound, "Gaming", &[src.join("a.wav"), src.join("b.wav")]);
        lib.import(Kind::Sound, "Gaming2", &[src.join("a.wav")]);
        let it = lib.own_items(Kind::Sound);
        let k: Vec<String> = it.iter().map(|i| i.key.clone()).collect();
        lib.move_entry("sfx", "p:Gaming", &k[..2], &k[1], Some(&k[0]));
        lib.toggle_pin("sfx", "Alle", &k[2]);
        lib.rename_cat("sfx", "p:Gaming", "Games", Some(Kind::Sound)).unwrap();
        let renamed: Vec<String> = lib.order("sfx", "p:Games").to_vec();
        assert_eq!(renamed.len(), 2);
        assert!(renamed[0].ends_with("b.wav") && renamed[0].contains("Games"));
        assert!(lib.is_pinned("sfx", "Alle", &k[2]), "Gaming2 bleibt unberührt");
        lib.toggle_pin("sfx", "Alle", &renamed[0]);
        lib.delete_cat("sfx", "p:Games", Some(Kind::Sound), &[]).unwrap();
        assert!(lib.order("sfx", "p:Games").is_empty());
        assert_eq!(lib.order("sfx", "Alle"), std::slice::from_ref(&k[2]), "Pin der gelöschten Datei weg");
    }

    #[test]
    fn linked_folders_are_listed_but_not_owned() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let ext = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ext.path().join("Clips/sub")).unwrap();
        std::fs::write(ext.path().join("Clips/a.mp4"), b"x").unwrap();
        std::fs::write(ext.path().join("Clips/sub/b.png"), b"x").unwrap();
        std::fs::write(ext.path().join("Clips/notes.txt"), b"x").unwrap();
        let mut lib = Library::default();
        lib.link_folder(Kind::Media, ext.path().join("Clips"));
        let items = lib.own_items(Kind::Media);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.linked && !i.is_own()));
        assert!(lib.folders(Kind::Media).contains(&link_label("Clips")));
        // Kategorie löschen löst nur die Verknüpfung
        lib.delete_cat("libmedia", &format!("p:{}", link_label("Clips")), Some(Kind::Media), &[]).unwrap();
        assert!(lib.own_items(Kind::Media).is_empty());
        assert!(ext.path().join("Clips/a.mp4").is_file());
    }

    #[test]
    fn import_folders_favorites_move_remove() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        let src = home.path().join("in");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("boom.wav"), b"x").unwrap();
        std::fs::write(src.join("note.txt"), b"x").unwrap();
        let mut lib = Library::load();
        assert_eq!(
            lib.import(
                Kind::Sound,
                "Gaming",
                &[src.join("boom.wav"), src.join("note.txt")]
            ),
            1
        );
        lib.create_folder(Kind::Sound, "Memes").unwrap();
        assert_eq!(
            lib.folders(Kind::Sound),
            vec!["Gaming".to_string(), "Memes".to_string()]
        );
        let it = lib.own_items(Kind::Sound).remove(0);
        lib.toggle_favorite(&it.key);
        assert!(lib.visible("sfx", "Favoriten", "", &it.key, &it.name, &it.folder));
        lib.move_to(&it, Kind::Sound, "Memes").unwrap();
        let moved = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(moved.folder, "Memes");
        assert!(
            lib.is_favorite(&moved.key),
            "Favorit bleibt beim Verschieben"
        );
        lib.rename(&moved, "Explosion").unwrap();
        let renamed = lib.own_items(Kind::Sound).remove(0);
        assert_eq!(renamed.name, "Explosion");
        assert!(lib.visible(
            "sfx",
            "Alle",
            "explo",
            &renamed.key,
            &renamed.name,
            &renamed.folder
        ));
        lib.remove(&renamed).unwrap();
        assert!(lib.own_items(Kind::Sound).is_empty());
        assert!(home
            .path()
            .join("Papierkorb/Library/Explosion.wav")
            .is_file());
        // eingebaute Einträge werden nur ausgeblendet
        let b = Item {
            key: "builtin:sfx:pop".into(),
            name: "Pop".into(),
            folder: "Oberfläche".into(),
            builtin: Some("pop"),
            path: None,
            linked: false,
            tags: vec!["Knall".into()],
        };
        lib.remove(&b).unwrap();
        assert!(!lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder));
        lib.unhide_all(|k| k.starts_with("builtin:sfx:"));
        // Suche über Schlagworte
        assert!(lib.visible_tagged("sfx", "Alle", "knall", &b.key, &b.name, &b.folder, &b.tags));
        assert!(!lib.visible("sfx", "Alle", "knall", &b.key, &b.name, &b.folder));
        assert!(lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder));
        // Kategorien: umbenennen (Anzeigename), verschieben, verbergen, Sammelordner
        let cats = lib.cats("sfx", &["Natur", "Oberfläche"], Some(Kind::Sound));
        assert_eq!(
            cats.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            ["b:Natur", "b:Oberfläche", "p:Gaming", "p:Memes"]
        );
        lib.rename_cat("sfx", "b:Natur", "Draußen", Some(Kind::Sound))
            .unwrap();
        lib.move_cat(
            "sfx",
            "p:Memes",
            -3,
            &["Natur", "Oberfläche"],
            Some(Kind::Sound),
        );
        lib.hide_cat("sfx", "b:Oberfläche");
        let cats = lib.cats("sfx", &["Natur", "Oberfläche"], Some(Kind::Sound));
        assert_eq!(cats[0].key, "p:Memes");
        assert!(cats.iter().any(|c| c.label == "Draußen"));
        assert!(!cats.iter().any(|c| c.key == "b:Oberfläche"));
        assert!(
            !lib.visible("sfx", "Alle", "", &b.key, &b.name, &b.folder),
            "Eintrag aus verborgener Kategorie"
        );
        let v = lib.create_virtual("sfx", "Intro").unwrap();
        lib.add_to_virtual("sfx", "Intro", "builtin:sfx:pop");
        assert!(lib.in_cat("sfx", &v, "builtin:sfx:pop", "Oberfläche"));
        lib.rename_cat("sfx", &v, "Intros", None).unwrap();
        assert!(lib.in_cat("sfx", "v:Intros", "builtin:sfx:pop", ""));
        lib.rename_cat("sfx", "p:Gaming", "Games", Some(Kind::Sound))
            .unwrap();
        assert!(Kind::Sound.dir().join("Games").is_dir());
        // Ordner mit Dateien des offenen Projekts bleibt
        let used = Kind::Sound.dir().join("Games/x.wav");
        std::fs::write(&used, b"x").unwrap();
        assert!(lib.delete_cat("sfx", "p:Games", Some(Kind::Sound), std::slice::from_ref(&used)).is_err());
        assert!(used.is_file());
        lib.delete_cat("sfx", "p:Games", Some(Kind::Sound), &[]).unwrap();
        assert!(!used.exists());
    }
}
